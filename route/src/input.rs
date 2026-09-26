//! Input parsing for new intent creation.
//!
//! This module owns the static token symbol table for the enso petal. In the
//! original bloom monorepo this registry came from `bloom_proto::tokens`; here
//! we keep a curated, hand-verified table covering the most traded tokens on
//! each of Bloom's default EVM chains except Arc. Addresses are
//! checksum-agnostic — `alloy` accepts lowercase hex and normalizes on parse.

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};

use crate::api_types::NATIVE_TOKEN;

pub const MAX_NEW_BODY_BYTES: usize = 16 * 1024;

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
    if body.is_empty() || body.len() > MAX_NEW_BODY_BYTES {
        return Err(format!(
            "intent body must be 1..={MAX_NEW_BODY_BYTES} bytes"
        ));
    }
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
        // Ethereum and the L2s that use ETH as gas: Base, Optimism, Arbitrum,
        // Linea and Robinhood Chain.
        1 | 8453 | 10 | 42161 | 59144 | 4663 => matches!(upper, "ETH" | "ETHER" | "NATIVE"),
        // Polygon.
        137 => matches!(upper, "MATIC" | "POL" | "NATIVE"),
        // BNB Chain.
        56 => matches!(upper, "BNB" | "NATIVE"),
        // Avalanche.
        43114 => matches!(upper, "AVAX" | "NATIVE"),
        // Gnosis.
        100 => matches!(upper, "XDAI" | "NATIVE"),
        // HyperEVM.
        999 => matches!(upper, "HYPE" | "NATIVE"),
        // Tempo (4217) has no native gas token: fees are paid in TIP-20
        // stablecoins and `eth_getBalance` returns a 0x4242… placeholder, so no
        // symbol aliases the native token there.
        _ => matches!(upper, "NATIVE"),
    }
}

/// One row of the static token registry.
struct TokenEntry {
    chain_id: u64,
    /// Upper-case lookup symbol.
    symbol: &'static str,
    address: &'static str,
    /// The token's on-chain `decimals()`, used to scale symbol amounts.
    decimals: u8,
}

const fn tok(
    chain_id: u64,
    symbol: &'static str,
    address: &'static str,
    decimals: u8,
) -> TokenEntry {
    TokenEntry {
        chain_id,
        symbol,
        address,
        decimals,
    }
}

