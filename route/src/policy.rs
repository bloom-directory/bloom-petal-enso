//! Petal-owned Enso venue preferences and fail-closed route evaluation.
//!
//! Bloom's Broker/Signer-authoritative wallet policy and approval limits are
//! enforced independently by the host. Enso-specific preferences live in the
//! Petal store; this guest must not interpret authoritative wallet policy.

use std::collections::BTreeSet;

use alloy::primitives::U256;
use serde::Deserialize;

use crate::runtime::Host;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefiPolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_source_chains: BTreeSet<String>,
    #[serde(default)]
    pub allowed_destination_chains: BTreeSet<String>,
    #[serde(default)]
    pub allowed_receivers: BTreeSet<String>,
    #[serde(default)]
    pub denied_receivers: BTreeSet<String>,
    #[serde(default)]
    pub allowed_routers: BTreeSet<String>,
    #[serde(default)]
    pub denied_protocols: BTreeSet<String>,
    #[serde(default)]
    pub allow_unknown_protocols: bool,
    #[serde(default = "default_true")]
    pub require_calldata_verification: bool,
    /// The Petal has no trusted USD oracle. If this field is configured, the
    /// evaluator refuses rather than pretending to enforce it.
    #[serde(default)]
    pub max_input_usd: Option<toml::Value>,
    #[serde(default)]
    pub max_native_value_wei: Option<String>,
}

impl Default for DefiPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_source_chains: BTreeSet::new(),
            allowed_destination_chains: BTreeSet::new(),
            allowed_receivers: BTreeSet::new(),
            denied_receivers: BTreeSet::new(),
            allowed_routers: BTreeSet::new(),
            denied_protocols: BTreeSet::new(),
            allow_unknown_protocols: false,
            require_calldata_verification: true,
            max_input_usd: None,
            max_native_value_wei: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MevPolicy {
    #[serde(default)]
    max_slippage_bps: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct EnsoVenueConfig {
    #[serde(default)]
    defi: DefiPolicy,
    #[serde(default)]
    mev: MevPolicy,
}

#[derive(Debug)]
pub struct VerifiedPolicy {
    pub defi: DefiPolicy,
    pub max_slippage_bps: Option<u16>,
}

#[derive(Debug)]
pub struct RoutePolicyContext<'a> {
    pub source_chain: &'a str,
    pub destination_chain: &'a str,
    pub cross_chain: bool,
    pub receiver: &'a str,
    pub token_out: &'a str,
    pub receiver_class: &'a str,
    pub router: &'a str,
    pub protocols: &'a [String],
    pub protocols_unknown: bool,
    pub native_value_wei: U256,
    pub slippage_bps: u16,
    pub route_verified: bool,
    pub receiver_verified: bool,
    pub min_out_enforced: bool,
    pub needs_approve: bool,
}

fn check(rule: &str, outcome: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "venue": "defi",
        "rule": format!("defi.{rule}"),
        "outcome": outcome,
        "message": message.into(),
    })
}

fn set_contains_case_insensitive(set: &BTreeSet<String>, needle: &str) -> bool {
    set.iter().any(|value| value.eq_ignore_ascii_case(needle))
}

fn receiver_matches(set: &BTreeSet<String>, ctx: &RoutePolicyContext<'_>) -> bool {
    let literal = format!(
        "{}:{}:{}",
        ctx.destination_chain.to_ascii_lowercase(),
        ctx.token_out.to_ascii_lowercase(),
        ctx.receiver.to_ascii_lowercase()
    );
    let class = format!("class:{}", ctx.receiver_class.to_ascii_lowercase());
    set_contains_case_insensitive(set, &literal) || set_contains_case_insensitive(set, &class)
}

const MAX_VENUE_CONFIG_BYTES: usize = 256 * 1024;

fn venue_config_key(wallet: &str) -> String {
    format!("settings/wallets/{wallet}/venue.toml")
}

/// The effective initial configuration is shared by the read surface and the
/// workflow. Explicit user configuration retains conservative field defaults.
const DEFAULT_VENUE_CONFIG: &[u8] = include_bytes!("venue-defaults.toml");

