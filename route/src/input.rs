//! Input parsing for new intent creation.
//!
//! This module owns the static token symbol table for the enso petal. In the
//! original bloom monorepo this registry came from `bloom_proto::tokens`; here
//! we keep a curated, hand-verified table covering the major tokens across the
//! seven supported chains. Addresses are checksum-agnostic — `alloy` accepts
//! lowercase hex and normalizes on parse.

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

use crate::api_types::NATIVE_TOKEN;

/// Body of `new` writes — accepts either a JSON object or plain NL text.
#[derive(Debug, Clone, Deserialize)]
pub struct NewIntentBody {
    #[serde(default)]
    #[allow(dead_code)]
    pub kind: Option<String>,
    pub intent: String,
    #[serde(default)]
    pub chain: Option<String>,
    #[serde(default)]
    pub destination_chain: Option<String>,
    #[serde(default)]
    pub receiver: Option<String>,
    #[serde(default)]
    pub slippage_bps: Option<u16>,
}

/// Parse the write body for `intents/<wallet>/new`.
/// Accepts JSON `{intent, chain, ...}` or bare NL text.
pub fn parse_new_body(body: &[u8]) -> Result<NewIntentBody, String> {
    let text = std::str::from_utf8(body)
        .map_err(|_| "intent body must be UTF-8")?
        .trim();

    if text.is_empty() {
        return Err("empty intent body".into());
    }

    if text.starts_with('{') {
        serde_json::from_str::<NewIntentBody>(text)
            .map_err(|e| format!("invalid intent JSON: {e}"))
            .and_then(|mut b| {
                if b.intent.trim().is_empty() {
                    return Err("missing 'intent' field".into());
                }
                b.intent = b.intent.trim().to_string();
                Ok(b)
            })
    } else {
        Ok(NewIntentBody {
            kind: None,
            intent: text.to_string(),
            chain: None,
            destination_chain: None,
            receiver: None,
            slippage_bps: None,
        })
    }
}

/// A parsed natural-language swap intent.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct NaturalIntent {
    pub verb: String,
    pub amount: String,
    pub token_in: String,
    pub token_out: String,
    pub chain: Option<String>,
}

/// Parse `<verb> <amount> <token_in> to <token_out> [on <chain>]`.
pub fn parse_natural_intent(input: &str) -> Option<NaturalIntent> {
    let toks: Vec<&str> = input.split_whitespace().collect();
    if toks.len() < 5 {
        return None;
    }
    if !toks[3].eq_ignore_ascii_case("to") {
        return None;
    }
    if !toks[1]
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        return None;
    }
    let chain = if toks.len() >= 7 && toks[5].eq_ignore_ascii_case("on") {
        Some(toks[6].to_string())
    } else {
        None
    };
    Some(NaturalIntent {
        verb: toks[0].to_ascii_lowercase(),
        amount: toks[1].to_string(),
        token_in: toks[2].to_string(),
        token_out: toks[4].to_string(),
        chain,
    })
}

/// Returns true when `upper` is an alias for the native gas token on `chain_id`.
///
/// Native aliases are chain-aware so that, e.g., `MATIC` resolves to the native
/// token on Polygon but to the bridged MATIC ERC-20 on Ethereum.
fn is_native_alias(chain_id: u64, upper: &str) -> bool {
    match chain_id {
        // Ethereum and the EVM-equivalent L2s use ETH as gas.
        1 | 8453 | 10 | 42161 => matches!(upper, "ETH" | "ETHER" | "NATIVE"),
        // Polygon.
        137 => matches!(upper, "MATIC" | "POL" | "NATIVE"),
        // BNB Chain.
        56 => matches!(upper, "BNB" | "NATIVE"),
        // Avalanche.
        43114 => matches!(upper, "AVAX" | "NATIVE"),
        _ => matches!(upper, "NATIVE"),
    }
}