/// Static token registry, grouped by chain.
///
/// Every row pairs an address with its on-chain `decimals()`, so a symbol can
/// never resolve without a matching amount scale. A plain symbol names the
/// issuer's canonical token on that chain; keep bridged variants out unless the
/// row says otherwise. Verify new rows against an issuer source and an on-chain
/// `symbol()`/`decimals()` read before adding them.
#[rustfmt::skip]
const TOKENS: &[TokenEntry] = &[
    // ── Ethereum (chain 1) ────────────────────────────────────────────────
    tok(1, "USDC", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", 6),
    tok(1, "USDT", "0xdac17f958d2ee523a2206206994597c13d831ec7", 6),
    tok(1, "DAI", "0x6b175474e89094c44da98b954eedeac495271d0f", 18),
    tok(1, "WETH", "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", 18),
    tok(1, "WBTC", "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599", 8),
    tok(1, "CBBTC", "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", 8),
    tok(1, "EURC", "0x1abaea1f7c830bd89acc67ec4af516284b1bc33c", 6),
    tok(1, "USDS", "0xdc035d45d973e3ec169d2276ddab16f1e407384f", 18),
    tok(1, "SUSDS", "0xa3931d71877c0e7a3148cb7eb4463524fec27fbd", 18),
    tok(1, "LINK", "0x514910771af9ca656af840dff83e8264ecf986ca", 18),
    tok(1, "UNI", "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984", 18),
    tok(1, "AAVE", "0x7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9", 18),
    tok(1, "MKR", "0x9f8f72aa9304c8b593d555f12ef6589cc3a579a2", 18),
    tok(1, "SNX", "0xc011a73ee8576fb46f5e1c5751ca3b9fe0af2a6f", 18),
    tok(1, "CRV", "0xd533a949740bb3306d119cc777fa900ba034cd52", 18),
    tok(1, "LDO", "0x5a98fcbea516cf06857215779fd812ca3bef1b32", 18),
    tok(1, "MATIC", "0x7d1afa7b718fb893db30a3abc0cfc608aacfebb0", 18),
    tok(1, "POL", "0x455e53cbb86018ac2b8092fdcd39d8444affc3f6", 18),
    tok(1, "SHIB", "0x95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce", 18),
    tok(1, "PEPE", "0x6982508145454ce325ddbe47a25d4ec3d2311933", 18),
    tok(1, "ENS", "0xc18360217d8f7ab5e7c516566761ea12ce7f9d72", 18),
    tok(1, "WSTETH", "0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0", 18),
    tok(1, "RETH", "0xae78736cd615f374d3085123a210448e74fc6393", 18),
    tok(1, "WEETH", "0xcd5fe23c85820f7b72d0926fc9b05b43e359b7ee", 18),
    tok(1, "CBETH", "0xbe9895146f7af43049ca1c1ae358b0541ea49704", 18),
    tok(1, "RPL", "0xd33526068d116ce69f19a9ee46f0bd304f21a51f", 18),
    tok(1, "FXS", "0x3432b6a60d23ca0dfca7761b7ab56459d9c964d0", 18),
    // ── Polygon (chain 137) ───────────────────────────────────────────────
    tok(137, "USDC", "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359", 6),
    tok(137, "USDT", "0xc2132d05d31c914a87c6611c10748aeb04b58e8f", 6),
    tok(137, "DAI", "0x8f3cf7ad23cd3cadbd9735aff958023239c6a063", 18),
    tok(137, "WETH", "0x7ceb23fd6bc0add59e62ac25578270cff1b9f619", 18),
    tok(137, "WMATIC", "0x0d500b1d8e8ef31e21c99d1db9a6444d3adf1270", 18),
    tok(137, "WBTC", "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6", 8),
    tok(137, "WSTETH", "0x03b54a6e9a984069379fae1a4fc4dbae93b3bccd", 18),
    tok(137, "LINK", "0x53e0bca35ec356bd5dddfebbd1fc0fd03fabad39", 18),
    tok(137, "AAVE", "0xd6df932a45c0f255f85145f286ea0b292b21c90b", 18),
    tok(137, "CRV", "0x172370d5cd63279efa6d502dab29171933a610af", 18),
    tok(137, "SUSHI", "0x0b3f868e0be5597d5db7feb59e1cadbb0fdda50a", 18),
    // ── Base (chain 8453) ─────────────────────────────────────────────────
    tok(8453, "USDC", "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", 6),
    tok(8453, "WETH", "0x4200000000000000000000000000000000000006", 18),
    tok(8453, "DAI", "0x50c5725949a6f0c72e6c4a641f24049a917db0cb", 18),
    tok(8453, "CBBTC", "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", 8),
    tok(8453, "EURC", "0x60a3e35cc302bfa44cb288bc5a4f316fdb1adb42", 6),
    tok(8453, "USDS", "0x820c137fa70c8691f0e44dc420a5e53c168921dc", 18),
    tok(8453, "SUSDS", "0x5875eee11cf8398102fdad704c9e96607675467a", 18),
    tok(8453, "CBETH", "0x2ae3f1ec7f1f5012cfeab0185bfc7aa3cf0dec22", 18),
    tok(8453, "WSTETH", "0xc1cba3fcea344f92d9239c08c0568f6f2f0ee452", 18),
    tok(8453, "RETH", "0xb6fe221fe9eef5aba221c348ba20a1bf5e73624c", 18),
    tok(8453, "WEETH", "0x04c0599ae5a44757c0af6f9ec3b93da8976c150a", 18),
    tok(8453, "AERO", "0x940181a94a35a4569e4529a3cdfb74e38fd98631", 18),
    tok(8453, "DEGEN", "0x4ed4e862860bed51a9570b96d89af5e1b0efefed", 18),
    // ── Optimism (chain 10) ───────────────────────────────────────────────
    tok(10, "USDC", "0x0b2c639c533813f4aa9d7837caf62653d097ff85", 6),
    tok(10, "USDT", "0x94b008aa00579c1307b0ef2c499ad98a8ce58e58", 6),
    tok(10, "DAI", "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1", 18),
    tok(10, "USDS", "0x4f13a96ec5c4cf34e442b46bbd98a0791f20edc3", 18),
    tok(10, "SUSDS", "0xb5b2dc7fd34c249f4be7fb1fcea07950784229e0", 18),
    tok(10, "WETH", "0x4200000000000000000000000000000000000006", 18),
    tok(10, "WBTC", "0x68f180fcce6836688e9084f035309e29bf0a2095", 8),
    tok(10, "WSTETH", "0x1f32b1c2345538c0c6f582fcb022739c4a194ebb", 18),
    tok(10, "RETH", "0x9bcef72be871e61ed4fbbc7630889bee758eb81d", 18),
    tok(10, "WEETH", "0x5a7facb970d094b6c7ff1df0ea68d99e6e73cbff", 18),
    tok(10, "LINK", "0x350a791bfc2c21f9ed5d10980dad2e2638ffa7f6", 18),
    tok(10, "OP", "0x4200000000000000000000000000000000000042", 18),
    // ── Arbitrum (chain 42161) ────────────────────────────────────────────
    tok(42161, "USDC", "0xaf88d065e77c8cc2239327c5edb3a432268e5831", 6),
    tok(42161, "USDT", "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9", 6),
    tok(42161, "DAI", "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1", 18),
    tok(42161, "USDS", "0x6491c05a82219b8d1479057361ff1654749b876b", 18),
    tok(42161, "SUSDS", "0xddb46999f8891663a8f2828d25298f70416d7610", 18),
    tok(42161, "WETH", "0x82af49447d8a07e3bd95bd0d56f35241523fbab1", 18),
    tok(42161, "WBTC", "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f", 8),
    tok(42161, "CBBTC", "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", 8),
    tok(42161, "WSTETH", "0x5979d7b546e38e414f7e9822514be443a4800529", 18),
    tok(42161, "RETH", "0xec70dcb4a1efa46b8f2d97c310c9c4790ba5ffa8", 18),
    tok(42161, "WEETH", "0x35751007a407ca6feffe80b3cb397736d2cf4dbe", 18),
    tok(42161, "ARB", "0x912ce59144191c1204e64559fe8253a0e49e6548", 18),
    tok(42161, "LINK", "0xf97f4df75117a78c1a5a0dbb814af92458539fb4", 18),
    tok(42161, "GMX", "0xfc5a1a6eb076a2c7ad06ed22c90d7e710e35ad0a", 18),
    tok(42161, "LDO", "0x13ad51ed4f1b7e9dc168d8a00cb3f4ddd85efa60", 18),
    // ── BNB Chain (chain 56) ──────────────────────────────────────────────
    // BNB Chain's Binance-Peg USDC and USDT use 18 decimals, not 6.
    tok(56, "USDC", "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d", 18),
    tok(56, "USDT", "0x55d398326f99059ff775485246999027b3197955", 18),
    tok(56, "DAI", "0x1af3f329e8be154074d8769d1ffa4ee058b1dbc3", 18),
    // BSC's WETH is its own contract, not the Ethereum WETH address.
    tok(56, "WETH", "0x2170ed0880ac9a755fd29b2688956bd959f933f8", 18),
    tok(56, "CAKE", "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82", 18),
    tok(56, "BUSD", "0xe9e7cea3dedca5984780bafc599bd69add087d56", 18),
    tok(56, "WBNB", "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c", 18),
    // BTCB is Binance-Peg BTC and reports 18 decimals, not 8.
    tok(56, "BTCB", "0x7130d2a12b9bcbfae4f2634d864a1ee1ce3ead9c", 18),
    // ── Avalanche (chain 43114) ───────────────────────────────────────────
    tok(43114, "USDC", "0xb97ef9ef8734c71904d8002f8b6bc66dd9c48a6e", 6),
    tok(43114, "USDT", "0x9702230a8ea53601f5cd2dc00fdbc13d4df4a8c7", 6),
    tok(43114, "EURC", "0xc891eb4cbdeff6e073e859e987815ed1505c2acd", 6),
    // DAI, WETH and LINK are the Avalanche Bridge `.e` tokens.
    tok(43114, "DAI", "0xd586e7f844cea2f87f50152665bcbc2c279d8d70", 18),
    tok(43114, "WAVAX", "0xb31f66aa3c1e785363f0875a1b74e27b85fd66c7", 18),
    tok(43114, "WETH", "0x49d5c2bdffac6ce2bfdb6640f4f80f226bc10bab", 18),
    tok(43114, "LINK", "0x5947bb275c521040051d82396192181b413227a3", 18),
    // ── Gnosis (chain 100) ────────────────────────────────────────────────
    tok(100, "WXDAI", "0xe91d153e0b41518a2ce8dd3d7944fa863463a97d", 18),
    // USDC is USDC.e, Circle's Bridged USDC Standard token that the Gnosis
    // Bridge mints by default. The legacy Omnibridge USDC is not listed.
    tok(100, "USDC", "0x2a22f9c3b484c3629090feed35f17ff8f88f76f0", 6),
    // USDT, WETH and wstETH are the Omnibridge tokens from Ethereum.
    tok(100, "USDT", "0x4ecaba5870353805a9f068101a40e0f32ed605c6", 6),
    tok(100, "WETH", "0x6a023ccd1ff6f2045c3309768ead9e68f978f6e1", 18),
    tok(100, "WSTETH", "0x6c76971f98945ae98dd7d4dfca8711ebea946ea6", 18),
    tok(100, "GNO", "0x9c58bacc331c9aa871afd802db6379a98e80cedb", 18),
    tok(100, "SDAI", "0xaf204776c7245bf4147c2612bf6e5972ee483701", 18),
    tok(100, "EURE", "0x420ca0f9b9b604ce0fd9c18ef134c705e5fa3430", 18),
    // ── Linea (chain 59144) ───────────────────────────────────────────────
    tok(59144, "WETH", "0xe5d7c2a44ffddf6b295a15c148167daaaf5cf34f", 18),
    tok(59144, "USDC", "0x176211869ca2b568f2a7d4ee941e073a821ee1ff", 6),
    // USDT, WBTC and DAI are Linea canonical-bridge tokens from Ethereum.
    tok(59144, "USDT", "0xa219439258ca9da29e9cc4ce5596924745e12b93", 6),
    tok(59144, "WBTC", "0x3aab2285ddcddad8edf438c1bab47e1a9d05a9b4", 8),
    tok(59144, "DAI", "0x4af15ec2a0bd43db75dd04e62faa3b8ef36b00d5", 18),
    tok(59144, "WSTETH", "0xb5bedd42000b71fdde22d3ee8a79bd49a568fc8f", 18),
    tok(59144, "LINEA", "0x1789e0043623282d5dcc7f213d703c6d8bafbb04", 18),
    tok(59144, "MUSD", "0xaca92e438df0b2401ff60da7e4337b687a2435da", 6),
    // ── HyperEVM (chain 999) ──────────────────────────────────────────────
    tok(999, "WHYPE", "0x5555555555555555555555555555555555555555", 18),
    tok(999, "USDC", "0xb88339cb7199b77e23db6e890353e22632ba630f", 6),
    // USDT0 (on-chain symbol `USD₮0`) is the omnichain USDT deployment on
    // HyperEVM; both symbols name it.
    tok(999, "USDT0", "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb", 6),
    tok(999, "USDT", "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb", 6),
    // UBTC and UETH are Unit's bridged BTC and ETH.
    tok(999, "UBTC", "0x9fdbda0a5e284c32744d2f17ee5c74b284993463", 8),
    tok(999, "UETH", "0xbe6727b535545c67d5caa73dea54865b92cf7907", 18),
    // ── Tempo (chain 4217) ────────────────────────────────────────────────
    // TIP-20 stablecoins are all 6-decimal. USDC and EURC are the Stargate
    // bridged USDC.e and EURC.e, the only USDC and EURC in Tempo's token list.
    tok(4217, "PATHUSD", "0x20c0000000000000000000000000000000000000", 6),
    tok(4217, "USDC", "0x20c000000000000000000000b9537d11c60e8b50", 6),
    tok(4217, "USDT0", "0x20c00000000000000000000014f22ca97301eb73", 6),
    tok(4217, "USDT", "0x20c00000000000000000000014f22ca97301eb73", 6),
    tok(4217, "EURC", "0x20c0000000000000000000001621e21f71cf12fb", 6),
    tok(4217, "CUSD", "0x20c0000000000000000000000520792dcccccccc", 6),
    // ── Robinhood Chain (chain 4663) ──────────────────────────────────────
    tok(4663, "WETH", "0x0bd7d308f8e1639fab988df18a8011f41eacad73", 18),
    tok(4663, "USDG", "0x5fc5360d0400a0fd4f2af552add042d716f1d168", 6),
    // Robinhood Stock Tokens: plain ERC-20s (not rebasing) whose offer and
    // sale are restricted in some jurisdictions, including the United States.
    tok(4663, "NVDA", "0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec", 18),
    tok(4663, "SPCX", "0x4a0e65a3eccec6dbe60ae065f2e7bb85fae35eea", 18),
    tok(4663, "META", "0xc0d6457c16cc70d6790dd43521c899c87ce02f35", 18),
    tok(4663, "SPY", "0x117cc2133c37b721f49de2a7a74833232b3b4c0c", 18),
];

fn registry_entry(chain_id: u64, upper: &str) -> Option<&'static TokenEntry> {
    TOKENS
        .iter()
        .find(|t| t.chain_id == chain_id && t.symbol == upper)
}

