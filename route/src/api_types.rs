use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use serde::{Deserialize, Serialize};

/// Sentinel address Enso uses for the chain's native token (ETH, MATIC, …).
pub const NATIVE_TOKEN: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

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
                req.token_in == NATIVE_TOKEN.parse::<Address>().unwrap()
                    && amount == req.amount_in
                    && self.tx.value == amount
            }
            1 => {
                let Ok((token_in, amount)) = <(Address, U256)>::abi_decode_params(&token.data)
                else {
                    return false;
                };
                req.token_in != NATIVE_TOKEN.parse::<Address>().unwrap()
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

// --- serde helpers ---

pub(crate) fn de_bytes_hex<'de, D>(d: D) -> Result<Bytes, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    let s = s.trim();
    if s.is_empty() || s == "0x" {
        return Ok(Bytes::new());
    }
    let s = s.strip_prefix("0x").unwrap_or(s);
    let v = hex::decode(s).map_err(serde::de::Error::custom)?;
    Ok(Bytes::from(v))
}

pub(crate) fn de_u256_dec_or_hex<'de, D>(d: D) -> Result<U256, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(U256::ZERO),
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(U256::ZERO);
            }
            if let Some(hex) = t.strip_prefix("0x") {
                U256::from_str_radix(hex, 16).map_err(D::Error::custom)
            } else {
                U256::from_str_radix(t, 10).map_err(D::Error::custom)
            }
        }
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(U256::from(u))
            } else {
                Err(D::Error::custom("non-u64 number for U256"))
            }
        }
        other => Err(D::Error::custom(format!("unexpected value: {other}"))),
    }
}
