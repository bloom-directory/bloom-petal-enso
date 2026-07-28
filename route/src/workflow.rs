//! Workflow operations for Enso intent sessions.
//!
//! create → route discovery → simulate → confirm → outbox staging

pub use crate::runtime::{BloomHost, Host};
use crate::{
    api,
    api_types::*,
    input::{self, NewIntentBody},
    session::{self, Session},
    settings,
};
use alloy::primitives::{Address, U256};
use petal::sdk::EvmTransaction;
use sha2::{Digest, Sha256};

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

fn quote_hash(route: &RouteResponse) -> String {
    let mut hasher = Sha256::new();
    hasher.update(route.tx.to.as_slice());
    hasher.update(route.amount_out.as_bytes());
    hasher.update(&route.tx.data);
    hex::encode(hasher.finalize())
}

/// Create a new intent session.
///
/// Parses the intent body, resolves token symbols, calls the Enso route API,
/// optionally simulates, and stores the durable session.
pub fn create<H: Host>(host: &mut H, wallet: &str, body: &[u8]) -> Result<String, String> {
    let now = host.now_ms();
    let parsed = input::parse_new_body(body)?;
    let address = wallet_address(host, wallet)?;
    let api_key = resolve_api_key(host)?;

    // Determine chain.
    let chain_name = parsed
        .chain
        .as_deref()
        .or_else(|| {
            if let Some(ref nat) = input::parse_natural_intent(&parsed.intent) {
                nat.chain.as_deref()
            } else {
                None
            }
        })
        .unwrap_or("ethereum");

    let chain_id = input::chain_to_id(chain_name)
        .ok_or_else(|| format!("unsupported chain: {chain_name}"))?;

    // Parse the natural intent to extract tokens and amount.
    let nat = input::parse_natural_intent(&parsed.intent)
        .ok_or_else(|| {
            format!(
                "could not parse intent '{}' (expected `swap <amount> <tok> to <tok>`)",
                parsed.intent
            )
        })?;

    // Resolve token addresses.
    let token_in = input::resolve_token_symbol(chain_id, &nat.token_in)
        .ok_or_else(|| format!("could not resolve token symbol: {}", nat.token_in))?;
    let token_out = input::resolve_token_symbol(chain_id, &nat.token_out)
        .ok_or_else(|| format!("could not resolve token symbol: {}", nat.token_out))?;

    // Parse amount — accept decimal human amount and scale to token decimals.
    // For simplicity, we treat the amount as a raw decimal string of the
    // smallest unit. A proper implementation would resolve decimals and scale.
    // TODO: implement proper decimal scaling (ETH has 18, USDC has 6, etc.)
    let amount_raw = parse_amount(&nat.amount, chain_id, &nat.token_in)?;

    let from_address: Address = address
        .parse()
        .map_err(|_| format!("invalid wallet address: {address}"))?;

    let mut route_req = RouteRequest::new(from_address, chain_id, token_in, token_out, amount_raw);
    route_req.slippage_bps = parsed.slippage_bps.unwrap_or(50);
    if let Some(ref dest) = parsed.destination_chain {
        if let Some(dest_id) = input::chain_to_id(dest) {
            route_req.destination_chain_id = Some(dest_id);
        }
    }
    if let Some(ref recv) = parsed.receiver {
        if let Ok(addr) = recv.parse::<Address>() {
            route_req.receiver = Some(addr);
        }
    }

    // Call Enso route API.
    let route_resp = api::route(host, &api_key, &route_req)?;

    // Verify route input matches request.
    let route_verified = route_resp.input_matches_request(&route_req);

    let id = generate_id(host)?;

    // Build plan markdown.
    let protocols = route_resp.protocols();
    let protocol_str = if protocols.1 {
        "unknown protocols".to_string()
    } else if protocols.0.is_empty() {
        "no protocol info".to_string()
    } else {
        protocols.0.join(", ")
    };

    let plan_md = format!(
        "# Enso Shortcuts Swap\n\n        The following is Bloom's authoritative transaction plan.\n\n        - Wallet: `{wallet}` (`{address}`)\n        - Chain: `{chain_name}` (chain ID {chain_id})\n        - Intent: `{}`\n        - Input: {} {} → Output: {} ({})\n        - Route verified: {}\n        - Protocols: {protocol_str}\n        - Gas estimate: {}\n        - Price impact (display only): {}\n\n        Write `confirm` to stage this transaction into the wallet outbox.\n",
        parsed.intent,
        nat.amount,
        nat.token_in,
        route_resp.amount_out,
        nat.token_out,
        route_verified,
        route_resp.gas.as_deref().unwrap_or("not estimated"),
        route_resp
            .price_impact
            .map(|v| format!("{v}"))
            .unwrap_or_else(|| "not reported".into()),
    );

    let prepared_tx = PreparedTx {
        to: format!("0x{:x}", route_resp.tx.to),
        value_wei: route_resp.tx.value.to_string(),
        data_hex: format!("0x{}", hex::encode(&route_resp.tx.data)),
        chain: chain_name.to_string(),
    };

    let mut sess = Session {
        schema_version: 1,
        id: id.clone(),
        wallet: wallet.to_string(),
        wallet_address: address,
        created_ms: now,
        updated_ms: now,
        state: "prepared".into(),
        intent_text: parsed.intent.clone(),
        chain: chain_name.to_string(),
        destination_chain: parsed.destination_chain.clone(),
        request_body: parsed,
        route: Some(route_resp),
        route_verified,
        simulation: None,
        prepared_tx: Some(prepared_tx),
        outbox_id: None,
        outbox_state: None,
        tx_hash: None,
        plan_md: Some(plan_md),
        last_error: None,
        history: vec![],
    };
    sess.transition(now, "prepared", "route discovered");

    save(host, &sess)?;
    Ok(id)
}