/// Resolve a token symbol or address into a concrete [`Address`] for a chain.
///
/// The static registry covers the most traded tokens on Bloom's default EVM
/// chains (all but Arc). A bare `0x` address is parsed directly. Native
/// gas-token aliases (ETH, MATIC/POL, BNB, AVAX, XDAI, HYPE) resolve to
/// [`NATIVE_TOKEN`] on their home chain.
pub fn resolve_token_symbol(chain_id: u64, sym: &str) -> Option<Address> {
    let s = sym.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        return s.parse::<Address>().ok();
    }
    let upper = s.to_ascii_uppercase();

    if is_native_alias(chain_id, &upper) {
        return NATIVE_TOKEN.parse().ok();
    }

    registry_entry(chain_id, &upper)?.address.parse().ok()
}

/// Decimals for a registry symbol on a chain.
///
/// Native gas-token aliases use 18. Registry tokens use the on-chain
/// `decimals()` recorded beside their address, so the same symbol can differ
/// by chain (BNB Chain's USDC and USDT are 18-decimal). Returns `None` for a
/// symbol the registry does not know rather than guessing a scale.
pub fn decimals_for_symbol(chain_id: u64, symbol: &str) -> Option<u8> {
    let upper = symbol.trim().to_ascii_uppercase();
    if is_native_alias(chain_id, &upper) {
        return Some(18);
    }
    registry_entry(chain_id, &upper).map(|t| t.decimals)
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
        "bsc" | "bnb" => Some(56),
        "avalanche" | "avax" => Some(43114),
        "gnosis" | "xdai" => Some(100),
        "linea" => Some(59144),
        "hyperliquid" | "hyperevm" => Some(999),
        "tempo" => Some(4217),
        "robinhood" => Some(4663),
        _ => None,
    }
}

