use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};

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
}