pub fn load_venue_config<H: Host>(host: &mut H, wallet: &str) -> Result<VerifiedPolicy, String> {
    let bytes = read_venue_config(host, wallet)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "Enso venue configuration is not UTF-8")?;
    let policy = toml::from_str::<EnsoVenueConfig>(text)
        .map_err(|e| format!("Enso venue configuration is invalid: {e}"))?;
    Ok(VerifiedPolicy {
        defi: policy.defi,
        max_slippage_bps: policy.mev.max_slippage_bps,
    })
}

pub fn read_venue_config<H: Host>(host: &mut H, wallet: &str) -> Result<Vec<u8>, String> {
    petal::validate_wallet_id(wallet)?;
    if let Some(bytes) = host.get(&venue_config_key(wallet), MAX_VENUE_CONFIG_BYTES)? {
        return Ok(bytes);
    }
    // Preserve settings written by PR 7 before the route namespace move,
    // including explicit opt-outs. Writes always use the new key.
    Ok(host
        .get(
            &format!("settings/{wallet}/venue.toml"),
            MAX_VENUE_CONFIG_BYTES,
        )?
        .unwrap_or_else(|| DEFAULT_VENUE_CONFIG.to_vec()))
}

pub fn write_venue_config<H: Host>(host: &mut H, wallet: &str, body: &[u8]) -> Result<(), String> {
    petal::validate_wallet_id(wallet)?;
    if body.len() > MAX_VENUE_CONFIG_BYTES {
        return Err("Enso venue configuration exceeds 256 KiB".into());
    }
    let text = std::str::from_utf8(body).map_err(|_| "Enso venue configuration must be UTF-8")?;
    toml::from_str::<EnsoVenueConfig>(text)
        .map_err(|e| format!("Enso venue configuration is invalid: {e}"))?;
    host.put(&venue_config_key(wallet), body, false)
}

