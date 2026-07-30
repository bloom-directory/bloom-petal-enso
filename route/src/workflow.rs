//! Workflow operations for Enso intent sessions.
//!
//! Lifecycle: create → route discovery → (optional simulate) → confirm →
//! outbox staging → broadcast → settlement verification.

use crate::api_types::{NATIVE_TOKEN, RouteRequest, RouteResponse};
pub use crate::runtime::{BloomHost, Host};
use crate::session::{self, IntentState, PreparedIntent, Session};
use crate::{api, input, settings};
use alloy::primitives::{Address, U256};
use petal::sdk::EvmTransaction;

// ---------------------------------------------------------------------------
// persistence helpers
// ---------------------------------------------------------------------------

fn json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|e| e.to_string())
}

fn save<H: Host>(host: &mut H, s: &Session) -> Result<(), String> {
    host.put(&s.key(), &json(s)?, false)
}

pub fn load<H: Host>(host: &mut H, wallet: &str, id: &str) -> Result<Session, String> {
    validate_wallet_name(wallet)?;
    validate_session_id(id)?;
    let raw = host
        .get(&session::key(wallet, id), 2 * 1024 * 1024)?
        .ok_or("session not found")?;
    serde_json::from_slice(&raw).map_err(|e| format!("corrupt session: {e}"))
}

// ---------------------------------------------------------------------------
// resolution helpers
// ---------------------------------------------------------------------------

fn resolve_api_key<H: Host>(host: &mut H) -> Result<String, String> {
    let private_store = host.get_secret(settings::API_KEY, 8192)?;
    let runtime = host.setting("enso-api-key")?;
    let resolved = settings::configured_api_key(private_store.as_deref(), runtime.as_deref())?;
    Ok(resolved.key.expose().to_string())
}

fn wallet_address<H: Host>(host: &mut H, wallet: &str) -> Result<String, String> {
    validate_wallet_name(wallet)?;
    let address = String::from_utf8(host.vfs_read(&format!("wallets/{wallet}/address"), 128)?)
        .map_err(|_| "wallet address is not UTF-8")?
        .trim()
        .to_string();
    validate_address(&address)?;
    Ok(address)
}

fn validate_wallet_name(wallet: &str) -> Result<(), String> {
    if wallet.is_empty()
        || wallet.len() > 128
        || !wallet
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("wallet name is invalid".into());
    }
    Ok(())
}

fn validate_address(addr: &str) -> Result<(), String> {
    if !addr.starts_with("0x") || addr.len() != 42 {
        return Err("wallet address must be a 0x-prefixed 20-byte hex string".into());
    }
    Ok(())
}

fn validate_session_id(id: &str) -> Result<(), String> {
    if id.len() != 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("session ID is invalid".into());
    }
    Ok(())
}

fn generate_id<H: Host>(host: &mut H) -> Result<String, String> {
    let bytes = host.random(8)?;
    Ok(hex::encode(&bytes))
}

// ---------------------------------------------------------------------------
// amount / calldata helpers
// ---------------------------------------------------------------------------

/// Parse a human-readable amount into raw smallest-units using explicit
/// decimals.
fn parse_amount(amount: &str, decimals: u8) -> Result<U256, String> {
    let trimmed = amount.trim();
    if trimmed.is_empty() {
        return Err("amount is empty".into());
    }

    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() > 2 {
        return Err(format!("invalid amount: {trimmed}"));
    }

    let whole = parts[0];
    let frac = if parts.len() == 2 { parts[1] } else { "" };

    let whole_val = if whole.is_empty() {
        U256::from(0u64)
    } else {
        U256::from_str_radix(whole, 10).map_err(|_| format!("invalid whole part: {whole}"))?
    };

    let scaled_whole = whole_val * U256::from(10u64).pow(U256::from(decimals as u64));

    let frac_val = if frac.is_empty() {
        U256::from(0u64)
    } else {
        if frac.len() > decimals as usize {
            return Err(format!(
                "too many decimal places: {} has more than {decimals} digits",
                frac
            ));
        }
        let padded = format!("{:0<width$}", frac, width = decimals as usize);
        U256::from_str_radix(&padded, 10).map_err(|_| format!("invalid fractional part: {frac}"))?
    };

    Ok(scaled_whole + frac_val)
}

/// Build ERC-20 `approve(spender, amount)` calldata.
fn build_approve_calldata(spender_hex: &str, amount: U256) -> String {
    // Selector: approve(address,uint256) = 0x095ea7b3
    let stripped = spender_hex.strip_prefix("0x").unwrap_or(spender_hex);
    let padded_spender = format!("{:0>64}", stripped.to_ascii_lowercase());
    format!("0x095ea7b3{padded_spender}{amount:064x}")
}

