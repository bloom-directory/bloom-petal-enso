//! Workflow operations for Enso intent sessions.
//!
//! Lifecycle: create → route discovery → (optional simulate) → confirm →
//! outbox staging → broadcast → settlement verification.

use crate::api_types::{NATIVE_TOKEN, RouteRequest, RouteResponse};
use crate::runtime::StageError;
pub use crate::runtime::{BloomHost, Host};
use crate::session::{self, IntentState, PreparedIntent, Session};
use crate::{api, input, settings};
use alloy::primitives::{Address, U256};
use petal::sdk::{EvmTransaction, StagedTransaction};

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
    crate::wallet::validate_id(wallet)?;
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

/// Lets the SDK read an account's address through [`Host`].
struct HostVfs<'a, H>(&'a mut H);

impl<H: Host> petal::VfsRead for HostVfs<'_, H> {
    fn read(&mut self, path: &str, max_bytes: usize) -> Result<Vec<u8>, petal::SdkError> {
        self.0.vfs_read(path, max_bytes)
    }
}

/// The account's EVM address, classified as the SDK classifies the read: an
/// account with no EVM address is not found (`-1`), a denied read is `-2`,
/// and anything else is a backend failure.
fn wallet_address<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
) -> Result<String, CreateError> {
    account
        .address_evm_from(&mut HostVfs(host))
        .map_err(|response| match response {
            petal::DispatchResponse::Error { code: -4, message } => CreateError::Failed(message),
            petal::DispatchResponse::Error { code, message } => {
                CreateError::Account { code, message }
            }
            _ => CreateError::Failed("cannot read the account address".into()),
        })
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
         Prepared transaction and Enso quote. Nested shortcut effects are not independently verified.\n\n\
         - Intent: `{intent_text}`\n\
         - Source chain: `{chain_name}` (chain ID {})\n\
         - Destination chain: `{dest}`\n\
         - Token in: `{token_in_hex}`\n\
         - Token out: `{token_out_hex}`\n\
         - Amount in: {}\n\
         - Expected output (current Enso quote): {}\n\
         - Slippage tolerance: {} bps ({}.{:02}%), applied to the first reviewed quote\n\
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
        req.slippage_bps / 100,
        req.slippage_bps % 100,
        route.gas.as_deref().unwrap_or("not estimated"),
        route
            .price_impact
            .map(|v| format!("{v}"))
            .unwrap_or_else(|| "not reported".into()),
    );

    match crate::calldata::output_minimum(route, req) {
        Some(minimum) => out.push_str(&format!(
            "- Calldata minimum output: {minimum} raw units of `{token_out_hex}` to `{receiver_addr}`. This is the owner-reviewed floor; a refreshed quote never changes it. The Router V2 call checks this balance increase atomically; it is not a receipt or a guarantee of future execution.\n\n"
        )),
        None => out.push_str("- Destination output is not constrained by a decoded same-chain router check.\n\n"),
    }

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

/// Keep the readable plan tied to the currently prepared transaction after
/// refresh/simulation, rather than retaining the original quote's bytes.
fn update_plan(sess: &mut Session) -> Result<(), String> {
    let req = sess
        .route_request
        .as_ref()
        .ok_or("session has no route request")?;
    let route = sess.route.as_ref().ok_or("session has no route")?;
    sess.plan_md = render_plan_md(PlanView {
        intent_text: &sess.intent_text,
        chain_name: &sess.chain,
        destination_chain: sess.destination_chain.as_deref(),
        req,
        route,
        intents: &sess.intents,
        route_verified: route.input_matches_request(req),
        receiver_class: sess.receiver_class.as_deref().unwrap_or("unknown"),
        observed_before: sess.observed_before.as_deref(),
        policy_checks: &sess.policy_checks,
        simulation: sess.simulation.as_ref(),
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// create — route discovery + session creation
// ---------------------------------------------------------------------------

/// Why `new` refused an intent. Input mistakes surface as invalid input
/// (EINVAL on the mount); an account Bloom selected without a readable EVM
/// address keeps the SDK's code; everything else, such as Enso, RPC,
/// simulation or policy failures, surfaces as a backend failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    InvalidInput(String),
    /// The account has no EVM address (`-1`) or Bloom denied reading it
    /// (`-2`).
    Account {
        code: i32,
        message: String,
    },
    Failed(String),
}

impl CreateError {
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidInput(message) | Self::Account { message, .. } | Self::Failed(message) => {
                message
            }
        }
    }
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<String> for CreateError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for CreateError {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_owned())
    }
}