/// Resolve a token symbol or address into a concrete [`Address`] for a chain.
///
/// The static registry covers the major tokens across all seven supported
/// chains (Ethereum, Polygon, Base, Optimism, Arbitrum, BNB Chain, Avalanche).
/// A bare `0x` address is parsed directly. Native gas-token aliases (ETH, MATIC,
/// BNB, AVAX) resolve to [`NATIVE_TOKEN`] on their home chain.
pub fn resolve_token_symbol(chain_id: u64, sym: &str) -> Option<Address> {
    let s = sym.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        return s.parse::<Address>().ok();
    }
    let upper = s.to_ascii_uppercase();

    if is_native_alias(chain_id, &upper) {
        return NATIVE_TOKEN.parse().ok();
    }

    // Expanded static token registry. Grouped by chain for readability.
    match (chain_id, upper.as_str()) {
        // ── Ethereum (chain 1) ────────────────────────────────────────────
        (1, "USDC") => "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".parse().ok(),
        (1, "USDT") => "0xdac17f958d2ee523a2206206994597c13d831ec7".parse().ok(),
        (1, "DAI") => "0x6b175474e89094c44da98b954eedeac495271d0f".parse().ok(),
        (1, "WETH") => "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".parse().ok(),
        (1, "WBTC") => "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599".parse().ok(),
        (1, "LINK") => "0x514910771af9ca656af840dff83e8264ecf986ca".parse().ok(),
        (1, "UNI") => "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984".parse().ok(),
        (1, "AAVE") => "0x7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9".parse().ok(),
        (1, "MKR") => "0x9f8f72aa9304c8b593d555f12ef6589cc3a579a2".parse().ok(),
        (1, "SNX") => "0xc011a73ee8576fb46f5e1c5751ca3b9fe0af2a6f".parse().ok(),
        (1, "CRV") => "0xd533a949740bb3306d119cc777fa900ba034cd52".parse().ok(),
        (1, "LDO") => "0x5a98fcbea516cf06857215779fd812ca3bef1b32".parse().ok(),
        (1, "MATIC") => "0x7d1afa7b718fb893db30a3abc0cfc608aacfebb0".parse().ok(),
        (1, "SHIB") => "0x95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce".parse().ok(),
        (1, "PEPE") => "0x6982508145454ce325ddbe47a25d4ec3d2311933".parse().ok(),
        (1, "ENS") => "0xc183442df006c69b9695c1f4e7b6f364ba85e07c".parse().ok(),
        (1, "WSTETH") => "0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0".parse().ok(),
        (1, "RPL") => "0xd33526068d116ce69f19a9ee46f0bd304f21a51f".parse().ok(),
        (1, "GMX") => "0xfc5a1a6eb076a2c7ad06ed42c24598413edd782d".parse().ok(),
        (1, "FXS") => "0x3432b6a60d23ca0dfca7761b7ab56459d9c964d0".parse().ok(),

        // ── Polygon (chain 137) ───────────────────────────────────────────
        (137, "USDC") => "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359".parse().ok(),
        (137, "USDT") => "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".parse().ok(),
        (137, "DAI") => "0x8f3cf7ad23cd3cadbd9735aff958023239c6a063".parse().ok(),
        (137, "WETH") => "0x7ceb23fd6bc0add59e62ac25578270cff1b9f619".parse().ok(),
        (137, "WMATIC") => "0x0d500b1d8e8ef31e21c99d1db9a6444d3adf1270".parse().ok(),
        (137, "WBTC") => "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6".parse().ok(),
        (137, "LINK") => "0x53e0bca35ec356bd5dddfebbd1fc0fd03fabad39".parse().ok(),
        (137, "AAVE") => "0xd6df932a45c0f255f85145f286ea0b292b21c90b".parse().ok(),
        (137, "CRV") => "0x172370d5cd63279efa6d502dab29171933a610af".parse().ok(),
        (137, "SUSHI") => "0x0b3f868e0be5597d5db7feb59e1cadbb0fdda50a".parse().ok(),

        // ── Base (chain 8453) ─────────────────────────────────────────────
        (8453, "USDC") => "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().ok(),
        (8453, "WETH") => "0x4200000000000000000000000000000000000006".parse().ok(),
        // Bridged mainnet DAI; Base has no canonical native DAI deployment.
        (8453, "DAI") => "0x6b175474e89094c44da98b954eedeac495271d0f".parse().ok(),
        (8453, "CBETH") => "0x2ae3f1ec7f1f5012cfeab0185bfc7aa3cf0dec22".parse().ok(),
        (8453, "DEGEN") => "0x4ed4e862860bed51a9570b96d89af5e1b0efefed".parse().ok(),

        // ── Optimism (chain 10) ───────────────────────────────────────────
        (10, "USDC") => "0x0b2c639c533813f4aa9d7837caf62653d097ff85".parse().ok(),
        (10, "USDT") => "0x94b008aa00579c1307b0ef2c499ad98a8ce58e58".parse().ok(),
        (10, "DAI") => "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1".parse().ok(),
        (10, "WETH") => "0x4200000000000000000000000000000000000006".parse().ok(),
        (10, "WBTC") => "0x68f180fcce6836688e9084f035309e29bf0a2095".parse().ok(),
        (10, "LINK") => "0x350a79107fc7c24f8f9f897a08b3b57cfc9afe7f".parse().ok(),
        (10, "OP") => "0x4200000000000000000000000000000000000042".parse().ok(),

        // ── Arbitrum (chain 42161) ────────────────────────────────────────
        (42161, "USDC") => "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().ok(),
        (42161, "USDT") => "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9".parse().ok(),
        (42161, "DAI") => "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1".parse().ok(),
        (42161, "WETH") => "0x82af49447d8a07e3bd95bd0d56f35241523fbab1".parse().ok(),
        (42161, "WBTC") => "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f".parse().ok(),
        (42161, "ARB") => "0x912ce59144191c1204e64559fe8253a0e49e6548".parse().ok(),
        (42161, "LINK") => "0xf97f4df75117a78c1a5a0dbb814af92458539fb4".parse().ok(),
        (42161, "GMX") => "0xfc5a1a6eb076a2c7ad06ed42c24598413edd782d".parse().ok(),
        (42161, "LDO") => "0x13ad51ed4f1b7e9dc168d8a00cb3f4ddd85efa3e".parse().ok(),

        // ── BNB Chain (chain 56) ──────────────────────────────────────────
        (56, "USDC") => "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d".parse().ok(),
        (56, "USDT") => "0x55d398326f99059ff775485246999027b3197955".parse().ok(),
        (56, "DAI") => "0x1af3f329e8be154074d8769d1ffa4ee058b1dbc3".parse().ok(),
        // BSC's WETH is its own contract, not the Ethereum WETH address.
        (56, "WETH") => "0x2170ed0880ac9a755fd29b2688956bd959f933f8".parse().ok(),
        (56, "CAKE") => "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82".parse().ok(),
        (56, "BUSD") => "0xe9e7cea3dedca5984780bafc599bd69add087d56".parse().ok(),

        // ── Avalanche (chain 43114) ───────────────────────────────────────
        (43114, "USDC") => "0xb97ef9ef8734c71904d8002f8b6bc66dd9c48a6e".parse().ok(),
        (43114, "USDT") => "0x9702230a8ea53601f5cd2dc00fdbc13d69699b33".parse().ok(),
        (43114, "DAI") => "0xd586e7f844cea2f87f50152665bcbc2c279d8d70".parse().ok(),
        (43114, "WAVAX") => "0xb31f66aa3c1e785363f0875a1b74e27b85fd66c7".parse().ok(),
        (43114, "WETH") => "0x49d5c2bdffac6ce2bfdb6640f4f80f226bc10bab".parse().ok(),
        (43114, "LINK") => "0x5947bb275c521040051d823b1c7de035f1827efb".parse().ok(),

        _ => None,
    }
}

