//! Enso quote freshness.
//!
//! Enso embeds its quote time in the route calldata. Bloom's outbox refuses an
//! Enso transaction whose quote is older than [`BLOOM_QUOTE_MAX_AGE_SECS`] on
//! every confirm, including the one that follows the owner's approval
//! ceremony. A route quoted at `new` and staged minutes later (for example
//! after a policy ceremony) is therefore unconfirmable. Staging refreshes an
//! aged quote so that window starts at staging, and never accepts a refreshed
//! route outside the bounds the owner reviewed.

use crate::api_types::{RouteRequest, RouteResponse};
use alloy::primitives::U256;

/// Bloom's outbox limit for an Enso quote's age at confirm time.
pub const BLOOM_QUOTE_MAX_AGE_SECS: u64 = 300;

/// Staging keeps a quote this fresh and refreshes anything older.
pub const REFRESH_AFTER_SECS: u64 = 30;

const MARKER: &[u8] = b"{\"Source\":\"Enso";

/// Quote time in Unix seconds, read from the Enso metadata in the calldata
/// the same way Bloom's outbox reads it. `None` when the calldata carries no
/// Enso quote metadata.
pub fn quoted_at_secs(calldata: &[u8]) -> Option<u64> {
    let start = calldata
        .windows(MARKER.len())
        .position(|window| window == MARKER)?;
    let value: serde_json::Value = serde_json::Deserializer::from_slice(&calldata[start..])
        .into_iter()
        .next()?
        .ok()?;
    value.get("Timestamp")?.as_u64()
}

/// Whether staging should replace this route's quote before staging it.
pub fn needs_refresh(route: &RouteResponse, now_ms: u64) -> bool {
    quoted_at_secs(&route.tx.data)
        .is_some_and(|quoted| (now_ms / 1000).saturating_sub(quoted) > REFRESH_AFTER_SECS)
}

/// The smallest output the owner accepted when reviewing the plan: the
/// reviewed quote less the reviewed slippage tolerance.
pub fn reviewed_minimum_output(
    reviewed: &RouteResponse,
    req: &RouteRequest,
) -> Result<U256, String> {
    let quoted = U256::from_str_radix(reviewed.amount_out.trim(), 10)
        .map_err(|_| "reviewed route has an invalid quoted output amount".to_string())?;
    let kept = U256::from(10_000u64.saturating_sub(u64::from(req.slippage_bps)));
    Ok(quoted * kept / U256::from(10_000u64))
}

/// The minimum output Enso reports for a route, when it reports exactly one.
fn reported_minimum_output(route: &RouteResponse) -> Option<U256> {
    let value = match route.min_amount_out.as_ref()? {
        serde_json::Value::Array(items) if items.len() == 1 => &items[0],
        other => other,
    };
    U256::from_str_radix(value.as_str()?.trim(), 10).ok()
}

/// Accept a fresh quote only within the reviewed bounds: the same request,
/// router and native value, and a transaction whose own minimum output is
/// no lower than the reviewed quote less its slippage tolerance.
///
/// The refresh asks Enso for exactly that floor (`minAmountOut`), so the
/// minimum Enso reports must reach it. Without a reported minimum, the fresh
/// quote less the reviewed slippage must reach it instead, which is the
/// floor a slippage-based route would set.
pub fn verify_refreshed(
    reviewed: &RouteResponse,
    fresh: &RouteResponse,
    req: &RouteRequest,
) -> Result<(), String> {
    if !fresh.input_matches_request(req) || !fresh.destination_matches_request(req) {
        return Err("the refreshed Enso route no longer matches the requested swap".into());
    }
    if fresh.tx.to != reviewed.tx.to {
        return Err(format!(
            "the refreshed Enso route uses router 0x{:x} instead of the reviewed 0x{:x}",
            fresh.tx.to, reviewed.tx.to
        ));
    }
    if fresh.tx.value != reviewed.tx.value {
        return Err("the refreshed Enso route changes the native value sent".into());
    }
    let fresh_output = U256::from_str_radix(fresh.amount_out.trim(), 10)
        .map_err(|_| "the refreshed Enso route has an invalid quoted output amount".to_string())?;
    let minimum = reviewed_minimum_output(reviewed, req)?;
    if fresh_output.is_zero() || fresh_output < minimum {
        return Err(format!(
            "the refreshed Enso quote ({fresh_output}) is below the reviewed minimum output ({minimum})"
        ));
    }
    let fresh_minimum = match reported_minimum_output(fresh) {
        Some(reported) => reported,
        None => {
            let kept = U256::from(10_000u64.saturating_sub(u64::from(req.slippage_bps)));
            fresh_output * kept / U256::from(10_000u64)
        }
    };
    if fresh_minimum < minimum {
        return Err(format!(
            "the refreshed Enso route's minimum output ({fresh_minimum}) is below the reviewed minimum output ({minimum})"
        ));
    }
    Ok(())
}