/// Create for Bloom's trusted selected account.
pub fn create_for_account<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    body: &[u8],
) -> Result<String, CreateError> {
    // Refuse a wallet name Bloom would never select before any read, store
    // key or Enso request is made for it.
    let wallet = crate::wallet::validate_id(account.wallet()).map_err(CreateError::InvalidInput)?;
    let now = host.now_ms();
    let parsed = input::parse_new_body(body).map_err(CreateError::InvalidInput)?;
    let address = wallet_address(host, account)?;
    let api_key = resolve_api_key(host)?;

    // Determine source chain. It must be named: a guessed chain would quote
    // the swap on a network the caller never chose.
    let nat = input::parse_natural_intent(&parsed.intent).ok_or_else(|| {
        CreateError::InvalidInput(format!(
            "could not parse intent '{}' (expected `swap <amount> <tok> to <tok> on <chain>`)",
            parsed.intent
        ))
    })?;
    let chain_name = parsed
        .chain
        .as_deref()
        .or(nat.chain.as_deref())
        .ok_or_else(|| {
            CreateError::InvalidInput(
                "name the source chain: add \"chain\" to the JSON body \
                 (for example {\"intent\":\"swap 0.003 ETH to USDC\",\"chain\":\"base\"}) \
                 or end the intent with `on <chain>`"
                    .into(),
            )
        })?;
    let chain_name = input::canonical_chain(chain_name);

    // Resolve the configured RPC and prove that it is the requested chain.
    let chain_id = host
        .chain_id(&chain_name)
        .map_err(|e| format!("cannot verify source chain {chain_name}: {e}"))?;

    // Resolve token_in on source chain.
    let token_in = input::resolve_token_symbol(chain_id, &nat.token_in).ok_or_else(|| {
        CreateError::InvalidInput(format!("could not resolve token symbol: {}", nat.token_in))
    })?;

    // Determine destination chain.
    let destination_chain = parsed
        .destination_chain
        .as_ref()
        .map(|chain| input::canonical_chain(chain));
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
        input::resolve_token_symbol(resolve_chain_id, &nat.token_out).ok_or_else(|| {
            CreateError::InvalidInput(format!("could not resolve token symbol: {}", nat.token_out))
        })?
    };

    // Resolve decimals for token_in.
    let token_in_hex = format!("0x{:x}", token_in);
    let decimals = if token_in_hex.eq_ignore_ascii_case(NATIVE_TOKEN) {
        18u8
    } else if nat.token_in.starts_with("0x") || nat.token_in.starts_with("0X") {
        host.erc20_decimals(&chain_name, &token_in_hex)
            .map_err(|e| format!("cannot read token decimals: {e}"))?
    } else {
        input::decimals_for_symbol(chain_id, &nat.token_in).ok_or_else(|| {
            CreateError::InvalidInput(format!("unknown decimals for token: {}", nat.token_in))
        })?
    };

    // Parse amount with correct decimals.
    let amount_raw = parse_amount(&nat.amount, decimals).map_err(CreateError::InvalidInput)?;

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
            .map_err(|_| CreateError::InvalidInput(format!("invalid receiver address: {recv}")))?;
        route_req.receiver = Some(addr);
    }

    // Call Enso route API.
    let mut route_resp = api::route(host, &api_key, &route_req)?;

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
    crate::calldata::bind_output_checks(&mut route_resp, &route_req)?;
    // Store one effective floor before showing the plan. Replacement quotes
    // must never become a new baseline for slippage.
    route_req.min_amount_out = Some(
        crate::quote::reviewed_minimum_output(&route_resp, &route_req)?
            .max(crate::calldata::output_minimum(&route_resp, &route_req).unwrap_or(U256::ZERO)),
    );

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

    // Load and enforce the Petal-owned Enso venue preferences. Bloom applies
    // authoritative wallet policy independently when staging.
    let protocols = route_resp.protocols();
    let verified_policy = crate::policy::load_venue_config(host, wallet)?;
    let output_verified = crate::calldata::verify_output_checks(&route_resp, &route_req)?;
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
            receiver_verified: output_verified,
            min_out_enforced: output_verified,
            needs_approve,
        },
    );
    if let Some(reason) = crate::policy::deny_reason(&policy_checks) {
        return Err(reason.into());
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
            return Err(CreateError::Failed(format!(
                "route simulation failed: {}",
                crate::simulation::failure_message(&result)
            )));
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
        schema_version: 2,
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
    crate::wallet::validate_id(wallet)?;
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

/// Written with put-new just before an intent is staged, and removed once the
/// session save that records its outbox id succeeds. A marker beside an
/// intent with no outbox id means a stage may have reached the outbox without
/// being recorded, so that intent must never be staged again, unless the
/// marker records that Bloom refused the stage.
fn staging_marker_key(wallet: &str, id: &str, index: usize) -> String {
    format!("intents/{wallet}/{id}/staging-{index}.json")
}

/// Whether a staging marker records a stage Bloom refused before writing it.
/// A marker that cannot be read as one counts as a possible outbox entry.
fn marker_refused(marker: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(marker)
        .ok()
        .and_then(|value| value.get("refused").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// The first unrecorded intent that an earlier confirm began staging.
fn unrecorded_stage<H: Host>(host: &mut H, sess: &Session) -> Result<Option<usize>, String> {
    for state in &sess.intent_states {
        let key = staging_marker_key(&sess.wallet, &sess.id, state.index);
        if state.outbox_id.is_none()
            && let Some(marker) = host.get(&key, 1024)?
            && !marker_refused(&marker)
        {
            return Ok(Some(state.index));
        }
    }
    Ok(None)
}

/// What to do about an intent transaction that may be in the outbox without
/// its outbox id recorded here. Every such refusal includes this text, and
/// abandon looks for it in `last_error`, so the owner has been told where to
/// look before the intent closes.
fn unrecorded_stage_advice(account: &petal::WalletAccount, intent: &PreparedIntent) -> String {
    format!(
        "this intent's {} transaction may be in the wallet's outbox without being recorded \
         here: review /{}/chains/{}/outbox/pending and cancel it there if it should not be \
         sent. This intent will not stage it again; write `abandon` to close it, and create \
         a new intent only if it was not sent",
        intent.label,
        account.dir(),
        intent.chain,
    )
}

/// The route was quoted for the address the account had at creation, and
/// Bloom picks the sender from the live projection, so refuse once it changes.
fn require_quoted_address<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    sess: &Session,
) -> Result<(), String> {
    let quoted = &sess.wallet_address;
    let current = wallet_address(host, account).map_err(|error| {
        format!("cannot re-read the account's EVM address before staging: {error}")
    })?;
    if !current.eq_ignore_ascii_case(quoted) {
        return Err(format!(
            "account {account} now has EVM address {current}, but this intent was \
             quoted for {quoted}; nothing was staged, create a new intent for this account"
        ));
    }
    Ok(())
}

/// Stage one prepared intent behind its staging marker.
fn stage_marked<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    sess: &Session,
    index: usize,
    intent: &PreparedIntent,
) -> Result<StagedTransaction, String> {
    // Bloom picks the sender as it stages, so check the address last.
    require_quoted_address(host, account, sess)?;
    let tx = EvmTransaction {
        wallet: account.wallet().to_string(),
        chain: intent.chain.clone(),
        to: intent.to.clone(),
        value_wei: intent.value_wei.clone(),
        data_hex: intent.data_hex.clone(),
        nonce: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
    };
    let key = staging_marker_key(&sess.wallet, &sess.id, index);
    // A marker left by a refused stage proves nothing reached the outbox.
    // Remove it before staging; until that works, this retry stages nothing.
    if let Some(existing) = host.get(&key, 1024)?
        && marker_refused(&existing)
    {
        host.delete_if(&key, &existing).map_err(|error| {
            format!(
                "cannot clear the record of a stage Bloom refused, so nothing was staged; \
                 write `confirm` again: {error}"
            )
        })?;
    }
    let mut marker_value = serde_json::json!({
        "created_ms": host.now_ms(),
        "chain": tx.chain,
        "to": tx.to,
    });
    let marker = json(&marker_value)?;
    host.put_new(&key, &marker, false).map_err(|error| {
        format!("cannot record that staging began, so nothing was staged: {error}")
    })?;
    match host.tx_stage(&tx) {
        Ok(staged) => Ok(staged),
        Err(StageError::Refused(error)) => {
            // Nothing was written, so a retry cannot duplicate it. If the
            // marker cannot be removed, record the refusal in it instead, so
            // the next stage removes it. A marker left unchanged fails closed.
            if host.delete_if(&key, &marker).is_err() {
                marker_value["refused"] = true.into();
                if let Ok(refused) = json(&marker_value) {
                    let _ = host.put(&key, &refused, false);
                }
            }
            Err(error)
        }
        // The marker stays, so no later confirm stages this intent again.
        Err(StageError::Uncertain(error)) => Err(format!(
            "{} (staging failed after it may have reached the outbox: {error})",
            unrecorded_stage_advice(account, intent)
        )),
    }
}

/// The stage reached the outbox, but the session that records it was not
/// saved.
fn unrecorded_save_error(
    account: &petal::WalletAccount,
    intent: &PreparedIntent,
    outbox_id: &str,
    error: &str,
) -> String {
    format!(
        "{} (it was staged as outbox entry {outbox_id}, but saving this intent failed: {error})",
        unrecorded_stage_advice(account, intent)
    )
}

/// Remove an intent's staging marker after its outbox id has been saved. One
/// left behind is harmless: a recorded outbox id is what a later confirm
/// checks first.
fn clear_staging_marker<H: Host>(host: &mut H, sess: &Session, index: usize) {
    let key = staging_marker_key(&sess.wallet, &sess.id, index);
    if let Ok(Some(marker)) = host.get(&key, 1024) {
        let _ = host.delete_if(&key, &marker);
    }
}

pub fn confirm<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    id: &str,
    body: &[u8],
) -> Result<(), String> {
    let (lock_key, lock_value) = acquire_confirm_lock(host, account.wallet(), id)?;
    let result = confirm_locked(host, account, id, body);
    record_confirm_result(host, account.wallet(), id, &result);
    let unlock = host.delete_if(&lock_key, &lock_value);
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(format!(
            "transaction was staged but confirmation lock cleanup failed: {error}"
        )),
    }
}