/// Common token decimals for a symbol on a chain.
///
/// Defaults to the dominant EVM standard of 18. Stablecoins USDC and USDT use 6,
/// DAI uses 18 (it is an 18-decimal token, *not* 6), and WBTC uses 8. The
/// `chain_id` is accepted so callers can special-case chain-specific quirks;
/// the symbol-based map below reflects the canonical values across our table.
pub fn decimals_for_symbol(chain_id: u64, symbol: &str) -> u8 {
    let upper = symbol.trim().to_ascii_uppercase();
    match upper.as_str() {
        // 6-decimal stablecoins.
        "USDC" | "USDT" => 6,
        // DAI is an 18-decimal token — do NOT group it with the 6-dec stables.
        "DAI" => 18,
        // Bitcoin wraps use 8 decimals.
        "WBTC" | "CBBTC" => 8,
        // Everything else in the registry (WETH, LINK, UNI, AAVE, etc.) is 18.
        _ => {
            // Let chain-specific overrides surface if needed in the future.
            let _ = chain_id;
            18
        }
    }
}

/// Convenience: resolve `symbol` to `(address, chain_id)` on a named chain.
///
/// Returns `None` if the chain name is unknown or the symbol is not present in
/// the registry for that chain. `chain_name` accepts the same aliases as
/// [`chain_to_id`] (e.g. `"eth"`, `"mainnet"`, `"matic"`, `"arb"`).
pub fn resolve_token_on_chain(chain_name: &str, symbol: &str) -> Option<(Address, u64)> {
    let chain_id = chain_to_id(chain_name)?;
    let addr = resolve_token_symbol(chain_id, symbol)?;
    Some((addr, chain_id))
}

