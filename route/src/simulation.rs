//! Simulation of staged Enso transactions via `eth_call`.
//!
//! Runs a non-committal `eth_call` against the route's `to`/`data`/`value`
//! and, on revert, decodes the standard `Error(string)` / `Panic(uint256)` ABI.

use crate::api_types::RouteResponse;
use crate::runtime::Host;
use crate::session::Session;
use alloy::primitives::{Bytes, hex};

/// Simulate a route response directly (used by `create()` before the session
/// is persisted, and by `simulate_route()` for stored sessions).
pub fn simulate_route_response<H: Host>(
    host: &mut H,
    chain: &str,
    route: &RouteResponse,
) -> serde_json::Value {
    let to = format!("0x{:x}", route.tx.to);
    let data = hex::encode_prefixed(&route.tx.data);
    let from = format!("0x{:x}", route.tx.from);

    match host.eth_call(chain, &to, &data, Some(&from), Some(route.tx.value)) {
        Ok(res) if res.success => serde_json::json!({
            "success": true,
            "return_data": res.return_data,
            "gas": route.gas,
        }),
        Ok(res) => {
            let message =
                decode_revert_message(&res.return_data).unwrap_or_else(|| res.return_data.clone());
            serde_json::json!({
                "success": false,
                "decoded_error": { "message": message },
                "gas": route.gas,
            })
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error": e,
            "gas": route.gas,
        }),
    }
}

/// Simulate the route transaction for a stored session.
pub fn simulate_route<H: Host>(host: &mut H, sess: &Session) -> serde_json::Value {
    match sess.route.as_ref() {
        Some(route) => simulate_route_response(host, &sess.chain, route),
        None => serde_json::json!({
            "success": false,
            "error": "session has no route to simulate",
        }),
    }
}

pub fn failure_message(result: &serde_json::Value) -> String {
    result
        .get("decoded_error")
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
        .or_else(|| result.get("error").and_then(|value| value.as_str()))
        .or_else(|| result.get("message").and_then(|value| value.as_str()))
        .unwrap_or("simulation returned an unsuccessful result")
        .chars()
        .take(256)
        .collect()
}

/// Decode a revert payload (`Error(string)`, `Panic(uint256)`, or a raw
/// UTF-8 reason) with alloy's validating decoder.
pub fn decode_revert_message(hex_data: &str) -> Option<String> {
    let bytes: Bytes = hex_data.parse().ok()?;
    alloy::sol_types::decode_revert_reason(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{Panic, Revert, SolError};

    #[test]
    fn decodes_error_string() {
        let payload = hex::encode_prefixed(Revert::from("Insufficient allowance").abi_encode());
        assert_eq!(
            decode_revert_message(&payload).unwrap(),
            "revert: Insufficient allowance"
        );
    }

    #[test]
    fn decodes_panic_code() {
        let payload = hex::encode_prefixed(Panic::from(0x11).abi_encode());
        assert!(
            decode_revert_message(&payload)
                .unwrap()
                .contains("overflow")
        );
    }

    #[test]
    fn rejects_truncated_error_string() {
        let mut encoded = Revert::from("Insufficient allowance").abi_encode();
        encoded.truncate(80);
        assert!(decode_revert_message(&hex::encode_prefixed(encoded)).is_none());
        assert!(decode_revert_message("0x08c379a0").is_none());
    }
}
