//! Settlement verification via on-chain balance observations.
//!
//! After a swap is broadcast, the destination token balance of the receiver
//! should increase. This module compares a pre-stage baseline (stored in the
//! session) against the current on-chain balance.

use crate::api_types::{IERC20, NATIVE_TOKEN, parse_decimal_u256};
use crate::runtime::Host;
use crate::session::Session;
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::sol_types::SolEvent;

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
    let receiver_addr = req.receiver.unwrap_or(req.from_address);
    let receiver = format!("0x{:x}", receiver_addr);

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

    let current = match read_balance(host, dest_chain, req.token_out, &receiver) {
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

    let Some(before) = sess.observed_before.as_deref().and_then(parse_decimal_u256) else {
        return serde_json::json!({
            "status": "unverified_baseline",
            "error": "session has no trusted pre-route balance observation",
            "source_state": inspection.state,
            "source_tx_hash": inspection.tx_hash,
        });
    };

    let delta = current.saturating_sub(before);
    let Some(minimum) = sess
        .route
        .as_ref()
        .and_then(|route| parse_decimal_u256(route.amount_out.trim()))
    else {
        return serde_json::json!({
            "status": "error",
            "error": "Enso route has no valid quoted output floor",
        });
    };
    let balance_reached_quote = delta >= minimum;
    let same_chain = dest_chain.eq_ignore_ascii_case(&sess.chain);
    let receipt_transfer = if same_chain && req.token_out != NATIVE_TOKEN {
        inspection
            .receipt_json
            .as_deref()
            .and_then(|receipt| transfer_amount_to_receiver(receipt, req.token_out, receiver_addr))
    } else {
        None
    };
    let receipt_reached_quote = receipt_transfer.is_some_and(|amount| amount >= minimum);
    let status = if receipt_reached_quote {
        "destination_received"
    } else if balance_reached_quote {
        "destination_observed_unattributed"
    } else {
        "destination_below_quote"
    };

    serde_json::json!({
        "status": status,
        "source_state": inspection.state,
        "source_tx_hash": inspection.tx_hash,
        "route_outbox_id": route_outbox_id,
        "observed_before": before.to_string(),
        "observed_after": current.to_string(),
        "delta": delta.to_string(),
        "receipt_transfer_amount": receipt_transfer.map(|amount| amount.to_string()),
        "minimum_expected": minimum.to_string(),
        "destination_chain": dest_chain,
        "receiver": receiver,
        "token_out": token_out_hex,
    })
}

/// Sum the ERC-20 `Transfer` events from `token` to `receiver` in a receipt.
///
/// Logs are decoded with alloy's validating event decoder; anything that is
/// not a well-formed `Transfer` from the token contract is skipped.
fn transfer_amount_to_receiver(receipt: &str, token: Address, receiver: Address) -> Option<U256> {
    let receipt: serde_json::Value = serde_json::from_str(receipt).ok()?;
    let logs = receipt.get("logs")?.as_array()?;
    let mut total = U256::ZERO;
    let mut matched = false;
    for log in logs {
        let Some(transfer) = decode_transfer(log, token) else {
            continue;
        };
        if transfer.to != receiver {
            continue;
        }
        total = total.checked_add(transfer.value)?;
        matched = true;
    }
    matched.then_some(total)
}

fn decode_transfer(log: &serde_json::Value, token: Address) -> Option<IERC20::Transfer> {
    let address: Address = log.get("address")?.as_str()?.parse().ok()?;
    if address != token {
        return None;
    }
    let topics = log
        .get("topics")?
        .as_array()?
        .iter()
        .map(|topic| topic.as_str()?.parse::<B256>().ok())
        .collect::<Option<Vec<B256>>>()?;
    let data: Bytes = log.get("data")?.as_str()?.parse().ok()?;
    let transfer = IERC20::Transfer::decode_raw_log_validate(topics.iter().copied(), &data).ok()?;
    // Indexed addresses must be canonical words, not dirty high bytes that
    // happen to end in the receiver's address.
    (topics[2] == transfer.to.into_word() && data.len() == 32).then_some(transfer)
}

/// Observe the pre-stage balance of the output token for the receiver.
///
/// Called during `create` so the session captures a baseline.
pub fn observe_balance_before<H: Host>(
    host: &mut H,
    dest_chain: &str,
    token_out: Address,
    receiver_hex: &str,
) -> Result<String, String> {
    read_balance(host, dest_chain, token_out, receiver_hex).map(|value| value.to_string())
}

fn read_balance<H: Host>(
    host: &mut H,
    chain: &str,
    token: Address,
    receiver: &str,
) -> Result<U256, String> {
    if token == NATIVE_TOKEN {
        host.eth_balance(chain, receiver)
    } else {
        host.erc20_balance(chain, &format!("0x{token:x}"), receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const TOKEN: Address = address!("0x6b175474e89094c44da98b954eedeac495271d0f");
    const RECEIVER: Address = address!("0x742d35cc6634c0532925a3b844bc9e7595f0beb1");

    fn transfer_log(recipient_topic: String, data: String) -> serde_json::Value {
        serde_json::json!({
            "address": format!("0x{TOKEN:x}"),
            "topics": [
                IERC20::Transfer::SIGNATURE_HASH,
                format!("0x{}", "00".repeat(32)),
                recipient_topic,
            ],
            "data": data,
        })
    }

    fn amount_word(value: u64) -> String {
        format!("0x{:064x}", U256::from(value))
    }

    #[test]
    fn attributes_erc20_transfer_log_to_receiver() {
        let receipt = serde_json::json!({
            "logs": [transfer_log(RECEIVER.into_word().to_string(), amount_word(123))]
        });
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), TOKEN, RECEIVER),
            Some(U256::from(123))
        );
    }

    #[test]
    fn skips_malformed_logs_before_attributable_transfer() {
        let receipt = serde_json::json!({
            "logs": [
                {"topics": "not-an-array"},
                {"address": TOKEN, "topics": [IERC20::Transfer::SIGNATURE_HASH, null, null], "data": "0x00"},
                transfer_log(RECEIVER.into_word().to_string(), amount_word(123)),
            ]
        });
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), TOKEN, RECEIVER),
            Some(U256::from(123))
        );
    }

    #[test]
    fn rejects_dirty_recipient_topic_and_bad_data_length() {
        let dirty = format!("0x{}{}", "ff".repeat(12), hex_address(RECEIVER));
        let receipt = serde_json::json!({
            "logs": [
                transfer_log(dirty, amount_word(123)),
                transfer_log(RECEIVER.into_word().to_string(), format!("{}00", amount_word(1))),
                transfer_log(RECEIVER.into_word().to_string(), "0x01".into()),
            ]
        });
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), TOKEN, RECEIVER),
            None
        );
    }

    #[test]
    fn ignores_transfers_from_other_tokens_and_to_other_receivers() {
        let other = address!("0x0000000000000000000000000000000000000001");
        let mut other_token = transfer_log(RECEIVER.into_word().to_string(), amount_word(5));
        other_token["address"] = format!("0x{other:x}").into();
        let receipt = serde_json::json!({
            "logs": [
                other_token,
                transfer_log(other.into_word().to_string(), amount_word(7)),
                transfer_log(RECEIVER.into_word().to_string(), amount_word(2)),
                transfer_log(RECEIVER.into_word().to_string(), amount_word(3)),
            ]
        });
        assert_eq!(
            transfer_amount_to_receiver(&receipt.to_string(), TOKEN, RECEIVER),
            Some(U256::from(5))
        );
    }

    fn hex_address(address: Address) -> String {
        format!("{address:x}")
    }
}