pub fn evaluate(policy: &VerifiedPolicy, ctx: &RoutePolicyContext<'_>) -> serde_json::Value {
    let mut out = Vec::new();
    let defi = &policy.defi;

    out.push(check(
        "route_verified",
        if ctx.route_verified { "pass" } else { "deny" },
        if ctx.route_verified {
            "Enso route source asset, amount, sender, and native value match the request"
        } else {
            "Enso route transaction does not match the request"
        },
    ));

    out.push(check(
        "enabled",
        if defi.enabled { "pass" } else { "deny" },
        if defi.enabled {
            "DeFi routes enabled"
        } else {
            "generic DeFi routes are disabled for this wallet"
        },
    ));

    let source_allowed = set_contains_case_insensitive(
        &defi.allowed_source_chains,
        &ctx.source_chain.to_ascii_lowercase(),
    );
    out.push(check(
        "source_chain",
        if source_allowed { "pass" } else { "deny" },
        if source_allowed {
            format!("source chain {} allowed", ctx.source_chain)
        } else {
            format!("source chain {} is not allowlisted", ctx.source_chain)
        },
    ));

    if ctx.cross_chain {
        let destination_allowed = set_contains_case_insensitive(
            &defi.allowed_destination_chains,
            &ctx.destination_chain.to_ascii_lowercase(),
        );
        out.push(check(
            "destination_chain",
            if destination_allowed { "pass" } else { "deny" },
            if destination_allowed {
                format!("destination chain {} allowed", ctx.destination_chain)
            } else {
                format!(
                    "destination chain {} is not allowlisted",
                    ctx.destination_chain
                )
            },
        ));
    }

    let receiver_literal = format!(
        "{}:{}:{}",
        ctx.destination_chain.to_ascii_lowercase(),
        ctx.token_out.to_ascii_lowercase(),
        ctx.receiver.to_ascii_lowercase()
    );
    let receiver_denied = receiver_matches(&defi.denied_receivers, ctx);
    let receiver_allowed = receiver_matches(&defi.allowed_receivers, ctx);
    let (receiver_outcome, receiver_message) = if receiver_denied {
        (
            "deny",
            format!("receiver {} is explicitly denylisted", ctx.receiver),
        )
    } else if receiver_allowed {
        (
            "pass",
            format!(
                "receiver {} permitted as {}",
                ctx.receiver, ctx.receiver_class
            ),
        )
    } else {
        (
            "deny",
            format!(
                "receiver is not allowlisted; expected literal {receiver_literal} or class:{}",
                ctx.receiver_class
            ),
        )
    };
    out.push(check("receiver", receiver_outcome, receiver_message));

    if !ctx.receiver_verified {
        out.push(check(
            "receiver_verified",
            if ctx.cross_chain || !defi.require_calldata_verification {
                "warn"
            } else {
                "deny"
            },
            if ctx.cross_chain {
                "cross-chain receiver is request-bound but cannot be proven until settlement"
            } else if defi.require_calldata_verification {
                "route receiver is not cryptographically proven by decoded calldata"
            } else {
                "route receiver is request-bound but not calldata-verified; Enso venue preferences accept this residual risk"
            },
        ));
    }

    if !ctx.min_out_enforced {
        out.push(check(
            "min_output",
            if defi.require_calldata_verification {
                "deny"
            } else {
                "warn"
            },
            if defi.require_calldata_verification {
                "no decoded minimum-output floor is enforced"
            } else {
                "minimum output is Enso-quoted, not calldata-verified; Enso venue preferences accept this residual risk"
            },
        ));
    } else {
        out.push(check(
            "min_output",
            "pass",
            "minimum-output floor is enforced",
        ));
    }

    let router_key = format!(
        "{}:{}",
        ctx.source_chain.to_ascii_lowercase(),
        ctx.router.to_ascii_lowercase()
    );
    let router_allowed = set_contains_case_insensitive(&defi.allowed_routers, &router_key);
    out.push(check(
        "router",
        if router_allowed { "pass" } else { "deny" },
        if router_allowed {
            format!("router {} allowlisted", ctx.router)
        } else {
            format!("router {} is not allowlisted as {router_key}", ctx.router)
        },
    ));

    if ctx.protocols_unknown {
        out.push(check(
            "protocols",
            if defi.allow_unknown_protocols {
                "warn"
            } else {
                "deny"
            },
            if defi.allow_unknown_protocols {
                "route protocol metadata is unknown; Enso venue preferences permit this with a warning"
            } else {
                "route protocol metadata is unknown and Enso venue preferences refuse it"
            },
        ));
    } else if let Some(protocol) = ctx
        .protocols
        .iter()
        .find(|p| set_contains_case_insensitive(&defi.denied_protocols, p))
    {
        out.push(check(
            "protocols",
            "deny",
            format!("route uses denied protocol {protocol}"),
        ));
    } else {
        out.push(check(
            "protocols",
            "pass",
            if ctx.protocols.is_empty() {
                "no protocols reported".to_string()
            } else {
                format!("protocols permitted: {}", ctx.protocols.join(" -> "))
            },
        ));
    }

    if let Some(max_slippage_bps) = policy.max_slippage_bps {
        out.push(check(
            "max_slippage",
            if ctx.slippage_bps <= max_slippage_bps {
                "pass"
            } else {
                "deny"
            },
            format!(
                "requested slippage {} bps; wallet maximum {} bps",
                ctx.slippage_bps, max_slippage_bps
            ),
        ));
    }

    if defi.max_input_usd.is_some() {
        out.push(check(
            "max_input_usd",
            "deny",
            "max_input_usd is configured but this Petal has no trusted USD valuation oracle",
        ));
    }

    if let Some(cap) = &defi.max_native_value_wei {
        match U256::from_str_radix(cap.trim(), 10) {
            Ok(cap) => out.push(check(
                "max_native_value",
                if ctx.native_value_wei <= cap {
                    "pass"
                } else {
                    "deny"
                },
                format!(
                    "route attaches {} wei; wallet maximum is {} wei",
                    ctx.native_value_wei, cap
                ),
            )),
            Err(_) => out.push(check(
                "max_native_value",
                "deny",
                "max_native_value_wei is not a valid decimal integer",
            )),
        }
    }

    if ctx.needs_approve {
        out.push(check(
            "erc20_approval",
            "warn",
            "an exact-amount ERC-20 approval must succeed before the route can be staged",
        ));
    }

    serde_json::Value::Array(out)
}

