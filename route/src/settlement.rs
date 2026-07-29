//! Settlement verification via on-chain balance observations.
//!
//! After a swap is broadcast, the destination token balance of the receiver
//! should increase. This module compares a pre-stage baseline (stored in the
//! session) against the current on-chain balance.

use crate::api_types::NATIVE_TOKEN;
use crate::runtime::Host;
use crate::session::Session;

/// Compute the current settlement status for a session. A destination balance
/// increase is only considered after the route outbox entry has a successful
/// mined receipt, and only when the increase meets the quoted output floor.
pub fn settlement_status<H: Host>(host: &mut H, sess: &Session) -> serde_json::Value {
    let req = match sess.route_request.as_ref() {
        Some(r) => r,
        None => {
            return serde_json::json!({
                "status": "error",
                "error": "session has no route request",
            });
        }
    };

    let dest_chain = sess.destination_chain.as_deref().unwrap_or(&sess.chain);

    // Determine receiver.
    let receiver = req
        .receiver
        .map(|a| format!("0x{:x}", a))
        .unwrap_or_else(|| format!("0x{:x}", req.from_address));

    let token_out_hex = format!("0x{:x}", req.token_out);

    let route_index = sess
        .intents
        .iter()
        .position(|intent| intent.label == "route");
    let Some(route_state) = route_index.and_then(|index| sess.intent_states.get(index)) else {
        return serde_json::json!({
            "status": "error",
            "error": "session has no route intent state",
        });
    };
    let Some(route_outbox_id) = route_state.outbox_id.as_deref() else {
        return serde_json::json!({
            "status": if sess.state == "awaiting_approval" {
                "awaiting_approval"
            } else {
                "not_staged"
            },
            "destination_chain": dest_chain,
            "receiver": receiver,
            "token_out": token_out_hex,
        });
    };

    let inspection = match host.tx_inspect(&sess.wallet, &sess.chain, route_outbox_id) {
        Ok(value) => value,
        Err(error) => {
            return serde_json::json!({
                "status": "error",
                "error": format!("cannot inspect route outbox: {error}"),
                "route_outbox_id": route_outbox_id,
            });
        }
    };

    if inspection.state != "success" {
        let failed = matches!(
            inspection.state.as_str(),
            "failed" | "reverted" | "cancelled"
        );
        return serde_json::json!({
            "status": if failed { "source_failed" } else { "source_pending" },
            "source_state": inspection.state,
            "source_tx_hash": inspection.tx_hash,
            "route_outbox_id": route_outbox_id,
            "destination_chain": dest_chain,
            "receiver": receiver,
            "token_out": token_out_hex,
        });
    }

    let current = match read_balance(host, dest_chain, &token_out_hex, &receiver) {
        Ok(value) => value,
        Err(error) => {
            return serde_json::json!({
                "status": "error",
                "error": format!("cannot read destination balance: {error}"),
                "source_state": inspection.state,
                "source_tx_hash": inspection.tx_hash,
            });
        }
    };

    let Some(before) = sess.observed_before.clone() else {
        return serde_json::json!({
            "status": "unverified_baseline",
            "error": "session has no trusted pre-route balance observation",
            "source_state": inspection.state,
            "source_tx_hash": inspection.tx_hash,
        });
    };

    let delta = sub_decimal(&current, &before);
    let minimum = match sess.route.as_ref().map(|route| route.amount_out.trim()) {
        Some(value) if is_decimal(value) => value,
        _ => {
            return serde_json::json!({
                "status": "error",
                "error": "Enso route has no valid quoted output floor",
            });
        }
    };
    let received = !lt_decimal(&delta, minimum);

    serde_json::json!({
        "status": if received {
            "destination_received"
        } else {
            "destination_below_quote"
        },
        "source_state": inspection.state,
        "source_tx_hash": inspection.tx_hash,
        "route_outbox_id": route_outbox_id,
        "observed_before": before,
        "observed_after": current,
        "delta": delta,
        "minimum_expected": minimum,
        "destination_chain": dest_chain,
        "receiver": receiver,
        "token_out": token_out_hex,
    })
}

/// Observe the pre-stage balance of the output token for the receiver.
///
/// Called during `create` so the session captures a baseline.
pub fn observe_balance_before<H: Host>(
    host: &mut H,
    dest_chain: &str,
    token_out_hex: &str,
    receiver_hex: &str,
) -> Result<String, String> {
    read_balance(host, dest_chain, token_out_hex, receiver_hex)
}

fn read_balance<H: Host>(
    host: &mut H,
    chain: &str,
    token: &str,
    receiver: &str,
) -> Result<String, String> {
    let value = if token.eq_ignore_ascii_case(NATIVE_TOKEN) {
        host.eth_balance(chain, receiver)?
    } else {
        host.erc20_balance(chain, token, receiver)?
    };
    if !is_decimal(&value) {
        return Err("balance host returned a non-decimal value".into());
    }
    Ok(normalize_decimal(&value))
}

fn is_decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn normalize_decimal(value: &str) -> String {
    let normalized = value.trim_start_matches('0');
    if normalized.is_empty() {
        "0".into()
    } else {
        normalized.into()
    }
}

fn lt_decimal(a: &str, b: &str) -> bool {
    let a = normalize_decimal(a);
    let b = normalize_decimal(b);
    if a.len() != b.len() {
        return a.len() < b.len();
    }
    a < b
}

/// Subtract two non-negative decimal strings: `a - b`. Returns "0" if b > a.
fn sub_decimal(a: &str, b: &str) -> String {
    if !is_decimal(a) || !is_decimal(b) {
        return "0".to_string();
    }
    let a_digits: Vec<u8> = a
        .chars()
        .rev()
        .filter_map(|c| c.to_digit(10))
        .map(|d| d as u8)
        .collect();
    let b_digits: Vec<u8> = b
        .chars()
        .rev()
        .filter_map(|c| c.to_digit(10))
        .map(|d| d as u8)
        .collect();

    if b_digits.len() > a_digits.len() {
        return "0".to_string();
    }

    let mut result = Vec::with_capacity(a_digits.len());
    let mut borrow = 0i32;

    for i in 0..a_digits.len() {
        let av = a_digits[i] as i32;
        let bv = if i < b_digits.len() {
            b_digits[i] as i32
        } else {
            0
        };
        let mut diff = av - bv - borrow;
        if diff < 0 {
            diff += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        result.push(diff as u8);
    }

    // If there's still a borrow, b > a.
    if borrow > 0 {
        return "0".to_string();
    }

    // Strip leading zeros.
    while result.len() > 1 && *result.last().unwrap() == 0 {
        result.pop();
    }

    result.iter().rev().map(|d| (b'0' + d) as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_subtraction() {
        assert_eq!(sub_decimal("1000", "300"), "700");
        assert_eq!(sub_decimal("100", "100"), "0");
        assert_eq!(sub_decimal("50", "100"), "0"); // underflow → 0
        assert_eq!(sub_decimal("0", "0"), "0");
    }

    #[test]
    fn large_numbers() {
        assert_eq!(
            sub_decimal("1000000000000000000000", "1"),
            "999999999999999999999"
        );
    }

    #[test]
    fn strip_leading_zeros() {
        assert_eq!(sub_decimal("1000", "999"), "1");
    }
}