/// A refused confirmation reaches a mounted filesystem only as an I/O error,
/// so keep its reason in the session's `status.json` (`last_error`) where the
/// caller can read it. Best effort: the refusal itself is what matters, and a
/// terminal session keeps the reason it ended with. Successful retries clear
/// stale errors, including the idempotent already-staged path.
fn record_confirm_result<H: Host>(
    host: &mut H,
    wallet: &str,
    id: &str,
    result: &Result<(), String>,
) {
    let Ok(mut sess) = load(host, wallet, id) else {
        return;
    };
    let reason = result
        .as_ref()
        .err()
        .map(|error| crate::redaction::sanitize_message(error));
    if sess.wallet != wallet || sess.terminal() || sess.last_error == reason {
        return;
    }
    sess.last_error = reason;
    let _ = save(host, &sess);
}

fn confirm_locked<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    id: &str,
    body: &[u8],
) -> Result<(), String> {
    let wallet = account.wallet();
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

    // An earlier confirm may have staged an intent and then failed to save
    // its outbox id. Staging it again could send the same transaction twice.
    // Check this first, so no other refusal replaces this advice in
    // `last_error`.
    if let Some(index) = unrecorded_stage(host, &sess)? {
        let intent = sess
            .intents
            .get(index)
            .ok_or("session intent state has no matching intent — refusing")?;
        let reason = format!(
            "{} (an earlier confirm began staging it, so this confirm staged nothing)",
            unrecorded_stage_advice(account, intent)
        );
        sess.last_error = Some(reason.clone());
        save(host, &sess)?;
        return Err(reason);
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
    // Refuse before any quote refresh; `stage_marked` checks again.
    require_quoted_address(host, account, &sess)?;

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

    // Re-read the current Enso venue preferences at the last possible moment.
    // The outbox host independently enforces authoritative wallet policy.
    let needs_approve = sess.intents.iter().any(|i| i.label == "approve");
    let router = format!("0x{:x}", route.tx.to);
    sess.policy_checks = route_policy_checks(host, wallet, &sess, &req, &route, needs_approve)?;
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
            let staged = stage_marked(host, account, &sess, 0, approve)?;
            let outbox_id = staged.outbox_id.clone();
            if let Some(state) = sess.intent_states.first_mut() {
                state.status = "staged".into();
                state.outbox_id = Some(staged.outbox_id.clone());
                state.approval = staged.approval.as_ref().map(|approval| {
                    serde_json::json!({
                        "action_id": approval.action_id,
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
            save(host, &sess)
                .map_err(|error| unrecorded_save_error(account, approve, &outbox_id, &error))?;
            clear_staging_marker(host, &sess, 0);
            return Ok(());
        }

        let approval_id = approve_state.outbox_id.clone().unwrap();
        let inspection = host
            .tx_inspect(wallet, &approve.chain, &approval_id)
            .map_err(|error| error.message())?;
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

    // Bloom refuses an Enso quote older than five minutes on every confirm,
    // including the one after the owner's approval ceremony. Replace an aged
    // quote now so that window starts at staging, within the reviewed bounds.
    let route = if crate::quote::needs_refresh(&route, now) {
        match refresh_route_quote(host, wallet, &sess, &req, &route, needs_approve, now) {
            Ok((refreshed, fresh)) => {
                sess = refreshed;
                save(host, &sess)?;
                fresh
            }
            Err(error) => {
                let reason = match error {
                    RefreshError::Unavailable(reason) => {
                        format!("{reason}; nothing was staged, write `confirm` again to retry")
                    }
                    RefreshError::OutOfBounds(reason) => format!(
                        "{reason}; abandon this intent and create a new one for a fresh route"
                    ),
                };
                sess.last_error = Some(reason.clone());
                save(host, &sess)?;
                return Err(reason);
            }
        }
    } else {
        route
    };
    let intents = sess.intents.clone();

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
        update_plan(&mut sess)?;
        sess.last_error = Some(reason.clone());
        save(host, &sess)?;
        return Err(reason);
    }
    sess.simulation = Some(simulation);
    update_plan(&mut sess)?;

    let route_index = intents.len() - 1;
    let intent = &intents[route_index];
    let mut staged_route = None;
    if sess
        .intent_states
        .get(route_index)
        .and_then(|state| state.outbox_id.as_ref())
        .is_none()
    {
        // Record this attempt's outcome, so the session never shows an
        // earlier error or a missing simulation after staging fails.
        let staged = match stage_marked(host, account, &sess, route_index, intent) {
            Ok(staged) => staged,
            Err(error) => {
                let reason = format!("staging the swap failed: {error}");
                sess.last_error = Some(reason.clone());
                save(host, &sess)?;
                return Err(reason);
            }
        };

        if let Some(state) = sess.intent_states.get_mut(route_index) {
            state.status = "staged".into();
            state.outbox_id = Some(staged.outbox_id.clone());
            state.approval = staged.approval.as_ref().map(|approval| {
                serde_json::json!({
                    "action_id": approval.action_id,
                    "expires_ms": approval.expires_ms,
                })
            });
            state.updated_ms = now;
        }
        sess.staged_ids.push(staged.outbox_id.clone());
        staged_route = Some(staged.outbox_id);
    }

    sess.last_error = None;
    sess.transition(now, "staged", "route staged into outbox");
    save(host, &sess).map_err(|error| match &staged_route {
        Some(outbox_id) => unrecorded_save_error(account, intent, outbox_id, &error),
        None => error,
    })?;
    clear_staging_marker(host, &sess, route_index);
    Ok(())
}

/// Evaluate the wallet's Enso venue preferences for one route.
fn route_policy_checks<H: Host>(
    host: &mut H,
    wallet: &str,
    sess: &Session,
    req: &RouteRequest,
    route: &RouteResponse,
    needs_approve: bool,
) -> Result<serde_json::Value, String> {
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
    let verified_policy = crate::policy::load_venue_config(host, wallet)?;
    let output_verified = crate::calldata::verify_output_checks(route, req)?;
    Ok(crate::policy::evaluate(
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
            receiver_verified: output_verified,
            min_out_enforced: output_verified,
            needs_approve,
        },
    ))
}

/// Why a quote refresh did not stage. A temporary failure to reach Enso or
/// the host is worth retrying; a route outside the reviewed bounds is not.
enum RefreshError {
    Unavailable(String),
    OutOfBounds(String),
}

/// Fetch a fresh Enso quote for the stored request and return the session
/// rewritten to it. Enso is asked for the reviewed minimum output directly,
/// and the fresh route must stay within the reviewed bounds (request, router,
/// native value, minimum output) and pass the venue preferences again;
/// otherwise nothing changes.
fn refresh_route_quote<H: Host>(
    host: &mut H,
    wallet: &str,
    sess: &Session,
    req: &RouteRequest,
    reviewed: &RouteResponse,
    needs_approve: bool,
    now: u64,
) -> Result<(Session, RouteResponse), RefreshError> {
    if sess.schema_version < 2 || req.min_amount_out.is_none() {
        return Err(RefreshError::OutOfBounds(
            "the original reviewed floor is unavailable; a new review is required".into(),
        ));
    }
    let api_key = resolve_api_key(host).map_err(RefreshError::Unavailable)?;
    // The floor stored at review is the only baseline; a replacement quote
    // never raises or lowers it.
    let refresh_request = req.clone();
    let mut fresh = api::route(host, &api_key, &refresh_request).map_err(|error| {
        RefreshError::Unavailable(format!(
            "cannot refresh the Enso quote before staging: {error}"
        ))
    })?;
    crate::calldata::bind_output_checks(&mut fresh, &refresh_request)
        .map_err(RefreshError::OutOfBounds)?;
    crate::quote::verify_refreshed(reviewed, &fresh, req).map_err(RefreshError::OutOfBounds)?;
    let policy_checks = route_policy_checks(host, wallet, sess, req, &fresh, needs_approve)
        .map_err(RefreshError::Unavailable)?;
    if let Some(reason) = crate::policy::deny_reason(&policy_checks) {
        return Err(RefreshError::OutOfBounds(format!(
            "the refreshed Enso route fails venue policy: {reason}"
        )));
    }
    let mut refreshed = sess.clone();
    let route_intent = refreshed
        .intents
        .last_mut()
        .filter(|intent| intent.label == "route")
        .ok_or_else(|| {
            RefreshError::OutOfBounds("session route intent is missing or out of order".into())
        })?;
    route_intent.data_hex = format!("0x{}", hex::encode(&fresh.tx.data));
    refreshed.route = Some(fresh.clone());
    refreshed.route_request = Some(refresh_request);
    refreshed.policy_checks = policy_checks;
    refreshed.simulation = None;
    update_plan(&mut refreshed).map_err(RefreshError::OutOfBounds)?;
    verify_prepared_intents(&refreshed, req, &fresh).map_err(RefreshError::OutOfBounds)?;
    let state = refreshed.state.clone();
    refreshed.transition(
        now,
        &state,
        &format!(
            "Enso quote refreshed before staging: quoted output {} (reviewed {})",
            fresh.amount_out, reviewed.amount_out
        ),
    );
    Ok((refreshed, fresh))
}

// ---------------------------------------------------------------------------
// abandon — cancel a session before any outbox transaction exists
// ---------------------------------------------------------------------------

pub fn abandon<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    id: &str,
) -> Result<(), String> {
    abandon_with_body(host, account, id, b"abandon")
}

pub fn abandon_with_body<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    id: &str,
    body: &[u8],
) -> Result<(), String> {
    // Serialize with confirmation before loading the session: an approval-only
    // snapshot must never overwrite a swap staged by a concurrent confirmation.
    let (lock_key, lock_value) = acquire_confirm_lock(host, account.wallet(), id)?;
    let result = abandon_locked(host, account, id, body);
    let unlock = host.delete_if(&lock_key, &lock_value);
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(format!(
            "session was abandoned but confirmation lock cleanup failed: {error}"
        )),
    }
}

