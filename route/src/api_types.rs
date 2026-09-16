use alloy::primitives::{Address, Bytes, U256, address};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use serde::{Deserialize, Serialize};

/// Sentinel address Enso uses for the chain's native token (ETH, MATIC, …).
pub const NATIVE_TOKEN: Address = address!("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

// The ERC-20 surface this Petal reads, approves, and verifies settlement with.
sol! {
    #[allow(missing_docs)]
    interface IERC20 {
        event Transfer(address indexed from, address indexed to, uint256 value);

        function allowance(address owner, address spender) external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
        function decimals() external view returns (uint8);
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

// Enso Router V2 wraps the actual shortcut calldata in one of these calls.
sol! {
    #[allow(missing_docs)]
    interface IEnsoRouter {
        struct Token {
            uint8 tokenType;
            bytes data;
        }

        function routeSingle(Token tokenIn, bytes data) external payable;
        function routeMulti(Token[] tokensIn, bytes data) external payable;
    }
}

/// Routing strategy. Maps directly onto Enso's `routingStrategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoutingStrategy {
    #[default]
    Router,
    Delegate,
    EnsoWallet,
}

impl RoutingStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            RoutingStrategy::Router => "router",
            RoutingStrategy::Delegate => "delegate",
            RoutingStrategy::EnsoWallet => "ensowallet",
        }
    }
}

/// Single-step route request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRequest {
    pub from_address: Address,
    pub chain_id: u64,
    pub destination_chain_id: Option<u64>,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub slippage_bps: u16,
    pub routing_strategy: Option<RoutingStrategy>,
    pub receiver: Option<Address>,
}

impl RouteRequest {
    pub fn new(
        from_address: Address,
        chain_id: u64,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
    ) -> Self {
        Self {
            from_address,
            chain_id,
            destination_chain_id: None,
            token_in,
            token_out,
            amount_in,
            slippage_bps: 50,
            routing_strategy: Some(RoutingStrategy::Router),
            receiver: None,
        }
    }

    /// Build the query string for GET endpoints.
    pub fn to_query(&self) -> Vec<(&'static str, String)> {
        let mut q: Vec<(&'static str, String)> = vec![
            ("fromAddress", format!("0x{:x}", self.from_address)),
            ("chainId", self.chain_id.to_string()),
            ("tokenIn", format!("0x{:x}", self.token_in)),
            ("tokenOut", format!("0x{:x}", self.token_out)),
            ("amountIn", self.amount_in.to_string()),
            ("slippage", self.slippage_bps.to_string()),
        ];
        if let Some(s) = self.routing_strategy {
            q.push(("routingStrategy", s.as_str().to_string()));
        }
        if let Some(d) = self.destination_chain_id {
            q.push(("destinationChainId", d.to_string()));
        }
        if let Some(r) = self.receiver {
            q.push(("receiver", format!("0x{:x}", r)));
        }
        q
    }
}

/// The transaction Enso wants the wallet to broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTx {
    pub to: Address,
    #[serde(deserialize_with = "de_bytes_hex")]
    pub data: Bytes,
    #[serde(deserialize_with = "de_u256_dec_or_hex", default)]
    pub value: U256,
    pub from: Address,
}

/// Wire representation of the route response from Enso.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteResponse {
    pub tx: RouteTx,
    pub amount_out: String,
    #[serde(default)]
    pub gas: Option<String>,
    #[serde(default)]
    pub route: serde_json::Value,
    /// Enso-reported, unit UNVERIFIED — display only.
    #[serde(default)]
    pub price_impact: Option<f64>,
    /// Destination chain id extracted from the first bridging hop.
    #[serde(default)]
    pub destination_chain_id: Option<u64>,
}

impl RouteResponse {
    /// Verify that the executable Router V2 transaction carries the same
    /// source asset and amount as the request.
    pub fn input_matches_request(&self, req: &RouteRequest) -> bool {
        if self.tx.from != req.from_address {
            return false;
        }

        let Some(token) = (if let Ok(call) = IEnsoRouter::routeSingleCall::abi_decode(&self.tx.data)
        {
            Some(call.tokenIn)
        } else if let Ok(call) = IEnsoRouter::routeMultiCall::abi_decode(&self.tx.data) {
            (call.tokensIn.len() == 1).then(|| call.tokensIn.into_iter().next().unwrap())
        } else {
            None
        }) else {
            return false;
        };

        match token.tokenType {
            0 => {
                let Ok((amount,)) = <(U256,)>::abi_decode_params(&token.data) else {
                    return false;
                };
                req.token_in == NATIVE_TOKEN && amount == req.amount_in && self.tx.value == amount
            }
            1 => {
                let Ok((token_in, amount)) = <(Address, U256)>::abi_decode_params(&token.data)
                else {
                    return false;
                };
                req.token_in != NATIVE_TOKEN
                    && token_in == req.token_in
                    && amount == req.amount_in
                    && self.tx.value == U256::ZERO
            }
            _ => false,
        }
    }

