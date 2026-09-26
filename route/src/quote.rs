//! Enso quote freshness.
//!
//! A quote's lifetime comes from Enso's route response: `validUntil` (Unix
//! seconds) when Enso sends it. Every route also records when this Petal
//! fetched it, and a quote without `validUntil` is treated as valid for
//! [`DEFAULT_QUOTE_LIFETIME_SECS`] after that. Nothing here reads venue data
//! inside the transaction calldata.
//!
//! Approval can take minutes after staging, so staging refreshes a quote
//! older than [`REFRESH_AFTER_SECS`] and never accepts a refreshed route
//! outside the bounds the owner reviewed.

use crate::api_types::{RouteRequest, RouteResponse};
use alloy::primitives::U256;

/// Lifetime assumed for a quote whose response has no `validUntil`.
pub const DEFAULT_QUOTE_LIFETIME_SECS: u64 = 300;

/// Staging keeps a quote this fresh and refreshes anything older.
pub const REFRESH_AFTER_SECS: u64 = 30;

/// Enso's `validUntil` in Unix seconds, when the response carries one.
fn valid_until_secs(route: &RouteResponse) -> Option<u64> {
    let value = route.valid_until.as_ref()?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// When the quote expires, in Unix milliseconds, and where that came from:
/// Enso's `validUntil`, or the fetch time plus the default lifetime. `None`
/// for a route with neither (stored before fetch times were recorded).
pub fn expires_at_ms(route: &RouteResponse) -> Option<(u64, &'static str)> {
    if let Some(valid_until) = valid_until_secs(route) {
        return Some((valid_until.saturating_mul(1000), "enso_valid_until"));
    }
    route.fetched_at_ms.map(|fetched| {
        (
            fetched.saturating_add(DEFAULT_QUOTE_LIFETIME_SECS * 1000),
            "fetch_time",
        )
    })
}

/// Whether staging should replace this route's quote before staging it:
/// it is older than the refresh threshold, or already expired.
pub fn needs_refresh(route: &RouteResponse, now_ms: u64) -> bool {
    let aged = route
        .fetched_at_ms
        .is_some_and(|fetched| now_ms.saturating_sub(fetched) > REFRESH_AFTER_SECS * 1000);
    let expired = expires_at_ms(route).is_some_and(|(expires, _)| now_ms >= expires);
    aged || expired
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

/// Quote timing for `status.json`, or `None` when the route has no timing.
pub fn status_view(route: &RouteResponse, now_ms: u64) -> Option<serde_json::Value> {
    let (expires_at_ms, source) = expires_at_ms(route)?;
    Some(serde_json::json!({
        "fetched_at_ms": route.fetched_at_ms,
        "expires_at_ms": expires_at_ms,
        "expires_from": source,
        "expired": now_ms >= expires_at_ms,
    }))
}

/// What an agent must do when a staged, unbroadcast route's quote expired.
pub fn expired_staged_error(outbox_id: &str) -> String {
    format!(
        "the Enso quote in outbox {outbox_id} expired; confirming that pending entry can \
         revert or be refused, so cancel it and create a new intent for a fresh quote"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(valid_until: Option<serde_json::Value>, fetched_at_ms: Option<u64>) -> RouteResponse {
        let mut body = serde_json::json!({
            "tx": {
                "to": "0x1234567890abcdef1234567890abcdef12345678",
                "data": "0x00",
                "value": "0",
                "from": "0x742d35cc6634c0532925a3b844bc9e7595f0beb1",
            },
            "amountOut": "1",
            "route": [],
        });
        if let Some(valid_until) = valid_until {
            body["validUntil"] = valid_until;
        }
        let mut route: RouteResponse = serde_json::from_value(body).unwrap();
        route.fetched_at_ms = fetched_at_ms;
        route
    }

    #[test]
    fn enso_valid_until_sets_the_expiry() {
        for valid_until in [serde_json::json!(1_200), serde_json::json!("1200")] {
            let quoted = route(Some(valid_until), Some(1_000_000));
            assert_eq!(
                expires_at_ms(&quoted),
                Some((1_200_000, "enso_valid_until"))
            );
            let view = status_view(&quoted, 1_199_999).unwrap();
            assert_eq!(view["expires_from"], "enso_valid_until");
            assert_eq!(view["expired"], false);
            assert_eq!(status_view(&quoted, 1_200_000).unwrap()["expired"], true);
        }
    }

    #[test]
    fn fetch_time_sets_the_expiry_without_valid_until() {
        let quoted = route(None, Some(1_000_000));
        assert_eq!(expires_at_ms(&quoted), Some((1_300_000, "fetch_time")));
        assert!(!needs_refresh(&quoted, 1_030_000));
        assert!(needs_refresh(&quoted, 1_030_001));
        assert!(status_view(&route(None, None), 9_999_999).is_none());
        assert!(!needs_refresh(&route(None, None), 9_999_999));
    }

    #[test]
    fn an_expired_quote_is_refreshed_even_when_young() {
        // Enso may grant less than the refresh threshold.
        let quoted = route(Some(serde_json::json!(1_010)), Some(1_000_000));
        assert!(!needs_refresh(&quoted, 1_009_999));
        assert!(needs_refresh(&quoted, 1_010_000));
    }
}
