use crate::api_types::IERC20;
use alloy::primitives::{Address, Bytes, U64, U256, hex};
use alloy::sol_types::{Panic, Revert, SolCall, SolError};
use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};

/// Result of a generic `eth_call`.
///
/// `return_data` is always `0x`-prefixed hex. When `success` is `false` the
/// `return_data` carries the raw revert payload (an `Error(string)` or
/// `Panic(uint256)` encoding).
#[derive(Debug, Clone)]
pub struct EthCallResult {
    pub success: bool,
    pub return_data: String, // 0x-prefixed hex
}

pub trait Host {
    fn now_ms(&mut self) -> u64;
    fn random(&mut self, len: usize) -> Result<Vec<u8>, String>;
    fn setting(&mut self, key: &str) -> Result<Option<String>, String>;
    fn http(&mut self, req: HttpRequest, max: usize) -> Result<HttpResponse, String>;
    fn get(&mut self, key: &str, max: usize) -> Result<Option<Vec<u8>>, String>;
    fn get_secret(&mut self, key: &str, max: usize) -> Result<Option<Vec<u8>>, String>;
    fn list(&mut self, prefix: &str, max: usize) -> Result<Vec<String>, String>;
    fn put(&mut self, key: &str, value: &[u8], secret: bool) -> Result<(), String>;
    fn put_new(&mut self, key: &str, value: &[u8], secret: bool) -> Result<(), String>;
    fn delete_if(&mut self, key: &str, expected: &[u8]) -> Result<(), String>;
    fn vfs_read(&mut self, path: &str, max: usize) -> Result<Vec<u8>, String>;
    fn chain_read(&mut self, chain: &str, method: &str, params: &str) -> Result<String, String>;
    fn tx_stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction, String>;
    fn tx_confirm(
        &mut self,
        wallet: &str,
        chain: &str,
        id: &str,
        warnings: bool,
    ) -> Result<StagedTransaction, String>;
    fn tx_inspect(
        &mut self,
        wallet: &str,
        chain: &str,
        id: &str,
    ) -> Result<OutboxInspection, String>;

    // --- Chain helpers -----------------------------------------------------

    /// Generic `eth_call` against `to` with the given already-encoded `data`
    /// (0x-prefixed hex). `from`/`value` are optional tx-object fields.
    fn eth_call(
        &mut self,
        chain: &str,
        to: &str,
        data: &str,
        from: Option<&str>,
        value: Option<U256>,
    ) -> Result<EthCallResult, String>;

    /// `eth_chainId` for the chain, returned as the canonical numeric chain id.
    fn chain_id(&mut self, chain: &str) -> Result<u64, String>;

    /// `ERC-20 allowance(owner, spender)`.
    fn erc20_allowance(
        &mut self,
        chain: &str,
        token: &str,
        owner: &str,
        spender: &str,
    ) -> Result<U256, String>;

    /// `ERC-20 balanceOf(addr)`.
    fn erc20_balance(&mut self, chain: &str, token: &str, addr: &str) -> Result<U256, String>;

    /// Native balance from `eth_getBalance`.
    fn eth_balance(&mut self, chain: &str, addr: &str) -> Result<U256, String>;

    /// `ERC-20 decimals()`.
    fn erc20_decimals(&mut self, chain: &str, token: &str) -> Result<u8, String>;
}

pub struct BloomHost;

impl Host for BloomHost {
    fn now_ms(&mut self) -> u64 {
        petal::sdk::now_ms()
    }

    fn random(&mut self, len: usize) -> Result<Vec<u8>, String> {
        petal::sdk::random_bytes(len).map_err(|error| error.message())
    }

    fn setting(&mut self, key: &str) -> Result<Option<String>, String> {
        petal::sdk::runtime_setting(key).map_err(|error| error.message())
    }

    fn http(&mut self, req: HttpRequest, max: usize) -> Result<HttpResponse, String> {
        petal::sdk::http_fetch(&req, max).map_err(|error| error.message())
    }

    fn get(&mut self, key: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        match petal::sdk::store_get(key, max) {
            Ok(value) => Ok(Some(value)),
            Err(petal::SdkError::Host(petal::HostStatus::NotFound)) => Ok(None),
            Err(error) => Err(error.message()),
        }
    }

    fn get_secret(&mut self, key: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        let value = petal::bindings::bloom::store::kv::get("secrets", key)?;
        if value.as_ref().is_some_and(|bytes| bytes.len() > max) {
            return Err("secret store value exceeds read limit".into());
        }
        Ok(value)
    }