    /// Verify that a response's bridge metadata agrees with the requested
    /// destination. Same-chain responses may omit destination metadata.
    pub fn destination_matches_request(&self, req: &RouteRequest) -> bool {
        let destination_chain_id = self.destination_chain_id.or_else(|| {
            self.route
                .as_array()?
                .iter()
                .find_map(|hop| hop.get("destinationChainId")?.as_u64())
        });
        match req.destination_chain_id {
            Some(expected) => destination_chain_id == Some(expected),
            None => destination_chain_id.is_none_or(|actual| actual == req.chain_id),
        }
    }

    /// Conservatively extract protocol names from the opaque `route` array.
    pub fn protocols(&self) -> (Vec<String>, bool) {
        let Some(hops) = self.route.as_array() else {
            return (Vec::new(), true);
        };
        let mut names: Vec<String> = Vec::new();
        let mut saw_field = false;
        for hop in hops {
            let name = hop
                .get("protocol")
                .or_else(|| hop.get("name"))
                .or_else(|| hop.get("project"));
            if let Some(n) = name.and_then(|v| v.as_str()) {
                saw_field = true;
                let lc = n.trim().to_lowercase();
                if !lc.is_empty() && !names.contains(&lc) {
                    names.push(lc);
                }
            }
        }
        let unknown = !hops.is_empty() && !saw_field;
        (names, unknown)
    }

    /// Whether the route's tx calldata encodes `receiver` as a 20-byte word.
    pub fn calldata_contains_receiver(&self, receiver: Address) -> bool {
        self.tx.data.windows(20).any(|w| w == receiver.as_slice())
    }
}

// --- parsing helpers ---

/// Parse a canonical base-unit integer string (ASCII digits only).
///
/// `U256::from_str_radix` alone also accepts `_` separators, which are never
/// valid in an Enso quote or a stored balance observation.
pub fn parse_decimal_u256(value: &str) -> Option<U256> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(value, 10).ok()
}

pub(crate) fn de_bytes_hex<'de, D>(d: D) -> Result<Bytes, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    let s = s.trim();
    if s.is_empty() {
        return Ok(Bytes::new());
    }
    s.parse().map_err(serde::de::Error::custom)
}

/// Enso sends `value` as a decimal string, a hex quantity, a number, `null`,
/// or an empty string. Everything except the empty forms is parsed by alloy.
pub(crate) fn de_u256_dec_or_hex<'de, D>(d: D) -> Result<U256, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(U256::ZERO),
        serde_json::Value::String(ref s) if s.trim().is_empty() => Ok(U256::ZERO),
        serde_json::Value::String(s) => s.trim().parse().map_err(D::Error::custom),
        other => U256::deserialize(other).map_err(D::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolEvent;

    #[derive(Deserialize)]
    struct Value {
        #[serde(deserialize_with = "de_u256_dec_or_hex")]
        value: U256,
    }

    fn value(json: &str) -> Result<U256, serde_json::Error> {
        serde_json::from_str::<Value>(json).map(|parsed| parsed.value)
    }

    #[test]
    fn route_value_accepts_enso_forms() {
        assert_eq!(value(r#"{"value":null}"#).unwrap(), U256::ZERO);
        assert_eq!(value(r#"{"value":""}"#).unwrap(), U256::ZERO);
        assert_eq!(value(r#"{"value":"1000"}"#).unwrap(), U256::from(1000));
        assert_eq!(value(r#"{"value":"0x3e8"}"#).unwrap(), U256::from(1000));
        assert_eq!(value(r#"{"value":1000}"#).unwrap(), U256::from(1000));
        assert!(value(r#"{"value":"-1"}"#).is_err());
        assert!(value(r#"{"value":1.5}"#).is_err());
    }

    #[test]
    fn decimal_u256_rejects_non_canonical_input() {
        assert_eq!(parse_decimal_u256("0"), Some(U256::ZERO));
        assert_eq!(parse_decimal_u256("0012"), Some(U256::from(12)));
        assert_eq!(parse_decimal_u256(""), None);
        assert_eq!(parse_decimal_u256("1_000"), None);
        assert_eq!(parse_decimal_u256("0x10"), None);
        assert_eq!(parse_decimal_u256(&"9".repeat(80)), None);
    }

    #[test]
    fn approve_calldata_uses_the_erc20_selector() {
        let spender = address!("0x1234567890abcdef1234567890abcdef12345678");
        let data = IERC20::approveCall {
            spender,
            amount: U256::from(123),
        }
        .abi_encode();
        assert_eq!(&data[..4], &[0x09, 0x5e, 0xa7, 0xb3]);
        assert_eq!(
            IERC20::Transfer::SIGNATURE_HASH,
            alloy::primitives::b256!(
                "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
            )
        );
    }
}