pub fn deny_reason(checks: &serde_json::Value) -> Option<String> {
    checks.as_array()?.iter().find_map(|check| {
        (check.get("outcome").and_then(|v| v.as_str()) == Some("deny")).then(|| {
            let rule = check.get("rule").and_then(|v| v.as_str()).unwrap_or("defi");
            let message = check
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("policy denied");
            format!("policy denied [{rule}]: {message}")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> VerifiedPolicy {
        VerifiedPolicy {
            defi: DefiPolicy {
                enabled: true,
                allowed_source_chains: ["base".into()].into_iter().collect(),
                allowed_destination_chains: ["arbitrum".into()].into_iter().collect(),
                allowed_receivers: [
                    "base:0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee:0x1111111111111111111111111111111111111111"
                        .into(),
                ]
                .into_iter()
                .collect(),
                allowed_routers: [
                    "base:0xf75584ef6673ad213a685a1b58cc0330b8ea22cf".into(),
                ]
                .into_iter()
                .collect(),
                require_calldata_verification: false,
                max_native_value_wei: Some("100".into()),
                ..DefiPolicy::default()
            },
            max_slippage_bps: Some(100),
        }
    }

    fn context<'a>(protocols: &'a [String]) -> RoutePolicyContext<'a> {
        RoutePolicyContext {
            source_chain: "base",
            destination_chain: "base",
            cross_chain: false,
            receiver: "0x1111111111111111111111111111111111111111",
            token_out: "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            receiver_class: "wallet_eoa",
            router: "0xf75584ef6673ad213a685a1b58cc0330b8ea22cf",
            protocols,
            protocols_unknown: false,
            native_value_wei: U256::from(10),
            slippage_bps: 50,
            route_verified: true,
            receiver_verified: false,
            min_out_enforced: false,
            needs_approve: false,
        }
    }

    #[test]
    fn allowed_route_has_warnings_but_no_denies() {
        let protocols = vec!["enso".into()];
        let checks = evaluate(&policy(), &context(&protocols));
        assert!(deny_reason(&checks).is_none(), "{checks:#}");
    }

    #[test]
    fn source_chain_router_receiver_and_slippage_fail_closed() {
        let protocols = vec!["enso".into()];
        let mut ctx = context(&protocols);
        ctx.source_chain = "ethereum";
        ctx.slippage_bps = 101;
        let checks = evaluate(&policy(), &ctx);
        assert!(deny_reason(&checks).is_some());
    }

    #[test]
    fn bundled_defaults_allow_supported_routes_and_keep_boundaries() {
        let defaults: EnsoVenueConfig =
            toml::from_str(std::str::from_utf8(DEFAULT_VENUE_CONFIG).unwrap()).unwrap();
        let policy = VerifiedPolicy {
            defi: defaults.defi,
            max_slippage_bps: defaults.mev.max_slippage_bps,
        };
        let mut ctx = context(&[]);
        ctx.protocols_unknown = true;
        for source in &policy.defi.allowed_source_chains {
            for destination in &policy.defi.allowed_destination_chains {
                ctx.source_chain = source;
                ctx.destination_chain = destination;
                ctx.cross_chain = source != destination;
                let checks = evaluate(&policy, &ctx);
                assert!(deny_reason(&checks).is_none(), "{checks:#}");
            }
        }
        ctx.source_chain = "base";
        ctx.destination_chain = "base";
        ctx.cross_chain = false;
        ctx.slippage_bps = 101;
        assert!(deny_reason(&evaluate(&policy, &ctx)).is_some());
        ctx.slippage_bps = 50;
        ctx.receiver_class = "external";
        assert!(deny_reason(&evaluate(&policy, &ctx)).is_some());
    }

    #[test]
    fn unsupported_defi_field_is_rejected() {
        let text = "[defi]\nenabled = true\nfuture_permission = true\n";
        let err = toml::from_str::<EnsoVenueConfig>(text).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }
}
