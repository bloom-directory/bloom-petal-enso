use alloy::primitives::U256;
use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};

/// Result of a generic `eth_call`.
///
/// `return_data` is always `0x`-prefixed hex. When `success` is `false` the
/// `return_data` carries the raw revert payload (e.g. an `Error(string)`
/// encoding beginning with `0x08c379a0`).
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
        value: Option<&str>,
    ) -> Result<EthCallResult, String>;

    /// `eth_chainId` for the chain, returned as the canonical numeric chain id.
    fn chain_id(&mut self, chain: &str) -> Result<u64, String>;

    /// `ERC-20 allowance(owner, spender)` as a decimal string.
    fn erc20_allowance(
        &mut self,
        chain: &str,
        token: &str,
        owner: &str,
        spender: &str,
    ) -> Result<String, String>;

    /// `ERC-20 balanceOf(addr)` as a decimal string.
    fn erc20_balance(&mut self, chain: &str, token: &str, addr: &str) -> Result<String, String>;

    /// Native balance from `eth_getBalance`, as a decimal string.
    fn eth_balance(&mut self, chain: &str, addr: &str) -> Result<String, String>;

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
        petal::sdk::chain_read(chain, method, params).map_err(|error| error.message())
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
        value: Option<&str>,
    ) -> Result<EthCallResult, String> {
        let mut tx = serde_json::Map::new();
        tx.insert("to".to_string(), serde_json::Value::String(to.to_string()));
        tx.insert(
            "data".to_string(),
            serde_json::Value::String(data.to_string()),
        );
        if let Some(f) = from {
            tx.insert("from".to_string(), serde_json::Value::String(f.to_string()));
        }
        if let Some(v) = value {
            let quantity = rpc_quantity(v)?;
            tx.insert("value".to_string(), serde_json::Value::String(quantity));
        }
        let params = serde_json::json!([serde_json::Value::Object(tx), "latest"]).to_string();
        let raw = self.chain_read(chain, "eth_call", &params)?;
        // chain_read returns the JSON-RPC `result` field verbatim; for eth_call
        // that is a JSON string ("0x...") — unwrap one layer of JSON quoting.
        let hex_data: String = serde_json::from_str(&raw)
            .map_err(|e| format!("eth_call result is not a JSON string: {e}"))?;

        // Error(string) selector — a revert.
        if hex_data.starts_with("0x08c379a0") {
            return Ok(EthCallResult {
                success: false,
                return_data: hex_data,
            });
        }
        if hex_data.starts_with("0x") {
            return Ok(EthCallResult {
                success: true,
                return_data: hex_data,
            });
        }
        Err(format!("eth_call returned unexpected value: {hex_data}"))
    }

    fn chain_id(&mut self, chain: &str) -> Result<u64, String> {
        let raw = self.chain_read(chain, "eth_chainId", "[]")?;
        let hex_data: String = serde_json::from_str(&raw)
            .map_err(|e| format!("eth_chainId result is not a JSON string: {e}"))?;
        let stripped = hex_data.strip_prefix("0x").unwrap_or(&hex_data);
        u64::from_str_radix(stripped, 16).map_err(|e| format!("chain id decode: {e}"))
    }

    fn erc20_allowance(
        &mut self,
        chain: &str,
        token: &str,
        owner: &str,
        spender: &str,
    ) -> Result<String, String> {
        let owner_word = encode_address_word(owner)?;
        let spender_word = encode_address_word(spender)?;
        let data = format!("0xdd62ed3e{owner_word}{spender_word}");
        let res = self.eth_call(chain, token, &data, None, None)?;
        if !res.success {
            return Err(format!("erc20 allowance reverted: {}", res.return_data));
        }
        decode_uint256(&res.return_data)
    }

    fn erc20_balance(&mut self, chain: &str, token: &str, addr: &str) -> Result<String, String> {
        let addr_word = encode_address_word(addr)?;
        let data = format!("0x70a08231{addr_word}");
        let res = self.eth_call(chain, token, &data, None, None)?;
        if !res.success {
            return Err(format!("erc20 balance reverted: {}", res.return_data));
        }
        decode_uint256(&res.return_data)
    }

    fn eth_balance(&mut self, chain: &str, addr: &str) -> Result<String, String> {
        let params = serde_json::json!([addr, "latest"]).to_string();
        let raw = self.chain_read(chain, "eth_getBalance", &params)?;
        let quantity: String = serde_json::from_str(&raw)
            .map_err(|e| format!("eth_getBalance result is not a JSON string: {e}"))?;
        let hex = quantity.strip_prefix("0x").unwrap_or(&quantity);
        U256::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16)
            .map(|value| value.to_string())
            .map_err(|e| format!("native balance decode: {e}"))
    }

    fn erc20_decimals(&mut self, chain: &str, token: &str) -> Result<u8, String> {
        let res = self.eth_call(chain, token, "0x313ce567", None, None)?;
        if !res.success {
            return Err(format!("erc20 decimals reverted: {}", res.return_data));
        }
        let hex = res
            .return_data
            .strip_prefix("0x")
            .unwrap_or(&res.return_data);
        if hex.len() < 64 {
            return Err(format!(
                "decimals return data too short: {}",
                res.return_data
            ));
        }
        let last_two = &hex[hex.len() - 2..];
        u8::from_str_radix(last_two, 16).map_err(|e| format!("decimals decode: {e}"))
    }
}

