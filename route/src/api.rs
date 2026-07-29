//! Enso Shortcuts API client.
//!
//! Synchronous wrapper over `petal::sdk::http_fetch` for the Enso route
//! endpoint.

use crate::api_types::*;
use crate::runtime::Host;
use petal::sdk::{HttpRequest, HttpResponse};

const ROUTE_BASE: &str = "https://api.enso.finance";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

fn build_get_url(base: &str, path: &str, query: &[(&str, String)]) -> String {
    let mut url = format!("{base}{path}?");
    for (i, (k, v)) in query.iter().enumerate() {
        if i > 0 {
            url.push('&');
        }
        url.push_str(k);
        url.push('=');
        for b in v.bytes() {
            match b {
                b'-' | b'.' | b'_' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' => {
                    url.push(b as char);
                }
                _ => {
                    url.push('%');
                    url.push_str(&format!("{b:02X}"));
                }
            }
        }
    }
    url
}

fn auth_headers(api_key: &str) -> Vec<(String, String)> {
    if api_key.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization".into(), format!("Bearer {api_key}"))]
    }
}

fn check_status(resp: &HttpResponse, context: &str) -> Result<(), String> {
    if resp.status >= 200 && resp.status < 300 {
        Ok(())
    } else {
        let body = String::from_utf8_lossy(&resp.body);
        Err(format!(
            "Enso {context} returned status {}: {}",
            resp.status,
            body.chars().take(512).collect::<String>()
        ))
    }
}

/// `GET /api/v1/shortcuts/route` — single-step route discovery.
pub fn route<H: Host>(
    host: &mut H,
    api_key: &str,
    req: &RouteRequest,
) -> Result<RouteResponse, String> {
    let query = req.to_query();
    let query_refs: Vec<(&str, String)> = query.iter().map(|(k, v)| (*k, v.clone())).collect();
    let url = build_get_url(ROUTE_BASE, "/api/v1/shortcuts/route", &query_refs);

    let http_req = HttpRequest {
        method: "GET".into(),
        url,
        headers: auth_headers(api_key),
        body: Vec::new(),
    };

    let resp = host.http(http_req, MAX_RESPONSE)?;
    check_status(&resp, "route")?;

    let mut v: RouteResponse = serde_json::from_slice(&resp.body)
        .map_err(|e| format!("failed to parse Enso route response: {e}"))?;

    // Extract destination_chain_id from the first bridging hop.
    if let Some(hops) = v.route.as_array() {
        v.destination_chain_id = hops
            .iter()
            .find_map(|hop| hop.get("destinationChainId")?.as_u64());
    }

    Ok(v)
}