    fn list(&mut self, prefix: &str, max: usize) -> Result<Vec<String>, String> {
        petal::sdk::store_list(prefix, max).map_err(|error| error.message())
    }

    fn put(&mut self, key: &str, value: &[u8], secret: bool) -> Result<(), String> {
        petal::sdk::store_put(key, value, secret).map_err(|error| error.message())
    }

    fn put_new(&mut self, key: &str, value: &[u8], secret: bool) -> Result<(), String> {
        petal::sdk::store_put_new(key, value, secret).map_err(|error| error.message())
    }

    fn delete_if(&mut self, key: &str, expected: &[u8]) -> Result<(), String> {
        petal::sdk::store_del_if_value(key, expected).map_err(|error| error.message())
    }

    fn vfs_read(&mut self, path: &str, max: usize) -> Result<Vec<u8>, String> {
        petal::sdk::vfs_read(path, max).map_err(|error| error.message())
    }

    fn chain_read(&mut self, chain: &str, method: &str, params: &str) -> Result<String, String> {
        petal::sdk::chain_read(chain, method, params).map_err(|error| match error {
            petal::SdkError::Message(message) => message,
            // The SDK classifies host errors by substring and can collapse a
            // descriptive invalid-parameter error to `Host(Invalid)`.
            other => format!(
                "{other:?} (chain={chain}, method={method}, params_len={})",
                params.len()
            ),
        })
    }

    fn tx_stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction, String> {
        petal::sdk::tx_stage(tx).map_err(|error| error.message())
    }

    fn tx_confirm(
        &mut self,
        wallet: &str,
        chain: &str,
        id: &str,
        warnings: bool,
    ) -> Result<StagedTransaction, String> {
        petal::sdk::tx_confirm(wallet, chain, id, warnings).map_err(|error| error.message())
    }

    fn tx_inspect(
        &mut self,
        wallet: &str,
        chain: &str,
        id: &str,
    ) -> Result<OutboxInspection, String> {
        petal::sdk::tx_inspect(wallet, chain, id).map_err(|error| error.message())
    }

    fn eth_call(
        &mut self,
        chain: &str,
        to: &str,
        data: &str,
        from: Option<&str>,
        value: Option<U256>,
    ) -> Result<EthCallResult, String> {
        let mut tx = serde_json::json!({ "to": to, "data": data });
        if let Some(from) = from {
            tx["from"] = from.into();
        }
        if let Some(value) = value {
            tx["value"] = format!("0x{value:x}").into();
        }
        let params = serde_json::json!([tx, "latest"]).to_string();
        let raw = self.chain_read(chain, "eth_call", &params)?;
        // chain_read returns the JSON-RPC `result` field verbatim; for eth_call
        // that is a JSON string ("0x...") — unwrap one layer of JSON quoting.
        let hex_data: String = serde_json::from_str(&raw)
            .map_err(|e| format!("eth_call result is not a JSON string: {e}"))?;
        if !hex_data.starts_with("0x") {
            return Err(format!("eth_call returned unexpected value: {hex_data}"));
        }
        let bytes: Bytes = hex_data
            .parse()
            .map_err(|e| format!("eth_call returned invalid hex: {e}"))?;
        Ok(EthCallResult {
            success: !is_revert_payload(&bytes),
            return_data: hex_data,
        })
    }

    fn chain_id(&mut self, chain: &str) -> Result<u64, String> {
        let raw = self.chain_read(chain, "eth_chainId", "[]")?;
        let id: U64 = decode_quantity(&raw, "chain id")?;
        Ok(id.to::<u64>())
    }

    fn erc20_allowance(
        &mut self,
        chain: &str,
        token: &str,
        owner: &str,
        spender: &str,
    ) -> Result<U256, String> {
        let call = IERC20::allowanceCall {
            owner: parse_address(owner)?,
            spender: parse_address(spender)?,
        };
        erc20_view(self, chain, token, &call)
    }

    fn erc20_balance(&mut self, chain: &str, token: &str, addr: &str) -> Result<U256, String> {
        let call = IERC20::balanceOfCall {
            account: parse_address(addr)?,
        };
        erc20_view(self, chain, token, &call)
    }

    fn eth_balance(&mut self, chain: &str, addr: &str) -> Result<U256, String> {
        let params = serde_json::json!([addr, "latest"]).to_string();
        let raw = self.chain_read(chain, "eth_getBalance", &params)?;
        decode_quantity(&raw, "native balance")
    }

