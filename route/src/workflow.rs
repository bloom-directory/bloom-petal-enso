//! Workflow operations for Enso intent sessions.
//!
//! Lifecycle: create → route discovery → (optional simulate) → confirm →
//! outbox staging → broadcast → settlement verification.

pub use crate::runtime::{BloomHost, Host};
use crate::api_types::{NATIVE_TOKEN, RouteRequest, RouteResponse};
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
    let resolved = settings::resolve_api_key(private_store.as_deref(), runtime.as_deref())?;
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

    // Raw integer without a decimal point — parse directly (no scaling).
    if !trimmed.contains('.') {
        return U256::from_str_radix(trimmed, 10)
            .map_err(|_| format!("invalid amount: {trimmed}"));
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
        U256::from_str_radix(whole, 10)
            .map_err(|_| format!("invalid whole part: {whole}"))?
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
        U256::from_str_radix(&padded, 10)
            .map_err(|_| format!("invalid fractional part: {frac}"))?
    };

    Ok(scaled_whole + frac_val)
}

/// Build ERC-20 `approve(spender, type(uint256).max)` calldata.
fn build_approve_calldata(spender_hex: &str) -> String {
    // Selector: approve(address,uint256) = 0x095ea7b3
    let stripped = spender_hex.strip_prefix("0x").unwrap_or(spender_hex);
    let padded_spender = format!("{:0>64}", stripped.to_ascii_lowercase());
    // type(uint256).max = 0xffff…ffff (64 hex chars)
    format!("0x095ea7b3{padded_spender}{}", "f".repeat(64))
}