/// Chain ID to the Bloom chain name (the key in Bloom's `[chains]` config).
pub fn chain_id_to_name(id: u64) -> Option<&'static str> {
    match id {
        1 => Some("ethereum"),
        137 => Some("polygon"),
        8453 => Some("base"),
        10 => Some("optimism"),
        42161 => Some("arbitrum"),
        56 => Some("bsc"),
        43114 => Some("avalanche"),
        100 => Some("gnosis"),
        59144 => Some("linea"),
        999 => Some("hyperliquid"),
        4217 => Some("tempo"),
        4663 => Some("robinhood"),
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
            "USDC", "USDT", "DAI", "WETH", "WBTC", "LINK", "UNI", "AAVE", "MKR", "SNX", "CRV",
            "LDO", "MATIC", "SHIB", "PEPE", "ENS", "WSTETH", "RPL", "FXS",
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
            "USDC", "USDT", "DAI", "WETH", "WMATIC", "WBTC", "LINK", "AAVE", "CRV", "SUSHI",
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
    fn base_dai_uses_the_base_deployment() {
        assert_eq!(
            resolve_token_symbol(8453, "DAI").unwrap(),
            "0x50c5725949a6f0c72e6c4a641f24049a917db0cb"
                .parse::<Address>()
                .unwrap()
        );
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
        for sym in [
            "USDC", "USDT", "DAI", "WETH", "WBTC", "ARB", "LINK", "GMX", "LDO",
        ] {
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
        assert_eq!(decimals_for_symbol(1, "USDC"), Some(6));
        assert_eq!(decimals_for_symbol(1, "USDT"), Some(6));
        // DAI is 18, never 6.
        assert_eq!(decimals_for_symbol(1, "DAI"), Some(18));
        assert_eq!(decimals_for_symbol(42161, "DAI"), Some(18));
        assert_eq!(decimals_for_symbol(1, "WBTC"), Some(8));
        assert_eq!(decimals_for_symbol(1, "WETH"), Some(18));
        assert_eq!(decimals_for_symbol(10, "OP"), Some(18));
        assert_eq!(decimals_for_symbol(1, "LINK"), Some(18));
        assert_eq!(decimals_for_symbol(1, "PEPE"), Some(18));
        // Native gas-token aliases are 18.
        assert_eq!(decimals_for_symbol(8453, "ETH"), Some(18));
        assert_eq!(decimals_for_symbol(137, "POL"), Some(18));
        // Unknown symbols have no guessed scale.
        assert_eq!(decimals_for_symbol(1, "UNKNOWN"), None);
        // A symbol absent on one chain is not borrowed from another.
        assert_eq!(decimals_for_symbol(56, "CBBTC"), None);
        // Case-insensitive.
        assert_eq!(decimals_for_symbol(1, "usdc"), Some(6));
        assert_eq!(decimals_for_symbol(1, "dai"), Some(18));
    }

    #[test]
    fn bnb_stablecoins_use_eighteen_decimals() {
        // Binance-Peg USDC/USDT on BNB Chain report decimals() == 18.
        assert_eq!(decimals_for_symbol(56, "USDC"), Some(18));
        assert_eq!(decimals_for_symbol(56, "USDT"), Some(18));
        assert_eq!(decimals_for_symbol(1, "USDC"), Some(6));
    }

    /// Every token added or corrected in the common-token expansion, with its
    /// issuer-documented address and on-chain `decimals()`.
    const VERIFIED_ADDITIONS: &[(u64, &str, &str, u8)] = &[
        (1, "cbBTC", "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf", 8),
        (1, "EURC", "0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c", 6),
        (1, "USDS", "0xdC035D45d973E3EC169d2276DDab16f1e407384F", 18),
        (1, "sUSDS", "0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD", 18),
        (1, "rETH", "0xae78736Cd615f374D3085123A210448E74Fc6393", 18),
        (1, "weETH", "0xCd5fE23C85820F7B72D0926FC9b05b43E359b7ee", 18),
        (1, "cbETH", "0xBe9895146f7AF43049ca1c1AE358B0541Ea49704", 18),
        (1, "POL", "0x455e53CBB86018Ac2B8092FdCd39d8444aFFC3F6", 18),
        (1, "ENS", "0xC18360217D8F7Ab5e7c516566761Ea12Ce7F9D72", 18),
        (
            8453,
            "cbBTC",
            "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf",
            8,
        ),
        (
            8453,
            "EURC",
            "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42",
            6,
        ),
        (
            8453,
            "AERO",
            "0x940181a94A35A4569E4529A3CDfB74e38FD98631",
            18,
        ),
        (
            8453,
            "wstETH",
            "0xc1CBa3fCea344f92D9239c08C0568f6F2F0ee452",
            18,
        ),
        (
            8453,
            "rETH",
            "0xB6fe221Fe9EeF5aBa221c348bA20A1Bf5e73624c",
            18,
        ),
        (
            8453,
            "weETH",
            "0x04C0599Ae5A44757c0af6F9eC3b93da8976c150A",
            18,
        ),
        (
            8453,
            "USDS",
            "0x820C137fa70C8691f0e44Dc420a5e53c168921Dc",
            18,
        ),
        (
            8453,
            "sUSDS",
            "0x5875eEE11Cf8398102FdAd704C9E96607675467a",
            18,
        ),
        (
            10,
            "wstETH",
            "0x1F32b1c2345538c0c6f582fCB022739c4A194Ebb",
            18,
        ),
        (10, "rETH", "0x9Bcef72be871e61ED4fBbc7630889beE758eb81D", 18),
        (
            10,
            "weETH",
            "0x5A7fACB970D094B6C7FF1df0eA68D99E6e73CBFF",
            18,
        ),
        (10, "USDS", "0x4F13a96EC5C4Cf34e442b46Bbd98a0791F20edC3", 18),
        (
            10,
            "sUSDS",
            "0xb5B2dc7fd34C249F4be7fB1fCea07950784229e0",
            18,
        ),
        (10, "LINK", "0x350a791Bfc2C21F9Ed5d10980Dad2e2638ffa7f6", 18),
        (
            42161,
            "cbBTC",
            "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf",
            8,
        ),
        (
            42161,
            "wstETH",
            "0x5979D7b546E38E414F7E9822514be443A4800529",
            18,
        ),
        (
            42161,
            "rETH",
            "0xEC70Dcb4A1EFa46b8F2D97C310C9c4790ba5ffA8",
            18,
        ),
        (
            42161,
            "weETH",
            "0x35751007a407ca6FEFfE80b3cB397736D2cf4dbe",
            18,
        ),
        (
            42161,
            "USDS",
            "0x6491c05A82219b8D1479057361ff1654749b876b",
            18,
        ),
        (
            42161,
            "sUSDS",
            "0xdDb46999F8891663a8F2828d25298f70416d7610",
            18,
        ),
        (
            42161,
            "GMX",
            "0xfc5A1A6EB076a2C7aD06eD22C90d7E710E35ad0a",
            18,
        ),
        (
            42161,
            "LDO",
            "0x13Ad51ed4F1B7e9Dc168d8a00cB3f4dDD85EfA60",
            18,
        ),
        (
            137,
            "wstETH",
            "0x03b54A6e9a984069379fae1a4fC4dBAE93B3bCCD",
            18,
        ),
        (
            43114,
            "EURC",
            "0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD",
            6,
        ),
        (
            43114,
            "USDT",
            "0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7",
            6,
        ),
        (
            43114,
            "LINK",
            "0x5947BB275c521040051D82396192181b413227A3",
            18,
        ),
        // Default-chain coverage: BNB Chain, Gnosis, Linea, HyperEVM, Tempo
        // and Robinhood Chain.
        (56, "WBNB", "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", 18),
        (56, "BTCB", "0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c", 18),
        (
            100,
            "WXDAI",
            "0xe91D153E0b41518A2Ce8Dd3D7944Fa863463a97d",
            18,
        ),
        (100, "USDC", "0x2a22f9c3b484c3629090FeED35F17Ff8F88f76F0", 6),
        (100, "USDT", "0x4ECaBa5870353805a9F068101A40E0f32ed605C6", 6),
        (
            100,
            "WETH",
            "0x6A023CCd1ff6F2045C3309768eAd9E68F978f6e1",
            18,
        ),
        (
            100,
            "wstETH",
            "0x6C76971f98945AE98dD7d4DFcA8711ebea946eA6",
            18,
        ),
        (100, "GNO", "0x9C58BAcC331c9aa871AFD802DB6379a98e80CEdb", 18),
        (
            100,
            "sDAI",
            "0xaf204776c7245bF4147c2612BF6e5972Ee483701",
            18,
        ),
        (
            100,
            "EURe",
            "0x420CA0f9B9b604cE0fd9C18EF134C705e5Fa3430",
            18,
        ),
        (
            59144,
            "WETH",
            "0xe5D7C2a44FfDDf6b295A15c148167daaAf5Cf34f",
            18,
        ),
        (
            59144,
            "USDC",
            "0x176211869cA2b568f2A7D4EE941E073a821EE1ff",
            6,
        ),
        (
            59144,
            "USDT",
            "0xA219439258ca9da29E9Cc4cE5596924745e12B93",
            6,
        ),
        (
            59144,
            "WBTC",
            "0x3aAB2285ddcDdaD8edf438C1bAB47e1a9D05a9b4",
            8,
        ),
        (
            59144,
            "DAI",
            "0x4AF15ec2A0BD43Db75dd04E62FAA3B8EF36b00d5",
            18,
        ),
        (
            59144,
            "wstETH",
            "0xB5beDd42000b71FddE22D3eE8a79Bd49A568fC8F",
            18,
        ),
        (
            59144,
            "LINEA",
            "0x1789e0043623282D5DCc7F213d703C6D8BAfBB04",
            18,
        ),
        (
            59144,
            "mUSD",
            "0xacA92E438df0B2401fF60dA7E4337B687a2435DA",
            6,
        ),
        (
            999,
            "WHYPE",
            "0x5555555555555555555555555555555555555555",
            18,
        ),
        (999, "USDC", "0xb88339CB7199b77E23DB6E890353E22632Ba630f", 6),
        (
            999,
            "USDT0",
            "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb",
            6,
        ),
        (999, "USDT", "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb", 6),
        (999, "UBTC", "0x9FDBdA0A5e284c32744D2f17Ee5c74B284993463", 8),
        (
            999,
            "UETH",
            "0xBe6727B535545C67d5cAa73dEa54865B92CF7907",
            18,
        ),
        (
            4217,
            "pathUSD",
            "0x20C0000000000000000000000000000000000000",
            6,
        ),
        (
            4217,
            "USDC",
            "0x20C000000000000000000000b9537d11c60E8b50",
            6,
        ),
        (
            4217,
            "USDT0",
            "0x20C00000000000000000000014f22CA97301EB73",
            6,
        ),
        (
            4217,
            "USDT",
            "0x20C00000000000000000000014f22CA97301EB73",
            6,
        ),
        (
            4217,
            "EURC",
            "0x20c0000000000000000000001621e21F71CF12fb",
            6,
        ),
        (
            4217,
            "cUSD",
            "0x20C0000000000000000000000520792DcCccCccC",
            6,
        ),
        (
            4663,
            "WETH",
            "0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73",
            18,
        ),
        (
            4663,
            "USDG",
            "0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168",
            6,
        ),
        (
            4663,
            "NVDA",
            "0xd0601CE157Db5bdC3162BbaC2a2C8aF5320D9EEC",
            18,
        ),
        (
            4663,
            "SPCX",
            "0x4a0E65A3EcceC6dBe60AE065F2e7bb85Fae35eEa",
            18,
        ),
        (
            4663,
            "META",
            "0xc0D6457C16Cc70d6790Dd43521C899C87ce02f35",
            18,
        ),
        (
            4663,
            "SPY",
            "0x117cc2133c37B721F49dE2A7a74833232B3B4C0C",
            18,
        ),
    ];

    #[test]
    fn verified_additions_resolve_with_decimals() {
        for &(chain, sym, addr, decimals) in VERIFIED_ADDITIONS {
            assert_eq!(
                resolve_token_symbol(chain, sym),
                Some(addr.parse::<Address>().unwrap()),
                "{sym} on chain {chain}"
            );
            assert_eq!(
                decimals_for_symbol(chain, sym),
                Some(decimals),
                "{sym} decimals on chain {chain}"
            );
        }
    }

    #[test]
    fn cbbtc_is_only_on_coinbase_chains() {
        for chain in [1u64, 8453, 42161] {
            assert!(resolve_token_symbol(chain, "cbBTC").is_some());
        }
        for chain in [137u64, 10, 56, 43114, 100, 59144, 999, 4663] {
            assert!(resolve_token_symbol(chain, "cbBTC").is_none());
        }
    }

    #[test]
    fn gmx_is_not_listed_on_ethereum() {
        assert!(resolve_token_symbol(1, "GMX").is_none());
    }

    #[test]
    fn registry_rows_are_unique_and_parse() {
        let mut seen = std::collections::HashSet::new();
        for t in TOKENS {
            assert_eq!(t.symbol, t.symbol.to_ascii_uppercase(), "{}", t.symbol);
            assert!(
                seen.insert((t.chain_id, t.symbol)),
                "duplicate {} on chain {}",
                t.symbol,
                t.chain_id
            );
            assert!(t.address.parse::<Address>().is_ok(), "{}", t.address);
            assert!(chain_id_to_name(t.chain_id).is_some());
            assert!(!is_native_alias(t.chain_id, t.symbol));
        }
    }

    #[test]
    fn swap_eth_to_cbbtc_on_base_resolves() {
        let nat = parse_natural_intent("swap 0.001 ETH to cbBTC on base").unwrap();
        let chain_id = chain_to_id(nat.chain.as_deref().unwrap()).unwrap();
        assert_eq!(chain_id, 8453);
        assert_eq!(
            resolve_token_symbol(chain_id, &nat.token_in).unwrap(),
            NATIVE_TOKEN.parse::<Address>().unwrap()
        );
        assert_eq!(
            resolve_token_symbol(chain_id, &nat.token_out).unwrap(),
            "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"
                .parse::<Address>()
                .unwrap()
        );
        assert_eq!(decimals_for_symbol(chain_id, &nat.token_out), Some(8));
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

    /// Bloom's default EVM chains as `(bloom chain name, chain id)`, from
    /// `bloom_proto::config::default_chains()`. Arc (5042) is excluded: its gas
    /// token is USDC, which is also an ERC-20, and the registry does not model
    /// a native token that shares a symbol with a registry row.
    const BLOOM_DEFAULT_CHAINS: &[(&str, u64)] = &[
        ("ethereum", 1),
        ("base", 8453),
        ("tempo", 4217),
        ("robinhood", 4663),
        ("arbitrum", 42161),
        ("optimism", 10),
        ("polygon", 137),
        ("bsc", 56),
        ("avalanche", 43114),
        ("gnosis", 100),
        ("linea", 59144),
        ("hyperliquid", 999),
    ];

    /// Minimum distinct non-native tokens per default chain.
    const MIN_TOKENS_PER_CHAIN: usize = 5;

    #[test]
    fn bloom_chain_names_resolve_to_their_ids() {
        for &(name, id) in BLOOM_DEFAULT_CHAINS {
            assert_eq!(chain_to_id(name), Some(id), "{name}");
            assert_eq!(chain_id_to_name(id), Some(name), "{id}");
        }
        // Pre-existing aliases keep working.
        assert_eq!(chain_to_id("bnb"), Some(56));
        assert_eq!(chain_to_id("BSC"), Some(56));
        assert_eq!(chain_to_id("xdai"), Some(100));
        assert_eq!(chain_to_id("hyperevm"), Some(999));
        assert_eq!(chain_to_id("arc"), None);
    }

    #[test]
    fn every_default_chain_has_common_tokens() {
        for &(name, id) in BLOOM_DEFAULT_CHAINS {
            let distinct: std::collections::HashSet<_> = TOKENS
                .iter()
                .filter(|t| t.chain_id == id)
                .map(|t| t.address)
                .collect();
            assert!(
                distinct.len() >= MIN_TOKENS_PER_CHAIN,
                "{name} ({id}) has {} tokens, want at least {MIN_TOKENS_PER_CHAIN}",
                distinct.len()
            );
        }
    }

    #[test]
    fn native_aliases_on_newer_chains() {
        let native = NATIVE_TOKEN.parse::<Address>().unwrap();
        for (chain, sym) in [
            (100u64, "XDAI"),
            (100, "xDAI"),
            (999, "HYPE"),
            (59144, "ETH"),
            (4663, "ETH"),
        ] {
            assert_eq!(
                resolve_token_symbol(chain, sym),
                Some(native),
                "{sym} on {chain}"
            );
            assert_eq!(
                decimals_for_symbol(chain, sym),
                Some(18),
                "{sym} on {chain}"
            );
        }
        // Tempo has no native gas token, so ETH does not resolve there.
        assert_eq!(resolve_token_symbol(4217, "ETH"), None);
        assert_eq!(decimals_for_symbol(4217, "ETH"), None);
        // Native aliases stay on their home chain.
        assert_eq!(resolve_token_symbol(1, "HYPE"), None);
        assert_eq!(resolve_token_symbol(1, "XDAI"), None);
    }

    #[test]
    fn swap_xdai_to_usdc_on_gnosis_resolves() {
        let nat = parse_natural_intent("swap 10 xDAI to USDC on gnosis").unwrap();
        let chain_id = chain_to_id(nat.chain.as_deref().unwrap()).unwrap();
        assert_eq!(chain_id, 100);
        assert_eq!(
            resolve_token_symbol(chain_id, &nat.token_in).unwrap(),
            NATIVE_TOKEN.parse::<Address>().unwrap()
        );
        assert_eq!(decimals_for_symbol(chain_id, &nat.token_out), Some(6));
    }

    #[test]
    fn chain_helpers_roundtrip() {
        for id in [
            1u64, 137, 8453, 10, 42161, 56, 43114, 100, 59144, 999, 4217, 4663,
        ] {
            let name = chain_id_to_name(id).unwrap();
            assert_eq!(chain_to_id(name), Some(id));
        }
        assert!(chain_to_id("ethereum").is_some());
        assert!(chain_to_id("mainnet").is_some());
        assert!(chain_to_id("matic").is_some());
        assert!(chain_to_id("bsc").is_some());
        assert!(chain_to_id("nope").is_none());
        assert!(chain_id_to_name(5042).is_none());
    }
}