    fn erc20_decimals(&mut self, chain: &str, token: &str) -> Result<u8, String> {
        erc20_view(self, chain, token, &IERC20::decimalsCall {})
    }
}

// --- ABI helpers --------------------------------------------------------------

fn parse_address(addr: &str) -> Result<Address, String> {
    addr.parse().map_err(|_| format!("invalid address: {addr}"))
}

/// Whether `eth_call` return data is a standard Solidity revert payload.
fn is_revert_payload(data: &[u8]) -> bool {
    data.get(..4)
        .is_some_and(|selector| selector == Revert::SELECTOR || selector == Panic::SELECTOR)
}

/// Run a read-only ERC-20 call and strictly ABI-decode its return value.
fn erc20_view<H: Host + ?Sized, C: SolCall>(
    host: &mut H,
    chain: &str,
    token: &str,
    call: &C,
) -> Result<C::Return, String> {
    let data = hex::encode_prefixed(call.abi_encode());
    let res = host.eth_call(chain, token, &data, None, None)?;
    if !res.success {
        return Err(format!(
            "erc20 {} reverted: {}",
            C::SIGNATURE,
            res.return_data
        ));
    }
    decode_returns::<C>(&res.return_data)
}

fn decode_returns<C: SolCall>(return_data: &str) -> Result<C::Return, String> {
    let bytes: Bytes = return_data
        .parse()
        .map_err(|e| format!("erc20 {} returned invalid hex: {e}", C::SIGNATURE))?;
    C::abi_decode_returns_validate(&bytes)
        .map_err(|e| format!("erc20 {} return decode: {e}", C::SIGNATURE))
}

/// Decode a JSON-RPC quantity result such as `"0x3e8"`.
fn decode_quantity<T: serde::de::DeserializeOwned>(raw: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(raw).map_err(|e| format!("{what} decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(value: U256) -> String {
        format!("0x{value:064x}")
    }

    #[test]
    fn erc20_calls_use_standard_selectors() {
        let owner: Address = "0x0123456789abcdef0123456789abcdef01234567"
            .parse()
            .unwrap();
        let data = IERC20::allowanceCall {
            owner,
            spender: owner,
        }
        .abi_encode();
        assert_eq!(&data[..4], &[0xdd, 0x62, 0xed, 0x3e]);
        assert_eq!(&data[4..36], owner.into_word().as_slice());
        assert_eq!(
            IERC20::balanceOfCall { account: owner }.abi_encode()[..4],
            [0x70, 0xa0, 0x82, 0x31]
        );
        assert_eq!(
            IERC20::decimalsCall {}.abi_encode(),
            [0x31, 0x3c, 0xe5, 0x67]
        );
    }

    #[test]
    fn decodes_uint256_returns() {
        assert_eq!(
            decode_returns::<IERC20::balanceOfCall>(&word(U256::from(1000))).unwrap(),
            U256::from(1000)
        );
        assert_eq!(
            decode_returns::<IERC20::allowanceCall>(&word(U256::MAX)).unwrap(),
            U256::MAX
        );
        assert!(decode_returns::<IERC20::balanceOfCall>("0x03e8").is_err());
        assert!(decode_returns::<IERC20::balanceOfCall>("0xzz").is_err());
    }

    #[test]
    fn decimals_reject_out_of_range_words() {
        assert_eq!(
            decode_returns::<IERC20::decimalsCall>(&word(U256::from(6))).unwrap(),
            6
        );
        // 0x106 used to be truncated to its last byte and read as 6 decimals.
        assert!(decode_returns::<IERC20::decimalsCall>(&word(U256::from(0x106))).is_err());
    }

    #[test]
    fn recognizes_revert_payloads() {
        assert!(is_revert_payload(&Revert::from("nope").abi_encode()));
        assert!(is_revert_payload(&Panic::from(0x11).abi_encode()));
        assert!(!is_revert_payload(&[]));
        assert!(!is_revert_payload(&U256::from(1).to_be_bytes::<32>()));
    }

    #[test]
    fn decodes_rpc_quantities() {
        assert_eq!(
            decode_quantity::<U64>(r#""0x2105""#, "chain id").unwrap(),
            U64::from(8453)
        );
        assert_eq!(
            decode_quantity::<U256>(r#""0xde0b6b3a7640000""#, "balance").unwrap(),
            U256::from(1_000_000_000_000_000_000u64)
        );
        assert!(decode_quantity::<U256>(r#""0xzz""#, "balance").is_err());
        assert!(decode_quantity::<U256>("12", "balance").is_ok());
    }
}