fn classify_receiver(wallet_addr: &str, receiver_addr: &str) -> String {
    if receiver_addr.eq_ignore_ascii_case(wallet_addr) {
        "wallet_eoa".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Enso returns the quoted output as a raw integer string (wei units, no
/// decimal point). This rejects empty, negative, zero, and non-digit values.
fn valid_positive_decimal(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

fn verify_prepared_intents(
    sess: &Session,
    req: &RouteRequest,
    route: &RouteResponse,
) -> Result<(), String> {
    if sess.intents.len() != sess.intent_states.len()
        || sess
            .intent_states
            .iter()
            .enumerate()
            .any(|(index, state)| state.index != index)
    {
        return Err("session intent state layout is invalid — refusing to stage".into());
    }

    let router = format!("0x{:x}", route.tx.to);
    let route_data = format!("0x{}", hex::encode(&route.tx.data));
    let route_intent = sess
        .intents
        .last()
        .filter(|intent| intent.label == "route")
        .ok_or("session route intent is missing or out of order")?;
    if !route_intent.to.eq_ignore_ascii_case(&router)
        || route_intent.value_wei != route.tx.value.to_string()
        || !route_intent.data_hex.eq_ignore_ascii_case(&route_data)
        || !route_intent.chain.eq_ignore_ascii_case(&sess.chain)
        || route_intent.approve_token.is_some()
        || route_intent.approve_spender.is_some()
    {
        return Err("prepared route transaction does not match the verified Enso route".into());
    }

    let approval_count = sess
        .intents
        .iter()
        .filter(|intent| intent.label == "approve")
        .count();
    if approval_count == 0 {
        if sess.intents.len() != 1 {
            return Err("session contains an unexpected prepared transaction".into());
        }
        return Ok(());
    }
    if approval_count != 1 || sess.intents.len() != 2 {
        return Err("session approval sequence is invalid — refusing to stage".into());
    }

    let approval = &sess.intents[0];
    let token = format!("0x{:x}", req.token_in);
    let expected_data = build_approve_calldata(&router, req.amount_in);
    if !approval.to.eq_ignore_ascii_case(&token)
        || approval.value_wei != "0"
        || !approval.data_hex.eq_ignore_ascii_case(&expected_data)
        || !approval.chain.eq_ignore_ascii_case(&sess.chain)
        || !approval
            .approve_token
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(&token))
        || !approval
            .approve_spender
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(&router))
    {
        return Err("prepared approval does not match the exact route allowance".into());
    }
    Ok(())
}

/// Render a human-readable plan document for the session.
struct PlanView<'a> {
    intent_text: &'a str,
    chain_name: &'a str,
    destination_chain: Option<&'a str>,
    req: &'a RouteRequest,
    route: &'a RouteResponse,
    intents: &'a [PreparedIntent],
    route_verified: bool,
    receiver_class: &'a str,
    observed_before: Option<&'a str>,
    policy_checks: &'a serde_json::Value,
    simulation: Option<&'a serde_json::Value>,
}

fn render_plan_md(view: PlanView<'_>) -> String {
    let PlanView {
        intent_text,
        chain_name,
        destination_chain,
        req,
        route,
        intents,
        route_verified,
        receiver_class,
        observed_before,
        policy_checks,
        simulation,
    } = view;
    let dest = destination_chain.unwrap_or(chain_name);
    let protocols = route.protocols();
    let protocol_str = if protocols.1 {
        "unknown protocols".to_string()
    } else if protocols.0.is_empty() {
        "no protocol info".to_string()
    } else {
        protocols.0.join(", ")
    };

    let token_in_hex = format!("0x{:x}", req.token_in);
    let token_out_hex = format!("0x{:x}", req.token_out);

    // Receiver address (checksummed via alloy).
    let receiver_addr = req
        .receiver
        .map(|a| format!("0x{:x}", a))
        .unwrap_or_else(|| format!("0x{:x}", req.from_address));

    let mut out = format!(
        "# Enso Shortcuts Swap\n\n\
         The following is Bloom's authoritative transaction plan.\n\n\
         - Intent: `{intent_text}`\n\
         - Source chain: `{chain_name}` (chain ID {})\n\
         - Destination chain: `{dest}`\n\
         - Token in: `{token_in_hex}`\n\
         - Token out: `{token_out_hex}`\n\
         - Amount in: {}\n\
         - Expected output: {}\n\
         - Slippage tolerance: {} bps (0.{}%)\n\
         - Receiver: `{receiver_addr}`\n\
         - Route verified: {route_verified}\n\
         - Protocols: {protocol_str}\n\
         - Gas estimate: {}\n\
         - Price impact (display only): {}\n\
         - Receiver class: {receiver_class}\n\n",
        req.chain_id,
        req.amount_in,
        route.amount_out,
        req.slippage_bps,
        req.slippage_bps,
        route.gas.as_deref().unwrap_or("not estimated"),
        route
            .price_impact
            .map(|v| format!("{v}"))
            .unwrap_or_else(|| "not reported".into()),
    );

    // Simulation summary.
    if let Some(sim) = simulation {
        let blocked_by_approval = sim
            .get("blocked_by_approval")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let sim_ok = sim
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if blocked_by_approval {
            out.push_str("- Simulation: **deferred until approval succeeds** ⏳\n\n");
        } else if sim_ok {
            out.push_str("- Simulation: **passed** ✅\n\n");
        } else {
            let msg = sim
                .get("decoded_error")
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .or_else(|| sim.get("error").and_then(|v| v.as_str()))
                .unwrap_or("simulation failed");
            out.push_str(&format!("- Simulation: **failed** ❌ — {msg}\n\n"));
        }
    }

    // Policy checks section.
    if let Some(checks) = policy_checks.as_array()
        && !checks.is_empty()
    {
        out.push_str("## Policy checks\n\n");
        for check in checks {
            let rule = check.get("rule").and_then(|v| v.as_str()).unwrap_or("?");
            let outcome = check.get("outcome").and_then(|v| v.as_str()).unwrap_or("?");
            let message = check.get("message").and_then(|v| v.as_str()).unwrap_or("");
            let icon = match outcome {
                "pass" => "✅",
                "warn" => "⚠️",
                "deny" => "❌",
                _ => "❓",
            };
            out.push_str(&format!("- {icon} **{rule}** ({outcome}): {message}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Transactions to stage\n\n");
    for (i, intent) in intents.iter().enumerate() {
        let data_len = if intent.data_hex.starts_with("0x") {
            (intent.data_hex.len() - 2) / 2
        } else {
            intent.data_hex.len() / 2
        };
        out.push_str(&format!(
            "**{i}. `{}`** — `{}`\n\
             - To: `{}`\n\
             - Value: {} wei\n\
             - Calldata: `{}…` ({} bytes)\n",
            intent.label,
            intent.chain,
            intent.to,
            intent.value_wei,
            &intent.data_hex[..intent.data_hex.len().min(74)],
            data_len,
        ));
        if let Some(ref token) = intent.approve_token {
            out.push_str(&format!("- Approve token: `{token}`\n"));
        }
        if let Some(ref spender) = intent.approve_spender {
            out.push_str(&format!("- Spender: `{spender}`\n"));
        }
        out.push('\n');
    }

    if let Some(before) = observed_before {
        out.push_str(&format!(
            "## Settlement baseline\n\n\
             Destination token balance before staging: `{before}`\n\n"
        ));
    }

    out.push_str(&format!(
        "Write exactly `confirm` to stage the next safe transaction. \
         This plan contains {} transaction(s); an approval, when present, \
         must succeed before a second confirmation can stage the route.\n",
        intents.len()
    ));
    out
}

// ---------------------------------------------------------------------------
// create — route discovery + session creation
// ---------------------------------------------------------------------------

pub fn create<H: Host>(host: &mut H, wallet: &str, body: &[u8]) -> Result<String, String> {
    let now = host.now_ms();
    let parsed = input::parse_new_body(body)?;
    let address = wallet_address(host, wallet)?;
    let api_key = resolve_api_key(host)?;

    // Determine source chain.
    let nat_opt = input::parse_natural_intent(&parsed.intent);
    let nat_chain = nat_opt.as_ref().and_then(|n| n.chain.clone());
    let chain_name = parsed
        .chain
        .as_deref()
        .or(nat_chain.as_deref())
        .unwrap_or("ethereum")
        .to_ascii_lowercase();

    // Resolve the configured RPC and prove that it is the requested chain.
    let chain_id = host
        .chain_id(&chain_name)
        .map_err(|e| format!("cannot verify source chain {chain_name}: {e}"))?;

    // Parse the natural intent.
    let nat = nat_opt.ok_or_else(|| {
        format!(
            "could not parse intent '{}' (expected `swap <amount> <tok> to <tok>`)",
            parsed.intent
        )
    })?;

    // Resolve token_in on source chain.
    let token_in = input::resolve_token_symbol(chain_id, &nat.token_in)
        .ok_or_else(|| format!("could not resolve token symbol: {}", nat.token_in))?;

    // Determine destination chain.
    let destination_chain = parsed
        .destination_chain
        .as_ref()
        .map(|chain| chain.to_ascii_lowercase());
    let dest_chain_name = destination_chain.as_deref().unwrap_or(&chain_name);
    let dest_chain_id = if let Some(ref dest) = destination_chain {
        Some(
            host.chain_id(dest)
                .map_err(|e| format!("cannot verify destination chain {dest}: {e}"))?,
        )
    } else {
        Some(chain_id)
    };

    // Resolve token_out — on destination chain if cross-chain, else source.
    let token_out = {
        let resolve_chain_id = dest_chain_id.unwrap_or(chain_id);
        input::resolve_token_symbol(resolve_chain_id, &nat.token_out)
            .ok_or_else(|| format!("could not resolve token symbol: {}", nat.token_out))?
    };

    // Resolve decimals for token_in.
    let token_in_hex = format!("0x{:x}", token_in);
    let decimals = if token_in_hex.eq_ignore_ascii_case(NATIVE_TOKEN) {
        18u8
    } else if nat.token_in.starts_with("0x") || nat.token_in.starts_with("0X") {
        host.erc20_decimals(&chain_name, &token_in_hex)
            .map_err(|e| format!("cannot read token decimals: {e}"))?
    } else {
        input::decimals_for_symbol(chain_id, &nat.token_in)
    };

    // Parse amount with correct decimals.
    let amount_raw = parse_amount(&nat.amount, decimals)?;

    let from_address: Address = address
        .parse()
        .map_err(|_| format!("invalid wallet address: {address}"))?;

    // Build route request.
    let mut route_req = RouteRequest::new(from_address, chain_id, token_in, token_out, amount_raw);
    route_req.slippage_bps = parsed.slippage_bps.unwrap_or(50);

    let cross_chain = destination_chain.is_some()
        && !destination_chain
            .as_deref()
            .map(|d| d.eq_ignore_ascii_case(&chain_name))
            .unwrap_or(false);
    if let Some(dest_id) = dest_chain_id
        && dest_id != chain_id
    {
        route_req.destination_chain_id = Some(dest_id);
        // For cross-chain, default receiver to the wallet address itself.
        if parsed.receiver.is_none() {
            route_req.receiver = Some(from_address);
        }
    }

    if let Some(ref recv) = parsed.receiver {
        let addr = recv
            .parse::<Address>()
            .map_err(|_| format!("invalid receiver address: {recv}"))?;
        route_req.receiver = Some(addr);
    }

    // Call Enso route API.
    let route_resp = api::route(host, &api_key, &route_req)?;

    // SECURITY: verify route input matches request — reject if not.
    let route_verified = route_resp.input_matches_request(&route_req);
    if !route_verified {
        return Err(
            "Enso route transaction input does not match the requested token and amount — refusing"
                .into(),
        );
    }
    if !route_resp.destination_matches_request(&route_req) {
        return Err(
            "Enso route destination chain does not match the requested destination — refusing"
                .into(),
        );
    }
    if !valid_positive_decimal(route_resp.amount_out.trim()) {
        return Err("Enso route returned an invalid quoted output amount — refusing".into());
    }

    let router_addr = format!("0x{:x}", route_resp.tx.to);

    // Check ERC-20 allowance and build approve intent if needed.
    let token_in_is_native = token_in_hex.eq_ignore_ascii_case(NATIVE_TOKEN);
    let mut needs_approve = false;
    let mut intents: Vec<PreparedIntent> = Vec::new();

    if !token_in_is_native {
        let allowance = host
            .erc20_allowance(&chain_name, &token_in_hex, &address, &router_addr)
            .map_err(|e| format!("cannot verify ERC-20 allowance: {e}"))?;

        if lt_decimal(&allowance, &route_req.amount_in.to_string()) {
            needs_approve = true;
            let approve_data = build_approve_calldata(&router_addr, route_req.amount_in);
            intents.push(PreparedIntent {
                label: "approve".into(),
                to: token_in_hex.clone(),
                value_wei: "0".into(),
                data_hex: approve_data,
                chain: chain_name.clone(),
                approve_token: Some(token_in_hex.clone()),
                approve_spender: Some(router_addr.clone()),
            });
        }
    }

    // Build the route intent.
    intents.push(PreparedIntent {
        label: "route".into(),
        to: router_addr.clone(),
        value_wei: route_resp.tx.value.to_string(),
        data_hex: format!("0x{}", hex::encode(&route_resp.tx.data)),
        chain: chain_name.clone(),
        approve_token: None,
        approve_spender: None,
    });

    // Classify receiver.
    let receiver_addr = route_req
        .receiver
        .map(|a| format!("0x{:x}", a))
        .unwrap_or_else(|| address.clone());
    let receiver_class = classify_receiver(&address, &receiver_addr);
    let token_out_hex = format!("0x{:x}", route_req.token_out);

    // Load and enforce the wallet's current signed route policy.
    let protocols = route_resp.protocols();
    let verified_policy = crate::policy::load_verified_policy(host, wallet)?;
    let policy_checks = crate::policy::evaluate(
        &verified_policy,
        &crate::policy::RoutePolicyContext {
            source_chain: &chain_name,
            destination_chain: dest_chain_name,
            cross_chain,
            receiver: &receiver_addr,
            token_out: &token_out_hex,
            receiver_class: &receiver_class,
            router: &router_addr,
            protocols: &protocols.0,
            protocols_unknown: protocols.1,
            native_value_wei: route_resp.tx.value,
            slippage_bps: route_req.slippage_bps,
            route_verified,
            // Enso Router V2's opaque action bytes are not decoded here.
            receiver_verified: false,
            min_out_enforced: false,
            needs_approve,
        },
    );
    if let Some(reason) = crate::policy::deny_reason(&policy_checks) {
        return Err(reason);
    }

    // A route that already has allowance must simulate successfully. When an
    // approval is needed, simulation is deferred until that approval succeeds.
    let simulation = if needs_approve {
        serde_json::json!({
            "success": false,
            "blocked_by_approval": true,
            "message": "route simulation deferred until the exact-amount approval succeeds",
        })
    } else {
        let result = crate::simulation::simulate_route_response(host, &chain_name, &route_resp);
        if !result
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Err(format!(
                "route simulation failed: {}",
                crate::simulation::failure_message(&result)
            ));
        }
        result
    };

    // Observe settlement baseline.
    let observed_before = Some(crate::settlement::observe_balance_before(
        host,
        dest_chain_name,
        &token_out_hex,
        &receiver_addr,
    )?);

    // Generate session ID.
    let id = generate_id(host)?;

    // Build plan markdown.
    let plan_md = render_plan_md(PlanView {
        intent_text: &parsed.intent,
        chain_name: &chain_name,
        destination_chain: destination_chain.as_deref(),
        req: &route_req,
        route: &route_resp,
        intents: &intents,
        route_verified,
        receiver_class: &receiver_class,
        observed_before: observed_before.as_deref(),
        policy_checks: &policy_checks,
        simulation: Some(&simulation),
    });

    // Build intent_states (one per intent, all "prepared").
    let now_ms = now;
    let intent_states: Vec<IntentState> = intents
        .iter()
        .enumerate()
        .map(|(i, _)| IntentState {
            index: i,
            status: "prepared".into(),
            outbox_id: None,
            tx_hash: None,
            depends_on: None,
            approval: None,
            updated_ms: now_ms,
        })
        .collect();

    let mut sess = Session {
        schema_version: 1,
        id: id.clone(),
        wallet: wallet.to_string(),
        wallet_address: address,
        chain: chain_name,
        destination_chain: destination_chain.clone(),
        intent_text: parsed.intent.clone(),
        route_request: Some(route_req),
        route: Some(route_resp),
        plan_md,
        intents,
        intent_states,
        staged_ids: Vec::new(),
        created_ms: now,
        updated_ms: now,
        state: "prepared".into(),
        observed_before,
        policy_checks,
        receiver_class: Some(receiver_class),
        simulation: Some(simulation),
        last_error: None,
        history: Vec::new(),
    };
    sess.transition(now, "prepared", "route discovered and verified");
    save(host, &sess)?;

    // Store latest pointer for convenience lookups.
    let _ = host.put(&format!("intents/{wallet}/latest"), id.as_bytes(), false);

    Ok(id)
}

// ---------------------------------------------------------------------------
// confirm — stage all prepared intents into the outbox
// ---------------------------------------------------------------------------

const CONFIRM_LOCK_TTL_MS: u64 = 10 * 60 * 1000;

fn confirm_lock_key(wallet: &str, id: &str) -> String {
    format!("intents/{wallet}/{id}/confirm.lock")
}

fn acquire_confirm_lock<H: Host>(
    host: &mut H,
    wallet: &str,
    id: &str,
) -> Result<(String, Vec<u8>), String> {
    validate_wallet_name(wallet)?;
    validate_session_id(id)?;
    let key = confirm_lock_key(wallet, id);
    let now = host.now_ms();
    let nonce = hex::encode(host.random(16)?);
    let value = serde_json::to_vec(&serde_json::json!({
        "created_ms": now,
        "nonce": nonce,
    }))
    .map_err(|error| format!("cannot encode confirmation lock: {error}"))?;

    if host.put_new(&key, &value, false).is_ok() {
        return Ok((key, value));
    }

    let existing = host
        .get(&key, 1024)?
        .ok_or("confirmation is already in progress")?;
    let created_ms = serde_json::from_slice::<serde_json::Value>(&existing)
        .ok()
        .and_then(|lock| lock.get("created_ms").and_then(|value| value.as_u64()));
    if created_ms.is_none_or(|created| now.saturating_sub(created) <= CONFIRM_LOCK_TTL_MS) {
        return Err("confirmation is already in progress".into());
    }

    host.delete_if(&key, &existing)?;
    host.put_new(&key, &value, false)
        .map_err(|_| "confirmation is already in progress".to_string())?;
    Ok((key, value))
}

pub fn confirm<H: Host>(host: &mut H, wallet: &str, id: &str, body: &[u8]) -> Result<(), String> {
    let (lock_key, lock_value) = acquire_confirm_lock(host, wallet, id)?;
    let result = confirm_locked(host, wallet, id, body);
    let unlock = host.delete_if(&lock_key, &lock_value);
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(format!(
            "transaction was staged but confirmation lock cleanup failed: {error}"
        )),
    }
}

fn confirm_locked<H: Host>(
    host: &mut H,
    wallet: &str,
    id: &str,
    body: &[u8],
) -> Result<(), String> {
    let confirmation = std::str::from_utf8(body)
        .map_err(|_| "confirmation body must be UTF-8")?
        .trim();
    if confirmation != "confirm" {
        return Err("write exactly `confirm` to authorize outbox staging".into());
    }

    let now = host.now_ms();
    let mut sess = load(host, wallet, id)?;

    // SECURITY: verify wallet ownership.
    if sess.wallet != wallet {
        return Err("wallet mismatch: session does not belong to this wallet".into());
    }

    // Idempotent: if already staged, return.
    if sess.state == "staged" || sess.state == "confirmed" {
        return Ok(());
    }

    // Refuse to confirm a session that has been abandoned or is otherwise terminal.
    if sess.terminal() {
        return Err(format!(
            "cannot confirm a session in terminal state: {}",
            sess.state
        ));
    }

    // Re-verify route binding.
    let req = sess
        .route_request
        .clone()
        .ok_or("session has no route request — refusing to stage")?;
    let route = sess
        .route
        .clone()
        .ok_or("session has no route — refusing to stage")?;

    // Verify addresses match (case-insensitive — checksum vs lowercase).
    let wallet_addr = sess.wallet_address.clone();
    let from_hex = format!("0x{:x}", req.from_address);
    if !from_hex.eq_ignore_ascii_case(&wallet_addr) {
        return Err("route request from_address does not match session wallet_address".into());
    }
    let tx_from_hex = format!("0x{:x}", route.tx.from);
    if !tx_from_hex.eq_ignore_ascii_case(&wallet_addr) {
        return Err("route tx from does not match session wallet_address".into());
    }

    // Re-verify route input integrity.
    if !route.input_matches_request(&req) {
        return Err(
            "stored Enso route transaction input does not match the stored request — refusing"
                .into(),
        );
    }
    if !route.destination_matches_request(&req) {
        return Err("stored route destination does not match the stored request — refusing".into());
    }
    if !valid_positive_decimal(route.amount_out.trim()) {
        return Err("stored route has an invalid quoted output amount — refusing".into());
    }
    verify_prepared_intents(&sess, &req, &route)?;

    // Verify chain_id.
    let live_chain_id = host
        .chain_id(&sess.chain)
        .map_err(|e| format!("cannot verify source chain at confirm time: {e}"))?;
    if live_chain_id != req.chain_id {
        return Err(format!(
            "live chain_id ({live_chain_id}) does not match session chain_id ({}) — refusing",
            req.chain_id
        ));
    }

    // Re-read and enforce the current signed policy at the last possible
    // moment. The outbox host verifies the passkey policy signature again.
    let needs_approve = sess.intents.iter().any(|i| i.label == "approve");
    let cross_chain = sess
        .destination_chain
        .as_deref()
        .map(|d| !d.eq_ignore_ascii_case(&sess.chain))
        .unwrap_or(false);
    let destination_chain = sess.destination_chain.as_deref().unwrap_or(&sess.chain);
    let receiver_class = sess.receiver_class.as_deref().unwrap_or("unknown");
    let receiver = req
        .receiver
        .map(|address| format!("0x{address:x}"))
        .unwrap_or_else(|| sess.wallet_address.clone());
    let token_out = format!("0x{:x}", req.token_out);
    let router = format!("0x{:x}", route.tx.to);
    let protocols = route.protocols();
    let verified_policy = crate::policy::load_verified_policy(host, wallet)?;
    sess.policy_checks = crate::policy::evaluate(
        &verified_policy,
        &crate::policy::RoutePolicyContext {
            source_chain: &sess.chain,
            destination_chain,
            cross_chain,
            receiver: &receiver,
            token_out: &token_out,
            receiver_class,
            router: &router,
            protocols: &protocols.0,
            protocols_unknown: protocols.1,
            native_value_wei: route.tx.value,
            slippage_bps: req.slippage_bps,
            route_verified: true,
            receiver_verified: false,
            min_out_enforced: false,
            needs_approve,
        },
    );
    if let Some(reason) = crate::policy::deny_reason(&sess.policy_checks) {
        sess.last_error = Some(reason.clone());
        save(host, &sess)?;
        return Err(reason);
    }

    let intents = sess.intents.clone();
    if intents.is_empty()
        || intents.last().map(|intent| intent.label.as_str()) != Some("route")
        || intents
            .iter()
            .any(|intent| !matches!(intent.label.as_str(), "approve" | "route"))
    {
        return Err("session intent sequence is invalid — refusing to stage".into());
    }

    // ERC-20 approval is deliberately a separate confirmation phase because
    // the Petal WIT does not expose Bloom's outbox dependency API.
    if needs_approve {
        let approve = intents
            .first()
            .filter(|intent| intent.label == "approve")
            .ok_or("approval intent is missing or out of order")?;
        let approve_state = sess
            .intent_states
            .first()
            .ok_or("approval intent state is missing")?;

        if approve_state.outbox_id.is_none() {
            let evm_tx = EvmTransaction {
                wallet: wallet.to_string(),
                chain: approve.chain.clone(),
                to: approve.to.clone(),
                value_wei: approve.value_wei.clone(),
                data_hex: approve.data_hex.clone(),
                nonce: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
            };
            let staged = host.tx_stage(&evm_tx)?;
            if let Some(state) = sess.intent_states.first_mut() {
                state.status = "staged".into();
                state.outbox_id = Some(staged.outbox_id.clone());
                state.approval = staged.approval.as_ref().map(|approval| {
                    serde_json::json!({
                        "action_id": approval.action_id,
                        "ceremony_url": approval.ceremony_url,
                        "expires_ms": approval.expires_ms,
                    })
                });
                state.updated_ms = now;
            }
            if let Some(route_state) = sess.intent_states.get_mut(1) {
                route_state.depends_on = Some(staged.outbox_id.clone());
                route_state.updated_ms = now;
            }
            sess.staged_ids = vec![staged.outbox_id];
            sess.transition(
                now,
                "awaiting_approval",
                "exact-amount approval staged; route remains unstaged",
            );
            save(host, &sess)?;
            return Ok(());
        }

        let approval_id = approve_state.outbox_id.clone().unwrap();
        let inspection = host.tx_inspect(wallet, &approve.chain, &approval_id)?;
        if inspection.state != "success" {
            if matches!(
                inspection.state.as_str(),
                "failed" | "reverted" | "cancelled"
            ) {
                sess.last_error = Some(format!(
                    "approval outbox transaction ended as {}",
                    inspection.state
                ));
                sess.transition(
                    now,
                    "approval_failed",
                    "approval transaction did not succeed",
                );
                save(host, &sess)?;
            }
            return Err(format!(
                "approval {approval_id} is {}; broadcast it and wait for a successful receipt before confirming again",
                inspection.state
            ));
        }
        if let Some(state) = sess.intent_states.first_mut() {
            state.status = "confirmed".into();
            state.tx_hash = inspection.tx_hash;
            state.updated_ms = now;
        }

        let allowance = host
            .erc20_allowance(&sess.chain, &approve.to, &wallet_addr, &router)
            .map_err(|e| format!("cannot re-verify ERC-20 allowance: {e}"))?;
        if lt_decimal(&allowance, &req.amount_in.to_string()) {
            return Err("approval receipt succeeded but allowance is still insufficient".into());
        }
        sess.updated_ms = now;
        save(host, &sess)?;
    }

    // Re-simulate with current chain state immediately before staging the
    // executable route.
    let simulation = crate::simulation::simulate_route_response(host, &sess.chain, &route);
    if !simulation
        .get("success")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        let reason = format!(
            "route simulation failed at confirm time: {}",
            crate::simulation::failure_message(&simulation)
        );
        sess.simulation = Some(simulation);
        sess.last_error = Some(reason.clone());
        save(host, &sess)?;
        return Err(reason);
    }
    sess.simulation = Some(simulation);

    let route_index = intents.len() - 1;
    if sess
        .intent_states
        .get(route_index)
        .and_then(|state| state.outbox_id.as_ref())
        .is_none()
    {
        let intent = &intents[route_index];
        let evm_tx = EvmTransaction {
            wallet: wallet.to_string(),
            chain: intent.chain.clone(),
            to: intent.to.clone(),
            value_wei: intent.value_wei.clone(),
            data_hex: intent.data_hex.clone(),
            nonce: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };

        let staged = host.tx_stage(&evm_tx)?;

        if let Some(state) = sess.intent_states.get_mut(route_index) {
            state.status = "staged".into();
            state.outbox_id = Some(staged.outbox_id.clone());
            state.approval = staged.approval.as_ref().map(|approval| {
                serde_json::json!({
                    "action_id": approval.action_id,
                    "ceremony_url": approval.ceremony_url,
                    "expires_ms": approval.expires_ms,
                })
            });
            state.updated_ms = now;
        }
        sess.staged_ids.push(staged.outbox_id);
    }

    sess.transition(now, "staged", "route staged into outbox");
    save(host, &sess)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// abandon — cancel a session before any outbox transaction exists
// ---------------------------------------------------------------------------

pub fn abandon<H: Host>(host: &mut H, wallet: &str, id: &str) -> Result<(), String> {
    abandon_with_body(host, wallet, id, b"abandon")
}

pub fn abandon_with_body<H: Host>(
    host: &mut H,
    wallet: &str,
    id: &str,
    body: &[u8],
) -> Result<(), String> {
    let command = std::str::from_utf8(body)
        .map_err(|_| "abandon body must be UTF-8")?
        .trim();
    if command != "abandon" {
        return Err("write exactly `abandon` to cancel this session".into());
    }
    let now = host.now_ms();
    let mut sess = load(host, wallet, id)?;

    if sess.wallet != wallet {
        return Err("wallet mismatch: session does not belong to this wallet".into());
    }

    // Refuse if any intent has been staged to the outbox.
    if sess.intent_states.iter().any(|s| s.outbox_id.is_some()) {
        return Err("cannot abandon after an outbox transaction exists".into());
    }

    if sess.terminal() {
        return Err("session is already terminal".into());
    }

    sess.transition(now, "abandoned", "user abandoned before staging");
    save(host, &sess)
}

// ---------------------------------------------------------------------------
// decimal comparison helper
// ---------------------------------------------------------------------------

/// Returns true if `a` < `b` where both are decimal strings.
fn lt_decimal(a: &str, b: &str) -> bool {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    if a.is_empty() && b.is_empty() {
        return false; // both zero
    }
    if a.len() != b.len() {
        return a.len() < b.len();
    }
    a < b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_amount_no_decimals() {
        let v = parse_amount("100", 6).unwrap();
        assert_eq!(v, U256::from(100_000_000u64));
    }

    #[test]
    fn parses_decimal_eth_amount() {
        let v = parse_amount("1.5", 18).unwrap();
        assert_eq!(v, U256::from(1_500_000_000_000_000_000u128));
    }

    #[test]
    fn parses_decimal_usdc_amount() {
        let v = parse_amount("100.5", 6).unwrap();
        assert_eq!(v, U256::from(100_500_000u64));
    }

    #[test]
    fn rejects_too_many_decimals() {
        assert!(parse_amount("1.1234567", 6).is_err());
    }

    #[test]
    fn approve_calldata_format() {
        let data = build_approve_calldata(
            "0x1234567890abcdef1234567890abcdef12345678",
            U256::from(123u64),
        );
        assert!(data.starts_with("0x095ea7b3"));
        // Spender should be padded to 32 bytes.
        assert!(data.contains("0000000000000000000000001234567890abcdef1234567890abcdef12345678"));
        // Exact amount at the end.
        assert!(data.ends_with(&format!("{:064x}", U256::from(123u64))));
    }

    #[test]
    fn decimal_less_than() {
        assert!(lt_decimal("99", "100"));
        assert!(!lt_decimal("100", "100"));
        assert!(!lt_decimal("101", "100"));
        assert!(lt_decimal("0", "1"));
        assert!(!lt_decimal("0", "0"));
        assert!(lt_decimal("999999999999999999", "1000000000000000000"));
    }
}
