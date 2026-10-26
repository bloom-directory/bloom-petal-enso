//! Settlement verification via on-chain balance observations.
//!
//! After a swap is broadcast, the destination token balance of the receiver
//! should increase. This module compares a pre-stage baseline (stored in the
//! session) against the current on-chain balance.

use crate::api_types::NATIVE_TOKEN;
use crate::runtime::Host;
use crate::session::Session;
/// Compute the current settlement status for a session. Machine's receipt
/// carries no logs, but successful safe routes prove their atomic output check
/// passed. Later balance observations include fees and unrelated activity.
/// Only `source_pending` (including a failed outbox inspection) can still
/// change on its own, so only it carries a retry hint.
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
                "status": "source_pending",
                "error": format!("cannot inspect route outbox: {error}"),
                "route_outbox_id": route_outbox_id,
                "retry_interval_ms": 10000,
            });
        }
    };

    if inspection.state != "success" {
        let failed = matches!(
            inspection.state.as_str(),
            "failed" | "reverted" | "cancelled"
        );
        let mut status = serde_json::json!({
            "status": if failed { "source_failed" } else { "source_pending" },
            "source_state": inspection.state,
            "source_tx_hash": inspection.tx_hash,
            "route_outbox_id": route_outbox_id,
            "destination_chain": dest_chain,
            "receiver": receiver,
            "token_out": token_out_hex,
        });
        if !failed {
            status["retry_interval_ms"] = 10000.into();
        }
        return status;
    }

    if !dest_chain.eq_ignore_ascii_case(&sess.chain) {
        return serde_json::json!({
            "status": "destination_unverified",
            "message": "Destination settlement tracking is unsupported in this package; source success does not prove destination receipt.",
            "source_state": inspection.state,
            "source_tx_hash": inspection.tx_hash,
            "route_outbox_id": route_outbox_id,
            "destination_chain": dest_chain,
            "receiver": receiver,
            "token_out": token_out_hex,
            "minimum_expected": if sess.schema_version >= 2 {
                req.min_amount_out.map(|minimum| minimum.to_string())
            } else { None },
            "minimum_source": "reviewed_expectation_not_source_chain_guarantee",
            "retry_supported": false,
        });
    }
    // The router checked the receiver's balance within execution, before gas
    // charges or later transfers affect our observation. Do not turn a mined
    // protected swap into a failure based on that later net balance.
    if let Some(minimum) = sess.route.as_ref().and_then(|route| {
        crate::calldata::verify_output_checks(route, req)
            .ok()
            .filter(|verified| *verified)
            .and_then(|_| crate::calldata::output_minimum(route, req))
    }) {
        let mut status = serde_json::json!({
            "status": "destination_minimum_enforced",
            "source_state": inspection.state,
            "source_tx_hash": inspection.tx_hash,
            "route_outbox_id": route_outbox_id,
            "minimum_expected": minimum.to_string(),
            "minimum_source": "router_calldata",
            "destination_chain": dest_chain,
            "receiver": receiver,
            "token_out": token_out_hex,
            "observed_before": sess.observed_before,
        });
        match read_balance(host, dest_chain, &token_out_hex, &receiver) {
            Ok(current) => {
                if let Some(before) = &sess.observed_before {
                    status["delta"] = sub_decimal(&current, before).into();
                }
                status["observed_after"] = current.into();
            }
            Err(error) => status["observation_error"] = error.into(),
        }
        return status;
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
    let (minimum, minimum_source) = match sess.route.as_ref() {
        Some(route) => match crate::calldata::output_minimum(route, req) {
            Some(value) => (value.to_string(), "router_calldata"),
            None if is_decimal(route.amount_out.trim()) => {
                (route.amount_out.trim().to_string(), "enso_quote_unverified")
            }
            None => {
                return serde_json::json!({"status": "error", "error": "Enso route has no valid output floor"});
            }
        },
        _ => {
            return serde_json::json!({
                "status": "error",
                "error": "Enso route has no valid quoted output floor",
            });
        }
    };
    let status = if !lt_decimal(&delta, &minimum) {
        "destination_observed_unattributed"
    } else {
        "destination_below_minimum"
    };

    serde_json::json!({
        "status": status,
        "source_state": inspection.state,
        "source_tx_hash": inspection.tx_hash,
        "route_outbox_id": route_outbox_id,
        "observed_before": before,
        "observed_after": current,
        "delta": delta,
        "minimum_expected": minimum,
        "minimum_source": minimum_source,
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

pub(crate) fn lt_decimal(a: &str, b: &str) -> bool {
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