/// Chain name to chain ID.
pub fn chain_to_id(name: &str) -> Option<u64> {
    match name.to_ascii_lowercase().as_str() {
        "ethereum" | "mainnet" | "eth" => Some(1),
        "polygon" | "matic" => Some(137),
        "base" => Some(8453),
        "optimism" | "op" => Some(10),
        "arbitrum" | "arb" => Some(42161),
        "bnb" | "bsc" => Some(56),
        "avalanche" | "avax" => Some(43114),
        _ => None,
    }
}

/// Chain ID to the bloom chain name.
pub fn chain_id_to_name(id: u64) -> Option<&'static str> {
    match id {
        1 => Some("ethereum"),
        137 => Some("polygon"),
        8453 => Some("base"),
        10 => Some("optimism"),
        42161 => Some("arbitrum"),
        56 => Some("bnb"),
        43114 => Some("avalanche"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swap_intent() {
        let got = parse_natural_intent("swap 1 ETH to USDC").unwrap();
        assert_eq!(got.verb, "swap");
        assert_eq!(got.amount, "1");
        assert_eq!(got.token_in, "ETH");
        assert_eq!(got.token_out, "USDC");
        assert!(got.chain.is_none());
    }

    #[test]
    fn parses_swap_with_chain() {
        let got = parse_natural_intent("swap 0.5 ETH to USDC on ethereum").unwrap();
        assert_eq!(got.chain.as_deref(), Some("ethereum"));
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse_natural_intent("hello world").is_none());
        assert!(parse_natural_intent("swap ETH to USDC").is_none());
        assert!(parse_natural_intent("swap 1 ETH into USDC").is_none());
        assert!(parse_natural_intent("").is_none());
    }

    #[test]
    fn resolves_native_and_known_symbols() {
        assert_eq!(
            resolve_token_symbol(1, "ETH").unwrap(),
            NATIVE_TOKEN.parse::<Address>().unwrap()
        );
        assert!(resolve_token_symbol(1, "USDC").is_some());
        assert!(resolve_token_symbol(1, "FOOBAR").is_none());
    }

    #[test]
    fn parses_json_body() {
        let body = br#"{"intent":"swap 100 usdc to eth","chain":"ethereum"}"#;
        let parsed = parse_new_body(body).unwrap();
        assert_eq!(parsed.intent, "swap 100 usdc to eth");
        assert_eq!(parsed.chain.as_deref(), Some("ethereum"));
    }

    #[test]
    fn parses_plain_text_body() {
        let body = b"swap 1 eth to usdc";
        let parsed = parse_new_body(body).unwrap();
        assert_eq!(parsed.intent, "swap 1 eth to usdc");
        assert!(parsed.chain.is_none());
    }

    // ── Expanded registry coverage ───────────────────────────────────────

    #[test]
    fn ethereum_registry_is_complete() {
        let expected = [
            "USDC", "USDT", "DAI", "WETH", "WBTC", "LINK", "UNI", "AAVE", "MKR",
            "SNX", "CRV", "LDO", "MATIC", "SHIB", "PEPE", "ENS", "WSTETH", "RPL",
            "GMX", "FXS",
        ];
        for sym in expected {
            assert!(
                resolve_token_symbol(1, sym).is_some(),
                "ethereum token {sym} should resolve"
            );
        }
    }

    #[test]
    fn polygon_registry_is_complete() {
        let expected = [
            "USDC", "USDT", "DAI", "WETH", "WMATIC", "WBTC", "LINK", "AAVE", "CRV",
            "SUSHI",
        ];
        for sym in expected {
            assert!(
                resolve_token_symbol(137, sym).is_some(),
                "polygon token {sym} should resolve"
            );
        }
    }

    #[test]
    fn base_registry_is_complete() {
        for sym in ["USDC", "WETH", "DAI", "CBETH", "DEGEN"] {
            assert!(
                resolve_token_symbol(8453, sym).is_some(),
                "base token {sym} should resolve"
            );
        }
    }

    #[test]
    fn optimism_registry_is_complete() {
        for sym in ["USDC", "USDT", "DAI", "WETH", "WBTC", "LINK", "OP"] {
            assert!(
                resolve_token_symbol(10, sym).is_some(),
                "optimism token {sym} should resolve"
            );
        }
    }

    #[test]
    fn arbitrum_registry_is_complete() {
        for sym in ["USDC", "USDT", "DAI", "WETH", "WBTC", "ARB", "LINK", "GMX", "LDO"] {
            assert!(
                resolve_token_symbol(42161, sym).is_some(),
                "arbitrum token {sym} should resolve"
            );
        }
    }

    #[test]
    fn bnb_registry_is_complete() {
        for sym in ["USDC", "USDT", "DAI", "WETH", "CAKE", "BUSD"] {
            assert!(
                resolve_token_symbol(56, sym).is_some(),
                "bnb token {sym} should resolve"
            );
        }
    }

    #[test]
    fn avalanche_registry_is_complete() {
        for sym in ["USDC", "USDT", "DAI", "WAVAX", "WETH", "LINK"] {
            assert!(
                resolve_token_symbol(43114, sym).is_some(),
                "avalanche token {sym} should resolve"
            );
        }
    }

    #[test]
    fn native_aliases_are_chain_aware() {
        // ETH is native on ethereum and the EVM L2s.
        for chain in [1u64, 8453, 10, 42161] {
            assert_eq!(
                resolve_token_symbol(chain, "ETH").unwrap(),
                NATIVE_TOKEN.parse::<Address>().unwrap(),
                "ETH native on chain {chain}"
            );
        }
        // MATIC is native on Polygon…
        assert_eq!(
            resolve_token_symbol(137, "MATIC").unwrap(),
            NATIVE_TOKEN.parse::<Address>().unwrap()
        );
        // …but a bridged ERC-20 token on Ethereum.
        let matic_on_eth = resolve_token_symbol(1, "MATIC").unwrap();
        assert_ne!(matic_on_eth, NATIVE_TOKEN.parse::<Address>().unwrap());
        assert_eq!(
            matic_on_eth,
            "0x7d1afa7b718fb893db30a3abc0cfc608aacfebb0"
                .parse::<Address>()
                .unwrap()
        );
        // BNB / AVAX are native on their home chains.
        assert_eq!(
            resolve_token_symbol(56, "BNB").unwrap(),
            NATIVE_TOKEN.parse::<Address>().unwrap()
        );
        assert_eq!(
            resolve_token_symbol(43114, "AVAX").unwrap(),
            NATIVE_TOKEN.parse::<Address>().unwrap()
        );
    }

    #[test]
    fn bare_address_is_parsed_directly() {
        let addr = "0x00000000219ab540356cBB839Cbe05303d7705Fa";
        let got = resolve_token_symbol(1, addr).unwrap();
        assert_eq!(
            got,
            "0x00000000219ab540356cbb839cbe05303d7705fa"
                .parse::<Address>()
                .unwrap()
        );
    }

    #[test]
    fn decimals_for_common_symbols() {
        assert_eq!(decimals_for_symbol(1, "USDC"), 6);
        assert_eq!(decimals_for_symbol(1, "USDT"), 6);
        // DAI is 18, never 6.
        assert_eq!(decimals_for_symbol(1, "DAI"), 18);
        assert_eq!(decimals_for_symbol(42161, "DAI"), 18);
        assert_eq!(decimals_for_symbol(1, "WBTC"), 8);
        // WETH, LINK, UNI, OP, etc. default to 18.
        assert_eq!(decimals_for_symbol(1, "WETH"), 18);
        assert_eq!(decimals_for_symbol(10, "OP"), 18);
        assert_eq!(decimals_for_symbol(1, "LINK"), 18);
        assert_eq!(decimals_for_symbol(1, "PEPE"), 18);
        // Unknown symbols default to 18.
        assert_eq!(decimals_for_symbol(1, "UNKNOWN"), 18);
        // Case-insensitive.
        assert_eq!(decimals_for_symbol(1, "usdc"), 6);
        assert_eq!(decimals_for_symbol(1, "dai"), 18);
    }

    #[test]
    fn resolve_token_on_chain_works() {
        let (addr, chain) = resolve_token_on_chain("ethereum", "USDC").unwrap();
        assert_eq!(chain, 1);
        assert_eq!(
            addr,
            "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
                .parse::<Address>()
                .unwrap()
        );
        // Alias chain names work.
        let (_, c) = resolve_token_on_chain("arb", "ARB").unwrap();
        assert_eq!(c, 42161);
        // Unknown chain / unknown symbol yield None.
        assert!(resolve_token_on_chain("solana", "USDC").is_none());
        assert!(resolve_token_on_chain("ethereum", "NOPE").is_none());
    }

    #[test]
    fn chain_helpers_roundtrip() {
        for id in [1u64, 137, 8453, 10, 42161, 56, 43114] {
            let name = chain_id_to_name(id).unwrap();
            assert_eq!(chain_to_id(name), Some(id));
        }
        assert!(chain_to_id("ethereum").is_some());
        assert!(chain_to_id("mainnet").is_some());
        assert!(chain_to_id("matic").is_some());
        assert!(chain_to_id("bsc").is_some());
        assert!(chain_to_id("nope").is_none());
        assert!(chain_id_to_name(999).is_none());
    }
}