/// Quote timing for `status.json`, or `None` without Enso quote metadata.
pub fn status_view(route: &RouteResponse, now_ms: u64) -> Option<serde_json::Value> {
    let quoted_at_ms = quoted_at_secs(&route.tx.data)?.saturating_mul(1000);
    let expires_at_ms = quoted_at_ms.saturating_add(BLOOM_QUOTE_MAX_AGE_SECS * 1000);
    Some(serde_json::json!({
        "quoted_at_ms": quoted_at_ms,
        "expires_at_ms": expires_at_ms,
        "expired": now_ms >= expires_at_ms,
    }))
}

/// What an agent must do when a staged, unbroadcast route's quote expired.
/// Bloom refuses to confirm it, so retrying cannot help.
pub fn expired_staged_error(outbox_id: &str) -> String {
    format!(
        "the Enso quote in outbox {outbox_id} expired; while that entry is pending, Bloom \
         refuses to confirm it, so cancel it and create a new intent for a fresh quote"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calldata(timestamp: u64) -> Vec<u8> {
        let mut data = vec![0xb9, 0x4c, 0x36, 0x09, 0, 0, 0];
        data.extend_from_slice(
            format!(r#"{{"Source":"Enso-ef06c8a82128","AmountOut":"1","Timestamp":{timestamp}}}"#)
                .as_bytes(),
        );
        data.extend_from_slice(&[0; 16]);
        data
    }

    #[test]
    fn reads_the_enso_quote_time_embedded_in_calldata() {
        assert_eq!(
            quoted_at_secs(&calldata(1_790_343_031)),
            Some(1_790_343_031)
        );
        assert_eq!(quoted_at_secs(b"no enso metadata"), None);
        assert_eq!(
            quoted_at_secs(br#"{"Source":"Enso","Timestamp":"x"}"#),
            None
        );
    }

    #[test]
    fn status_marks_a_quote_expired_after_bloom_limit() {
        let mut route: RouteResponse = serde_json::from_value(serde_json::json!({
            "tx": {
                "to": "0x1234567890abcdef1234567890abcdef12345678",
                "data": format!("0x{}", hex::encode(calldata(1_000))),
                "value": "0",
                "from": "0x742d35cc6634c0532925a3b844bc9e7595f0beb1",
            },
            "amountOut": "1",
            "route": [],
        }))
        .unwrap();
        let view = status_view(&route, 1_299_999).unwrap();
        assert_eq!(view["quoted_at_ms"], 1_000_000);
        assert_eq!(view["expires_at_ms"], 1_300_000);
        assert_eq!(view["expired"], false);
        assert_eq!(status_view(&route, 1_300_000).unwrap()["expired"], true);
        assert!(!needs_refresh(&route, 1_030_000));
        assert!(needs_refresh(&route, 1_031_000));
        route.tx.data = b"no metadata".to_vec().into();
        assert!(status_view(&route, 1_300_000).is_none());
        assert!(!needs_refresh(&route, 9_999_999));
    }
}