fn abandon_locked<H: Host>(
    host: &mut H,
    account: &petal::WalletAccount,
    id: &str,
    body: &[u8],
) -> Result<(), String> {
    let wallet = account.wallet();
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

    if sess.terminal() {
        return Err("session is already terminal".into());
    }

    // The swap itself must not be in the outbox. A finished token approval
    // may be: its exact allowance stays, and a new intent reuses it.
    let mut approval_left = false;
    for (intent, state) in sess.intents.iter().zip(&sess.intent_states) {
        let Some(outbox_id) = state.outbox_id.as_deref() else {
            continue;
        };
        let inspection = host
            .tx_inspect(&sess.wallet, &sess.chain, outbox_id)
            .map_err(|error| error.message())?;
        if intent.label == "route" {
            // Only a swap that will never execute can be abandoned.
            if !matches!(
                inspection.state.as_str(),
                "failed" | "reverted" | "cancelled"
            ) {
                return Err(format!(
                    "the swap {outbox_id} is in the outbox; discard it by writing `cancel` to its `confirm` file, then abandon"
                ));
            }
            continue;
        }
        if !matches!(
            inspection.state.as_str(),
            "success" | "failed" | "reverted" | "cancelled"
        ) {
            return Err(format!(
                "the token approval {outbox_id} is still pending in the outbox; confirm or discard it there, then abandon"
            ));
        }
        approval_left |= inspection.state == "success";
    }

    // A transaction left in the outbox unrecorded must not be forgotten: a new
    // intent would stage the swap again. Refuse until `last_error` has said
    // where to review it, then let the owner close the intent.
    let unrecorded = match unrecorded_stage(host, &sess)? {
        Some(index) => {
            let intent = sess
                .intents
                .get(index)
                .ok_or("session intent state has no matching intent — refusing")?;
            let advice = unrecorded_stage_advice(account, intent);
            let label = intent.label.clone();
            if !sess
                .last_error
                .as_deref()
                .is_some_and(|shown| shown.contains(&advice))
            {
                let reason = format!("{advice} (write `abandon` again once you have reviewed it)");
                sess.last_error = Some(reason.clone());
                save(host, &sess)?;
                return Err(reason);
            }
            Some(label)
        }
        None => None,
    };

    let mut reason = String::from("user abandoned");
    if let Some(label) = &unrecorded {
        reason.push_str(&format!(
            "; its {label} transaction may be in the outbox unrecorded"
        ));
    }
    if approval_left {
        reason.push_str("; the token approval already sent stays and a new intent reuses it");
    } else if unrecorded.is_none() {
        reason.push_str("; no successful token approval remains from this intent");
    }
    sess.transition(now, "abandoned", &reason);
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

    /// Abandon finds this advice in `last_error`, which is cut at 512
    /// characters, so every refusal carrying it must fit there whole.
    #[test]
    fn unrecorded_stage_advice_fits_in_last_error_for_the_longest_account() {
        let account = petal::testing::wallet_account(&"a".repeat(64), u32::MAX);
        let intent = PreparedIntent {
            label: "approve".into(),
            to: String::new(),
            value_wei: "0".into(),
            data_hex: "0x".into(),
            chain: "robinhood".into(),
            approve_token: None,
            approve_spender: None,
        };
        let advice = unrecorded_stage_advice(&account, &intent);
        for refusal in [
            format!("staging the swap failed: {advice}"),
            format!("{advice} (write `abandon` again once you have reviewed it)"),
            format!(
                "{advice} (an earlier confirm began staging it, so this confirm staged nothing)"
            ),
        ] {
            assert!(
                crate::redaction::sanitize_message(&refusal).contains(&advice),
                "{} characters: {refusal}",
                refusal.len()
            );
        }
    }

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