/// Confirm: stage the prepared transaction into the outbox.
pub fn confirm<H: Host>(
    host: &mut H,
    wallet: &str,
    id: &str,
    _body: &[u8],
) -> Result<(), String> {
    let now = host.now_ms();
    let mut sess = load(host, wallet, id)?;

    if sess.state == "staged" || sess.state == "confirmed" {
        return Ok(());
    }

    let prepared = sess
        .prepared_tx
        .as_ref()
        .ok_or_else(|| "no prepared transaction to confirm".to_string())?;

    let evm_tx = EvmTransaction {
        wallet: wallet.to_string(),
        chain: prepared.chain.clone(),
        to: prepared.to.clone(),
        value_wei: prepared.value_wei.clone(),
        data_hex: prepared.data_hex.clone(),
        nonce: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
    };

    let staged = host.tx_stage(&evm_tx)?;
    sess.outbox_id = Some(staged.outbox_id.clone());
    sess.outbox_state = Some("staged".into());
    if let Some(ref plan) = staged.plan_md {
        // The outbox may provide an updated plan; prefer the outbox version.
        sess.plan_md = Some(plan.clone());
    }
    sess.transition(now, "staged", "transaction staged into outbox");

    save(host, &sess)?;
    Ok(())
}

/// Parse a human-readable amount into raw wei/units.
/// TODO: implement proper decimal scaling based on token decimals.
/// For now, accepts decimal strings and multiplies by 10^18 for native tokens.
fn parse_amount(amount: &str, _chain_id: u64, token_in: &str) -> Result<U256, String> {
    let trimmed = amount.trim();
    if trimmed.is_empty() {
        return Err("amount is empty".into());
    }

    // If it's already a raw integer, parse directly.
    if let Ok(v) = U256::from_str_radix(trimmed, 10) {
        // Check if it contains a decimal point — if so, scale.
        if !trimmed.contains('.') {
            return Ok(v);
        }
    }

    // Parse decimal amount: split on '.'
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() > 2 {
        return Err(format!("invalid amount: {trimmed}"));
    }

    let whole = parts[0];
    let frac = if parts.len() == 2 { parts[1] } else { "" };

    // Default decimals: 18 for native ETH-like tokens, 6 for stablecoins.
    let decimals = match token_in.to_ascii_uppercase().as_str() {
        "USDC" | "USDT" => 6,
        "WBTC" => 8,
        _ => 18,
    };

    let whole_val = if whole.is_empty() {
        U256::from(0u64)
    } else {
        U256::from_str_radix(whole, 10)
            .map_err(|_| format!("invalid whole part: {whole}"))?
    };

    let scaled_whole = whole_val
        * U256::from(10u64).pow(U256::from(decimals));

    let frac_val = if frac.is_empty() {
        U256::from(0u64)
    } else {
        let padded = format!("{:0<decimals$}", frac, decimals = decimals);
        let trimmed_padded = padded[..decimals.min(padded.len())].to_string();
        U256::from_str_radix(&trimmed_padded, 10)
            .map_err(|_| format!("invalid fractional part: {frac}"))?
    };

    Ok(scaled_whole + frac_val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_amount() {
        let v = parse_amount("100", 1, "USDC").unwrap();
        assert_eq!(v, U256::from(100u64));
    }

    #[test]
    fn parses_decimal_eth_amount() {
        let v = parse_amount("1.5", 1, "ETH").unwrap();
        assert_eq!(v, U256::from(1_500_000_000_000_000_000u128));
    }

    #[test]
    fn parses_decimal_usdc_amount() {
        let v = parse_amount("100.5", 1, "USDC").unwrap();
        assert_eq!(v, U256::from(100_500_000u64));
    }
}
