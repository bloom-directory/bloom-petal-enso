//! Input parsing for new intent creation.

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

/// Resolve a token symbol or address into a concrete [`Address`] for a chain.
/// Symbol set is intentionally tiny — the registry can be expanded.
pub fn resolve_token_symbol(chain_id: u64, sym: &str) -> Option<Address> {
    let s = sym.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        return s.parse::<Address>().ok();
    }
    let upper = s.to_ascii_uppercase();
    if matches!(upper.as_str(), "ETH" | "ETHER" | "MATIC" | "BNB" | "AVAX") {
        return NATIVE_TOKEN.parse().ok();
    }
    // TODO: expand the token registry. The bloom monorepo had bloom_proto::tokens
    // for this; in the petal we should read from tokens.json or an on-chain registry.
    match (chain_id, upper.as_str()) {
        (1, "USDC") => "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".parse().ok(),
        (1, "USDT") => "0xdac17f958d2ee523a2206206994597c13d831ec7".parse().ok(),
        (1, "WETH") => "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".parse().ok(),
        (137, "USDC") => "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359".parse().ok(),
        (137, "USDT") => "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".parse().ok(),
        (137, "WMATIC") => "0x0d500b1d8e8ef31e21c99d1db9a6444d3adf1270".parse().ok(),
        (8453, "USDC") => "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().ok(),
        (10, "USDC") => "0x0b2c639c533813f4aa9d7837caf62653d097ff85".parse().ok(),
        _ => None,
    }
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
}
