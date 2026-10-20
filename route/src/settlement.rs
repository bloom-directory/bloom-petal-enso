//! Settlement verification via on-chain balance observations.
//!
//! After a swap is broadcast, the destination token balance of the receiver
//! should increase. This module compares a pre-stage baseline (stored in the
//! session) against the current on-chain balance.

use crate::api_types::NATIVE_TOKEN;
use crate::runtime::Host;
use crate::session::Session;
use alloy::primitives::U256;

const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// Compute the current settlement status for a session. Same-chain ERC-20
/// completion requires an attributable receipt log; balance-only observations
/// are reported without claiming that the route caused them.
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
    let balance_reached_quote = !lt_decimal(&delta, &minimum);
    let same_chain = dest_chain.eq_ignore_ascii_case(&sess.chain);
    let receipt_transfer = if same_chain && !token_out_hex.eq_ignore_ascii_case(NATIVE_TOKEN) {
        inspection
            .receipt_json
            .as_deref()
            .and_then(|receipt| transfer_amount_to_receiver(receipt, &token_out_hex, &receiver))
    } else {
        None
    };
    let receipt_reached_quote = receipt_transfer
        .as_deref()
        .is_some_and(|amount| !lt_decimal(amount, &minimum));
    let status = if receipt_reached_quote {
        "destination_received"
    } else if balance_reached_quote {
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
        "receipt_transfer_amount": receipt_transfer,
        "minimum_expected": minimum,
        "minimum_source": minimum_source,
        "destination_chain": dest_chain,
        "receiver": receiver,
        "token_out": token_out_hex,
    })
}

pub(crate) fn transfer_amount_to_receiver(
    receipt: &str,
    token: &str,
    receiver: &str,
) -> Option<String> {
    let receipt: serde_json::Value = serde_json::from_str(receipt).ok()?;
    let logs = receipt.get("logs")?.as_array()?;
    let receiver = receiver
        .strip_prefix("0x")
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    let mut total = U256::ZERO;
    let mut spent = U256::ZERO;
    let mut matched = false;
    for log in logs {
        if log.get("removed").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        let Some(address) = log.get("address").and_then(|value| value.as_str()) else {
            continue;
        };
        if !address.eq_ignore_ascii_case(token) {
            continue;
        }
        let Some(topics) = log.get("topics").and_then(|value| value.as_array()) else {
            continue;
        };
        if topics.len() != 3 {
            continue;
        }
        let (Some(event_topic), Some(recipient_topic)) = (topics[0].as_str(), topics[2].as_str())
        else {
            continue;
        };
        let recipient_topic = recipient_topic.trim_start_matches("0x");
        let Some(sender_topic) = topics[1].as_str().and_then(|s| s.strip_prefix("0x")) else {
            continue;
        };
        if !event_topic.eq_ignore_ascii_case(TRANSFER_TOPIC) {
            continue;
        }
        if recipient_topic.len() != 64
            || sender_topic.len() != 64
            || !recipient_topic
                .bytes()
                .chain(sender_topic.bytes())
                .all(|b| b.is_ascii_hexdigit())
            || &recipient_topic[..24] != "000000000000000000000000"
            || &sender_topic[..24] != "000000000000000000000000"
        {
            continue;
        }
        let Some(data) = log.get("data").and_then(|value| value.as_str()) else {
            continue;
        };
        let data = data.trim_start_matches("0x");
        if data.len() != 64 {
            continue;
        }
        let Ok(amount) = U256::from_str_radix(data, 16) else {
            continue;
        };
        if recipient_topic[24..].eq_ignore_ascii_case(&receiver) {
            total = total.checked_add(amount)?;
            matched = true;
        }
        if sender_topic[24..].eq_ignore_ascii_case(&receiver) {
            spent = spent.checked_add(amount)?;
        }
    }
    matched.then(|| total.saturating_sub(spent).to_string())
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

    #[test]
    fn attributes_erc20_transfer_log_to_receiver() {
        let token = "0x6b175474e89094c44da98b954eedeac495271d0f";
        let receiver = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1";
        let receipt = serde_json::json!({
            "logs": [{
                "address": token,
                "topics": [
                    TRANSFER_TOPIC,
                    format!("0x{}", "00".repeat(32)),
                    format!("0x{}{}", "00".repeat(12), receiver.trim_start_matches("0x")),
                ],
                "data": format!("0x{:064x}", U256::from(123_u64)),
            }]
        });
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), token, receiver).as_deref(),
            Some("123")
        );
    }

    #[test]
    fn outgoing_tokens_and_removed_logs_do_not_inflate_received_amount() {
        let token = "0x6b175474e89094c44da98b954eedeac495271d0f";
        let receiver = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1";
        let account = format!("0x000000000000000000000000{}", &receiver[2..]);
        let other = format!("0x{}", "00".repeat(32));
        let mut receipt = serde_json::json!({"logs":[
            {"address":token,"topics":[TRANSFER_TOPIC,other,account],"data":format!("0x{:064x}",100)},
            {"address":token,"topics":[TRANSFER_TOPIC,account,other],"data":format!("0x{:064x}",99)},
            {"removed":true,"address":token,"topics":[TRANSFER_TOPIC,other,account],"data":format!("0x{:064x}",200)}]});
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), token, receiver).as_deref(),
            Some("1")
        );
        receipt["logs"][1]["data"] = serde_json::json!(format!("0x{:064x}", 101));
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), token, receiver).as_deref(),
            Some("0")
        );
    }

    #[test]
    fn skips_malformed_logs_before_attributable_transfer() {
        let token = "0x6b175474e89094c44da98b954eedeac495271d0f";
        let receiver = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1";
        let receipt = serde_json::json!({
            "logs": [
                {"topics": "not-an-array"},
                {"address": token, "topics": [TRANSFER_TOPIC, null, null], "data": "0x00"},
                {
                    "address": token,
                    "topics": [
                        TRANSFER_TOPIC,
                        format!("0x{}", "00".repeat(32)),
                        format!("0x{}{}", "00".repeat(12), receiver.trim_start_matches("0x")),
                    ],
                    "data": format!("0x{:064x}", U256::from(123_u64)),
                }
            ]
        });
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), token, receiver).as_deref(),
            Some("123")
        );
    }
}