fn classify_receiver(_host_wallet_addr: &str, receiver_addr: &str) -> String {
    if receiver_addr.eq_ignore_ascii_case(_host_wallet_addr) {
        "wallet_eoa".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Basic policy evaluation — all local, no external proto dependency.
fn evaluate_policy(
    route_verified: bool,
    needs_approve: bool,
    cross_chain: bool,
    receiver_class: &str,
) -> serde_json::Value {
    let mut checks: Vec<serde_json::Value> = Vec::new();

    checks.push(serde_json::json!({
        "rule": "route_verified",
        "outcome": if route_verified { "pass" } else { "deny" },
        "message": if route_verified {
            "Enso route transaction input matches the request"
        } else {
            "Enso route transaction input does NOT match the request — refusing"
        },
    }));

    if needs_approve {
        checks.push(serde_json::json!({
            "rule": "erc20_approval",
            "outcome": "warn",
            "message": "An ERC-20 approve(spender, max) transaction will be staged before the swap",
        }));
    }

    if cross_chain {
        checks.push(serde_json::json!({
            "rule": "cross_chain",
            "outcome": "warn",
            "message": "This route crosses chains — settlement verification reads the destination chain",
        }));
    }

    checks.push(serde_json::json!({
        "rule": "receiver",
        "outcome": if receiver_class == "wallet_eoa" { "pass" } else { "warn" },
        "message": format!("Receiver classified as: {receiver_class}"),
    }));

    serde_json::Value::Array(checks)
}

/// Render a human-readable plan document for the session.
fn render_plan_md(
    intent_text: &str,
    chain_name: &str,
    destination_chain: Option<&str>,
    req: &RouteRequest,
    route: &RouteResponse,
    intents: &[PreparedIntent],
    route_verified: bool,
    receiver_class: &str,
    observed_before: Option<&str>,
) -> String {
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
         - Route verified: {route_verified}\n\
         - Protocols: {protocol_str}\n\
         - Gas estimate: {}\n\
         - Price impact (display only): {}\n\
         - Receiver class: {receiver_class}\n\n",
        req.chain_id,
        req.amount_in,
        route.amount_out,
        route.gas.as_deref().unwrap_or("not estimated"),
        route
            .price_impact
            .map(|v| format!("{v}"))
            .unwrap_or_else(|| "not reported".into()),
    );

    out.push_str("## Transactions to stage\n\n");
    for (i, intent) in intents.iter().enumerate() {
        out.push_str(&format!(
            "**{i}. `{}`** — `{}`\n\
             - To: `{}`\n\
             - Value: {} wei\n\
             - Calldata: `{}…`\n",
            intent.label,
            intent.chain,
            intent.to,
            intent.value_wei,
            &intent.data_hex[..intent.data_hex.len().min(74)],
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

    out.push_str("Write `confirm` to stage these transactions into the wallet outbox.\n");
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
        .unwrap_or("ethereum");

    // Determine chain_id — try on-chain first, fall back to static table.
    let chain_id = match host.chain_id(chain_name) {
        Ok(id) => id,
        Err(_) => input::chain_to_id(chain_name)
            .ok_or_else(|| format!("unsupported chain: {chain_name}"))?,
    };

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
    let destination_chain = parsed.destination_chain.clone();
    let dest_chain_name = destination_chain.as_deref().unwrap_or(chain_name);
    let dest_chain_id = if let Some(ref dest) = destination_chain {
        input::chain_to_id(dest)
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
        // Hex address — try on-chain decimals.
        match host.erc20_decimals(chain_name, &token_in_hex) {
            Ok(d) => d,
            Err(_) => 18, // safe default
        }
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

    let cross_chain = destination_chain.is_some() && destination_chain.as_deref() != Some(chain_name);
    if let Some(dest_id) = dest_chain_id {
        if dest_id != chain_id {
            route_req.destination_chain_id = Some(dest_id);
            // For cross-chain, default receiver to the wallet address itself.
            if parsed.receiver.is_none() {
                route_req.receiver = Some(from_address);
            }
        }
    }

    if let Some(ref recv) = parsed.receiver {
        if let Ok(addr) = recv.parse::<Address>() {
            route_req.receiver = Some(addr);
        }
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

    let router_addr = format!("0x{:x}", route_resp.tx.to);

    // Check ERC-20 allowance and build approve intent if needed.
    let token_in_is_native = token_in_hex.eq_ignore_ascii_case(NATIVE_TOKEN);
    let mut needs_approve = false;
    let mut intents: Vec<PreparedIntent> = Vec::new();

    if !token_in_is_native {
        let allowance = host
            .erc20_allowance(chain_name, &token_in_hex, &address, &router_addr)
            .unwrap_or_else(|_| "0".to_string());

        if lt_decimal(&allowance, &route_req.amount_in.to_string()) {
            needs_approve = true;
            let approve_data = build_approve_calldata(&router_addr);
            intents.push(PreparedIntent {
                label: "approve".into(),
                to: token_in_hex.clone(),
                value_wei: "0".into(),
                data_hex: approve_data,
                chain: chain_name.to_string(),
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
        chain: chain_name.to_string(),
        approve_token: None,
        approve_spender: None,
    });

    // Classify receiver.
    let receiver_addr = route_req
        .receiver
        .map(|a| format!("0x{:x}", a))
        .unwrap_or_else(|| address.clone());
    let receiver_class = classify_receiver(&address, &receiver_addr);

    // Evaluate policy checks.
    let policy_checks = evaluate_policy(route_verified, needs_approve, cross_chain, &receiver_class);

    // Observe settlement baseline.
    let token_out_hex = format!("0x{:x}", route_req.token_out);
    let observed_before = if !token_out_hex.eq_ignore_ascii_case(NATIVE_TOKEN) {
        crate::settlement::observe_balance_before(
            host,
            dest_chain_name,
            &token_out_hex,
            &receiver_addr,
        )
    } else {
        None
    };

    // Generate session ID.
    let id = generate_id(host)?;

    // Build plan markdown.
    let plan_md = render_plan_md(
        &parsed.intent,
        chain_name,
        destination_chain.as_deref(),
        &route_req,
        &route_resp,
        &intents,
        route_verified,
        &receiver_class,
        observed_before.as_deref(),
    );

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
            updated_ms: now_ms,
        })
        .collect();

    let mut sess = Session {
        schema_version: 1,
        id: id.clone(),
        wallet: wallet.to_string(),
        wallet_address: address,
        chain: chain_name.to_string(),
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
        min_settlement_delta: None,
        source_tx_hashes: Vec::new(),
        policy_checks,
        receiver_class: Some(receiver_class),
        simulation: None,
        last_error: None,
        history: Vec::new(),
    };
    sess.transition(now, "prepared", "route discovered and verified");
    save(host, &sess)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// confirm — stage all prepared intents into the outbox
// ---------------------------------------------------------------------------

pub fn confirm<H: Host>(
    host: &mut H,
    wallet: &str,
    id: &str,
    _body: &[u8],
) -> Result<(), String> {
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

    // Re-verify route binding.
    let req = sess
        .route_request
        .as_ref()
        .ok_or("session has no route request — refusing to stage")?;
    let route = sess
        .route
        .as_ref()
        .ok_or("session has no route — refusing to stage")?;

    // Verify addresses match.
    let wallet_addr = &sess.wallet_address;
    let from_hex = format!("0x{:x}", req.from_address);
    if from_hex != *wallet_addr {
        return Err("route request from_address does not match session wallet_address".into());
    }
    let tx_from_hex = format!("0x{:x}", route.tx.from);
    if tx_from_hex != *wallet_addr {
        return Err("route tx from does not match session wallet_address".into());
    }

    // Re-verify route input integrity.
    if !route.input_matches_request(req) {
        return Err(
            "stored Enso route transaction input does not match the stored request — refusing"
                .into(),
        );
    }

    // Verify chain_id.
    match host.chain_id(&sess.chain) {
        Ok(live_chain_id) if live_chain_id == req.chain_id => { /* ok */ }
        Ok(live_chain_id) => {
            return Err(format!(
                "live chain_id ({live_chain_id}) does not match session chain_id ({}) — refusing",
                req.chain_id
            ));
        }
        Err(_) => { /* chain_read unavailable — proceed cautiously */ }
    }

    // Stage each intent sequentially.
    let mut staged_ids: Vec<String> = Vec::new();
    let intents = sess.intents.clone();

    for (idx, intent) in intents.iter().enumerate() {
        // Skip already-staged intents.
        if let Some(state) = sess.intent_states.get(idx) {
            if let Some(ref existing_id) = state.outbox_id {
                // Check if already pending/staged in outbox.
                if state.status == "staged" {
                    staged_ids.push(existing_id.clone());
                    continue;
                }
            }
        }

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

        if let Some(state) = sess.intent_states.get_mut(idx) {
            state.status = "staged".into();
            state.outbox_id = Some(staged.outbox_id.clone());
            state.updated_ms = now;
        }

        staged_ids.push(staged.outbox_id.clone());

        // Persist after each successful stage (crash recovery).
        sess.staged_ids = staged_ids.clone();
        sess.updated_ms = now;
        save(host, &sess)?;
    }

    sess.staged_ids = staged_ids;
    sess.transition(now, "staged", "all intents staged into outbox");
    save(host, &sess)?;
    Ok(())
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
        assert_eq!(v, U256::from(100u64));
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
        let data = build_approve_calldata("0x1234567890abcdef1234567890abcdef12345678");
        assert!(data.starts_with("0x095ea7b3"));
        // Spender should be padded to 32 bytes.
        assert!(data.contains("0000000000000000000000001234567890abcdef1234567890abcdef12345678"));
        // Max uint256 at the end.
        assert!(data.ends_with(&"f".repeat(64)));
    }

    #[test]
    fn decimal_less_than() {
        assert!(lt_decimal("99", "100"));
        assert!(!lt_decimal("100", "100"));
        assert!(!lt_decimal("101", "100"));
        assert!(lt_decimal("0", "1"));
        assert!(!lt_decimal("0", "0"));
        assert!(lt_decimal(
            "999999999999999999",
            "1000000000000000000"
        ));
    }
}
