//! Simulation of staged Enso transactions via `eth_call`.
//!
//! Runs a non-committal `eth_call` against the route's `to`/`data`/`value`
//! and, on revert, attempts to decode the standard `Error(string)` ABI.

use crate::runtime::Host;
use crate::session::Session;
use hex;

/// Simulate the route transaction for a session.
///
/// Returns a JSON value with the shape:
/// - success: `{ "success": true, "return_data": "0x…", "gas": Option<String> }`
/// - revert:  `{ "success": false, "decoded_error": { "message": "…" }, "gas": … }`
/// - other:   `{ "success": false, "error": "…", "gas": … }`
pub fn simulate_route<H: Host>(host: &mut H, sess: &Session) -> serde_json::Value {
    let route = match sess.route.as_ref() {
        Some(r) => r,
        None => {
            return serde_json::json!({
                "success": false,
                "error": "session has no route to simulate",
            });
        }
    };

    let to = format!("0x{:x}", route.tx.to);
    let data = format!("0x{}", hex::encode(&route.tx.data));
    let from = format!("0x{:x}", route.tx.from);
    let value = route.tx.value.to_string();

    let result = host.eth_call(&sess.chain, &to, &data, Some(&from), Some(&value));

    match result {
        Ok(res) if res.success => serde_json::json!({
            "success": true,
            "return_data": res.return_data,
            "gas": route.gas,
        }),
        Ok(res) => {
            // Try to decode Error(string): selector 0x08c379a0
            let message = decode_error_string(&res.return_data)
                .unwrap_or_else(|| res.return_data.clone());
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

/// Decode an ABI-encoded `Error(string)` revert payload.
///
/// Layout after the 4-byte selector:
///   32-byte offset (always 0x20 for a single string)
///   32-byte length
///   `length` bytes of UTF-8 data, padded to a 32-byte boundary.
fn decode_error_string(hex_data: &str) -> Option<String> {
    let hex = hex_data.strip_prefix("0x").or_else(|| hex_data.strip_prefix("0X"))?;
    if !hex_data.starts_with("0x08c379a0") && !hex_data.starts_with("0X08c379a0") {
        return None;
    }
    // Skip the 4-byte selector (8 hex chars).
    let body = &hex[8..];
    if body.len() < 128 {
        return None;
    }
    // Offset word (64 chars) — should be 0x20 for a single-string encoding.
    let _offset = u64::from_str_radix(&body[..64], 16).ok()?;
    let len = usize::from_str_radix(&body[64..128], 16).ok()?;
    let data_start = 128;
    let need = len * 2;
    if data_start + need > body.len() {
        return None;
    }
    let bytes = hex::decode(&body[data_start..data_start + need]).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_error_string() {
        // Error("Insufficient allowance") — selector + offset + length + data
        let msg = "Insufficient allowance";
        let msg_hex = hex::encode(msg.as_bytes());
        let len_hex = format!("{:064x}", msg.len());
        let payload = format!(
            "0x08c379a0\
             0000000000000000000000000000000000000000000000000000000000000020\
             {len_hex}\
             {msg_hex}\
             00000000000000000000000000000000000000000000000000000000000000",
        );
        assert_eq!(decode_error_string(&payload).unwrap(), msg);
    }

    #[test]
    fn returns_none_for_non_error_revert() {
        // Panic selector 0x4e487b71 (Panic(uint256)) — not Error(string)
        let payload = "0x4e487b710000000000000000000000000000000000000000000000000000000000000032";
        assert!(decode_error_string(payload).is_none());
    }
}
