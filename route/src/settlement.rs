//! Settlement verification via on-chain balance observations.
//!
//! After a swap is broadcast, the destination token balance of the receiver
//! should increase. This module compares a pre-stage baseline (stored in the
//! session) against the current on-chain balance.

use crate::api_types::NATIVE_TOKEN;
use crate::runtime::Host;
use crate::session::Session;

/// Compute the current settlement status for a session.
///
/// Returns a JSON value:
/// ```json
/// {
///   "status": "not_broadcast" | "destination_pending" | "destination_received" | "unsupported_token",
///   "observed_before": "…",
///   "observed_after": "…",
///   "destination_chain": "…",
///   "receiver": "…",
///   "token_out": "…",
///   "delta": "…"
/// }
/// ```
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

    let dest_chain = sess
        .destination_chain
        .as_deref()
        .unwrap_or(&sess.chain);

    // Determine receiver.
    let receiver = req
        .receiver
        .map(|a| format!("0x{:x}", a))
        .unwrap_or_else(|| format!("0x{:x}", req.from_address));

    let token_out_hex = format!("0x{:x}", req.token_out);

    // Native token balance can't be read via erc20_balance.
    if token_out_hex.eq_ignore_ascii_case(NATIVE_TOKEN) {
        return serde_json::json!({
            "status": "unsupported_token",
            "note": "native token output — use eth_getBalance instead of erc20 balanceOf",
            "destination_chain": dest_chain,
            "receiver": receiver,
            "token_out": token_out_hex,
        });
    }

    // Read current balance on destination chain.
    let current = host
        .erc20_balance(dest_chain, &token_out_hex, &receiver)
        .unwrap_or_else(|_| "0".to_string());

    let before = sess.observed_before.clone().unwrap_or_else(|| "0".to_string());

    // Compute delta = current - before (as decimal string subtraction).
    let delta = sub_decimal(&current, &before);
    let received = delta != "0";

    // Determine status.
    let status = if sess.staged_ids.is_empty() {
        "not_broadcast"
    } else if received {
        "destination_received"
    } else {
        "destination_pending"
    };

    serde_json::json!({
        "status": status,
        "observed_before": before,
        "observed_after": current,
        "delta": delta,
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
) -> Option<String> {
    if token_out_hex.eq_ignore_ascii_case(NATIVE_TOKEN) {
        return None; // can't read native via erc20
    }
    match host.erc20_balance(dest_chain, token_out_hex, receiver_hex) {
        Ok(s) => Some(s),
        Err(_) => None,
    }
}

/// Subtract two non-negative decimal strings: `a - b`. Returns "0" if b > a.
fn sub_decimal(a: &str, b: &str) -> String {
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

    result
        .iter()
        .rev()
        .map(|d| (b'0' + d) as char)
        .collect()
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
            sub_decimal(
                "1000000000000000000000",
                "1"
            ),
            "999999999999999999999"
        );
    }

    #[test]
    fn strip_leading_zeros() {
        assert_eq!(sub_decimal("1000", "999"), "1");
    }
}