// --- ABI / hex helpers (no alloy, kept deliberately simple) -----------------

fn rpc_quantity(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        let parsed = U256::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16)
            .map_err(|e| format!("invalid hexadecimal RPC quantity: {e}"))?;
        return Ok(format!("0x{parsed:x}"));
    }
    let parsed = U256::from_str_radix(trimmed, 10)
        .map_err(|e| format!("invalid decimal RPC quantity: {e}"))?;
    Ok(format!("0x{parsed:x}"))
}

/// Encode a 20-byte address as a left-padded 32-byte ABI word (64 lowercase
/// hex chars, no `0x` prefix).
fn encode_address_word(addr: &str) -> Result<String, String> {
    let stripped = addr.strip_prefix("0x").unwrap_or(addr);
    if stripped.len() != 40 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid address: {addr}"));
    }
    Ok(format!("{:0>64}", stripped.to_ascii_lowercase()))
}

/// Decode an ABI-encoded `uint256` word from 0x-prefixed hex return data into
/// a decimal string. The value occupies the final 32 bytes (64 hex chars).
fn decode_uint256(return_data: &str) -> Result<String, String> {
    let hex = return_data.strip_prefix("0x").unwrap_or(return_data);
    if hex.len() < 64 {
        return Err(format!("uint256 return data too short: {return_data}"));
    }
    let word = &hex[hex.len() - 64..];
    hex_to_decimal_str(word)
}

/// Arbitrary-precision hex (no `0x` prefix) → decimal string, so a full
/// `uint256` can be represented without pulling in a big-int crate.
fn hex_to_decimal_str(hex: &str) -> Result<String, String> {
    if hex.is_empty() {
        return Ok("0".to_string());
    }
    // decimal digits, least-significant first
    let mut dec: Vec<u8> = vec![0];
    for ch in hex.chars() {
        let d = ch
            .to_digit(16)
            .ok_or_else(|| format!("invalid hex char '{ch}'"))?;
        // dec = dec * 16
        let mut carry = 0u32;
        for cell in dec.iter_mut() {
            let v = (*cell as u32) * 16 + carry;
            *cell = (v % 10) as u8;
            carry = v / 10;
        }
        while carry > 0 {
            dec.push((carry % 10) as u8);
            carry /= 10;
        }
        // dec += d
        let mut carry = d;
        for cell in dec.iter_mut() {
            let v = (*cell as u32) + carry;
            *cell = (v % 10) as u8;
            carry = v / 10;
            if carry == 0 {
                break;
            }
        }
        while carry > 0 {
            dec.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    let s: String = dec.iter().rev().map(|d| (b'0' + d) as char).collect();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_word_left_pads_to_32_bytes() {
        // 20-byte address left-padded to a 32-byte word (24 zero-hex prefix).
        assert_eq!(
            encode_address_word("0x0123456789abcdef0123456789abcdef01234567").unwrap(),
            "0000000000000000000000000123456789abcdef0123456789abcdef01234567"
        );
    }

    #[test]
    fn address_word_lowercases_and_accepts_no_prefix() {
        assert_eq!(
            encode_address_word("ABCDEF0123456789ABCDEF0123456789ABCDEF01").unwrap(),
            "000000000000000000000000abcdef0123456789abcdef0123456789abcdef01"
        );
    }

    #[test]
    fn address_word_rejects_bad_length() {
        assert!(encode_address_word("0x1234").is_err());
        assert!(encode_address_word("0xZZ3400000000000000000000000000000000000000").is_err());
    }

    #[test]
    fn hex_to_decimal_handles_uint256() {
        assert_eq!(hex_to_decimal_str("").unwrap(), "0");
        assert_eq!(hex_to_decimal_str("a").unwrap(), "10");
        assert_eq!(hex_to_decimal_str("ff").unwrap(), "255");
        // max uint256
        assert_eq!(
            hex_to_decimal_str(&"f".repeat(64)).unwrap(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
    }

    #[test]
    fn decode_uint256_takes_last_word() {
        // 0x...00000000000000000000000000000000000000000000000000000000000003e8 == 1000
        let word = format!(
            "0x{}",
            "00000000000000000000000000000000000000000000000000000000000003e8"
        );
        assert_eq!(decode_uint256(&word).unwrap(), "1000");
    }
}
