//! Integration tests: full create → confirm → abandon lifecycle with a mock Host.
//!
//! These tests exercise the workflow end-to-end through the Host trait,
//! verifying session persistence, policy enforcement, outbox staging,
//! idempotency, and error paths.

use std::collections::HashMap;

use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};

use crate::runtime::{EthCallResult, Host};

/// In-memory mock implementing the full Host trait.
struct MockHost {
    now: u64,
    store: HashMap<String, Vec<u8>>,
    secrets: HashMap<String, Vec<u8>>,
    vfs: HashMap<String, Vec<u8>>,
    tx_counter: u64,
    staged: HashMap<String, StagedTransaction>,
    inspections: HashMap<String, OutboxInspection>,
    /// Pre-built Enso API response body.
    enso_response: Option<Vec<u8>>,
    /// Whether eth_call should succeed.
    eth_call_success: bool,
    /// Chain IDs.
    chain_ids: HashMap<String, u64>,
    /// ERC-20 allowance value to return.
    allowance: String,
    /// Override for erc20_balance (set after create to simulate settlement).
    balance_override: Option<String>,
    /// HTTP status for Enso responses (default 200; set to non-200 to test errors).
    enso_status: u16,
    /// If set, tx_stage will fail after this many successful stages.
    stage_fail_after: Option<usize>,
    /// Count of tx_stage calls made.
    stage_count: usize,
    /// Counter for random() to produce unique session IDs.
    rand_counter: u8,
}

impl MockHost {
    fn new() -> Self {
        let mut chain_ids = HashMap::new();
        chain_ids.insert("ethereum".to_string(), 1);
        chain_ids.insert("base".to_string(), 8453);
        chain_ids.insert("polygon".to_string(), 137);

        let mut vfs = HashMap::new();
        vfs.insert(
            "wallets/test-wallet/address".to_string(),
            b"0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1".to_vec(),
        );
        vfs.insert(
            "wallets/test-wallet/addresses.json".to_string(),
            br#"{"wallet":"test-wallet","kind":"local","policy_status":"not_applicable"}"#.to_vec(),
        );
        vfs.insert(
            "wallets/test-wallet/policy.toml".to_string(),
            br#"
[mev]
max_slippage_bps = 100

[defi]
enabled = true
allowed_source_chains = ["ethereum", "base", "polygon"]
allowed_destination_chains = ["ethereum", "base", "polygon"]
allowed_receivers = ["class:wallet_eoa"]
denied_receivers = []
allowed_routers = [
  "ethereum:0x1234567890abcdef1234567890abcdef12345678",
  "base:0x1234567890abcdef1234567890abcdef12345678",
  "polygon:0x1234567890abcdef1234567890abcdef12345678",
]
denied_protocols = []
allow_unknown_protocols = false
require_calldata_verification = false
"#
            .to_vec(),
        );

        let mut secrets = HashMap::new();
        secrets.insert(
            "credentials/enso-api-key".to_string(),
            b"test-api-key-12345".to_vec(),
        );

        Self {
            now: 1_000_000,
            store: HashMap::new(),
            secrets,
            vfs,
            tx_counter: 0,
            staged: HashMap::new(),
            inspections: HashMap::new(),
            enso_response: None,
            eth_call_success: true,
            chain_ids,
            allowance:
                "115792089237316195423570985008687907853269984665640564039457584007913129639935"
                    .to_string(),
            balance_override: None,
            enso_status: 200,
            stage_fail_after: None,
            stage_count: 0,
            rand_counter: 0,
        }
    }

    /// Set a pre-built Enso route response.
    fn with_enso_response(mut self, body: Vec<u8>) -> Self {
        self.enso_response = Some(body);
        self
    }

    /// Set the ERC-20 allowance value.
    fn with_allowance(mut self, val: &str) -> Self {
        self.allowance = val.to_string();
        self
    }

    fn mark_outbox_success(&mut self, id: &str) {
        self.inspections.insert(
            id.to_string(),
            OutboxInspection {
                outbox_id: id.to_string(),
                state: "success".into(),
                tx_hash: Some(format!("0x{}", "ab".repeat(32))),
                receipt_json: Some(r#"{"outcome":"success"}"#.into()),
            },
        );
    }
}

impl Host for MockHost {
    fn now_ms(&mut self) -> u64 {
        self.now += 1;
        self.now
    }

    fn random(&mut self, len: usize) -> Result<Vec<u8>, String> {
        // Increment the first byte on each call so repeated calls produce different values.
        self.rand_counter += 1;
        let mut v = vec![0x42u8; len];
        v[0] = 0x42 + self.rand_counter;
        Ok(v)
    }

    fn setting(&mut self, _key: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn http(&mut self, req: HttpRequest, _max: usize) -> Result<HttpResponse, String> {
        if req.url.contains("api.enso.finance")
            && let Some(ref body) = self.enso_response
        {
            return Ok(HttpResponse {
                status: self.enso_status,
                headers: vec![],
                body: body.clone(),
            });
        }
        Err(format!("unexpected HTTP request to {}", req.url))
    }

    fn get(&mut self, key: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
        Ok(self.store.get(key).cloned())
    }

    fn get_secret(&mut self, key: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
        Ok(self.secrets.get(key).cloned())
    }

    fn list(&mut self, prefix: &str, _max: usize) -> Result<Vec<String>, String> {
        let mut result: Vec<String> = self
            .store
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        result.sort();
        Ok(result)
    }

    fn put(&mut self, key: &str, value: &[u8], _secret: bool) -> Result<(), String> {
        self.store.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn put_new(&mut self, key: &str, value: &[u8], _secret: bool) -> Result<(), String> {
        if self.store.contains_key(key) {
            return Err("key already exists".into());
        }
        self.store.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn delete_if(&mut self, key: &str, expected: &[u8]) -> Result<(), String> {
        if let Some(actual) = self.store.get(key)
            && actual == expected
        {
            self.store.remove(key);
        }
        Ok(())
    }

    fn vfs_read(&mut self, path: &str, _max: usize) -> Result<Vec<u8>, String> {
        self.vfs
            .get(path)
            .cloned()
            .ok_or_else(|| format!("vfs: not found: {path}"))
    }

    fn chain_read(&mut self, chain: &str, method: &str, _params: &str) -> Result<String, String> {
        match method {
            "eth_call" => {
                if self.eth_call_success {
                    Ok("\"0x\"".to_string())
                } else {
                    Ok("\"0x08c379a0000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000097265766572746564000000000000000000000000000000000000000000000000\"".to_string())
                }
            }
            "eth_chainId" => {
                let id = self.chain_ids.get(chain).unwrap_or(&1);
                Ok(format!("\"0x{:x}\"", id))
            }
            _ => Err(format!("unsupported chain method: {method}")),
        }
    }

    fn tx_stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction, String> {
        self.stage_count += 1;
        if let Some(limit) = self.stage_fail_after
            && self.stage_count > limit
        {
            return Err("simulated staging failure".into());
        }
        self.tx_counter += 1;
        let id = format!("outbox-{}", self.tx_counter);
        let staged = StagedTransaction {
            outbox_id: id.clone(),
            plan_md: format!("# Staged: {} to {}", tx.wallet, tx.to),
            approval: None,
        };
        self.staged.insert(id.clone(), staged.clone());
        Ok(staged)
    }

    fn tx_confirm(
        &mut self,
        _wallet: &str,
        _chain: &str,
        id: &str,
        _warnings: bool,
    ) -> Result<StagedTransaction, String> {
        self.staged
            .get(id)
            .cloned()
            .ok_or_else(|| format!("outbox tx not found: {id}"))
    }

    fn tx_inspect(
        &mut self,
        _wallet: &str,
        _chain: &str,
        id: &str,
    ) -> Result<OutboxInspection, String> {
        if let Some(insp) = self.inspections.get(id) {
            return Ok(insp.clone());
        }
        Ok(OutboxInspection {
            outbox_id: id.to_string(),
            state: "pending".to_string(),
            tx_hash: None,
            receipt_json: None,
        })
    }

    fn eth_call(
        &mut self,
        _chain: &str,
        _to: &str,
        _data: &str,
        _from: Option<&str>,
        _value: Option<&str>,
    ) -> Result<EthCallResult, String> {
        if self.eth_call_success {
            Ok(EthCallResult {
                success: true,
                return_data: "0x".to_string(),
            })
        } else {
            Ok(EthCallResult {
                success: false,
                return_data: "0x08c379a0".to_string(),
            })
        }
    }

    fn chain_id(&mut self, chain: &str) -> Result<u64, String> {
        self.chain_ids
            .get(chain)
            .copied()
            .ok_or_else(|| format!("unknown chain: {chain}"))
    }

    fn erc20_allowance(
        &mut self,
        _chain: &str,
        _token: &str,
        _owner: &str,
        _spender: &str,
    ) -> Result<String, String> {
        Ok(self.allowance.clone())
    }

    fn erc20_balance(&mut self, _chain: &str, _token: &str, _addr: &str) -> Result<String, String> {
        if let Some(ref bal) = self.balance_override {
            return Ok(bal.clone());
        }
        Ok("1000000000000000000".to_string())
    }

    fn eth_balance(&mut self, _chain: &str, _addr: &str) -> Result<String, String> {
        if let Some(ref bal) = self.balance_override {
            return Ok(bal.clone());
        }
        Ok("1000000000000000000".to_string())
    }

    fn erc20_decimals(&mut self, _chain: &str, _token: &str) -> Result<u8, String> {
        Ok(18)
    }
}

/// Build a valid Enso route response with a routeSingle call for ERC-20 swap.
fn build_enso_response_erc20() -> Vec<u8> {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};

    let token_in: Address = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48" // USDC
        .parse()
        .unwrap();
    let amount_in = U256::from(100_000_000u64); // 100.0 USDC (6 decimals)

    // Build Token struct data: (address, uint256)
    let token_data = (token_in, amount_in).abi_encode_params();
    let token = crate::api_types::IEnsoRouter::Token {
        tokenType: 1,
        data: token_data.into(),
    };

    // Build routeSingle calldata
    let route_data = crate::api_types::IEnsoRouter::routeSingleCall {
        tokenIn: token,
        data: vec![].into(),
    }
    .abi_encode();

    let router: Address = "0x1234567890abcdef1234567890abcdef12345678"
        .parse()
        .unwrap();
    let from: Address = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1"
        .parse()
        .unwrap();

    let body = serde_json::json!({
        "tx": {
            "to": format!("0x{:x}", router),
            "data": format!("0x{}", hex::encode(&route_data)),
            "value": "0",
            "from": format!("0x{:x}", from),
        },
        "amountOut": "50000000000000000",
        "gas": "150000",
        "route": [
            {"protocol": "uniswap-v3", "name": "Uniswap V3"}
        ],
        "priceImpact": 0.5
    });
    serde_json::to_vec(&body).unwrap()
}

/// Build a valid Enso route response for native ETH swap.
fn build_enso_response_native() -> Vec<u8> {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};

    let amount_in = U256::from(1_000_000_000_000_000_000u128); // 1.0 ETH

    let token_data = (amount_in,).abi_encode_params();
    let token = crate::api_types::IEnsoRouter::Token {
        tokenType: 0,
        data: token_data.into(),
    };

    let route_data = crate::api_types::IEnsoRouter::routeSingleCall {
        tokenIn: token,
        data: vec![].into(),
    }
    .abi_encode();

    let router: Address = "0x1234567890abcdef1234567890abcdef12345678"
        .parse()
        .unwrap();
    let from: Address = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1"
        .parse()
        .unwrap();

    let body = serde_json::json!({
        "tx": {
            "to": format!("0x{:x}", router),
            "data": format!("0x{}", hex::encode(&route_data)),
            "value": "1000000000000000000",
            "from": format!("0x{:x}", from),
        },
        "amountOut": "3000000000",
        "gas": "120000",
        "route": [
            {"protocol": "uniswap-v3"}
        ],
        "priceImpact": 0.3
    });
    serde_json::to_vec(&body).unwrap()
}

/// Build an ERC-20 route response with a *different* token_in address.
/// Used to test route verification rejection.
fn build_enso_response_wrong_token() -> Vec<u8> {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};

    // Use DAI address instead of USDC — mismatch with "swap usdc" intent.
    let token_in: Address = "0x6b175474e89094c44da98b954eedeac495271d0f" // DAI
        .parse()
        .unwrap();
    let amount_in = U256::from(100_000_000u64);

    let token_data = (token_in, amount_in).abi_encode_params();
    let token = crate::api_types::IEnsoRouter::Token {
        tokenType: 1,
        data: token_data.into(),
    };
    let route_data = crate::api_types::IEnsoRouter::routeSingleCall {
        tokenIn: token,
        data: vec![].into(),
    }
    .abi_encode();

    let router: Address = "0x1234567890abcdef1234567890abcdef12345678"
        .parse()
        .unwrap();
    let from: Address = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1"
        .parse()
        .unwrap();

    let body = serde_json::json!({
        "tx": {
            "to": format!("0x{:x}", router),
            "data": format!("0x{}", hex::encode(&route_data)),
            "value": "0",
            "from": format!("0x{:x}", from),
        },
        "amountOut": "50000000000000000",
        "gas": "150000",
        "route": [{"protocol": "uniswap-v3", "name": "Uniswap V3"}],
        "priceImpact": 0.5
    });
    serde_json::to_vec(&body).unwrap()
}

/// Build an ERC-20 route response with a *wrong* amount (99 USDC instead of 100).
fn build_enso_response_wrong_amount() -> Vec<u8> {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};

    let token_in: Address = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48" // USDC (correct)
        .parse()
        .unwrap();
    let amount_in = U256::from(99_000_000u64); // 99.0 USDC — wrong!

    let token_data = (token_in, amount_in).abi_encode_params();
    let token = crate::api_types::IEnsoRouter::Token {
        tokenType: 1,
        data: token_data.into(),
    };
    let route_data = crate::api_types::IEnsoRouter::routeSingleCall {
        tokenIn: token,
        data: vec![].into(),
    }
    .abi_encode();

    let router: Address = "0x1234567890abcdef1234567890abcdef12345678"
        .parse()
        .unwrap();
    let from: Address = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1"
        .parse()
        .unwrap();

    let body = serde_json::json!({
        "tx": {
            "to": format!("0x{:x}", router),
            "data": format!("0x{}", hex::encode(&route_data)),
            "value": "0",
            "from": format!("0x{:x}", from),
        },
        "amountOut": "50000000000000000",
        "gas": "150000",
        "route": [{"protocol": "uniswap-v3"}],
        "priceImpact": 0.5
    });
    serde_json::to_vec(&body).unwrap()
}

/// Build a native ETH route response where tx.value doesn't match the amount.
fn build_enso_response_native_wrong_value() -> Vec<u8> {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};

    let amount_in = U256::from(1_000_000_000_000_000_000u128); // 1.0 ETH

    let token_data = (amount_in,).abi_encode_params();
    let token = crate::api_types::IEnsoRouter::Token {
        tokenType: 0,
        data: token_data.into(),
    };
    let route_data = crate::api_types::IEnsoRouter::routeSingleCall {
        tokenIn: token,
        data: vec![].into(),
    }
    .abi_encode();

    let router: Address = "0x1234567890abcdef1234567890abcdef12345678"
        .parse()
        .unwrap();
    let from: Address = "0x742d35cc6634c0532925a3b844bc9e7595f0beb1"
        .parse()
        .unwrap();

    let body = serde_json::json!({
        "tx": {
            "to": format!("0x{:x}", router),
            "data": format!("0x{}", hex::encode(&route_data)),
            "value": "999999999999999999", // WRONG — should be 1000000000000000000
            "from": format!("0x{:x}", from),
        },
        "amountOut": "3000000000",
        "gas": "120000",
        "route": [{"protocol": "uniswap-v3"}],
        "priceImpact": 0.3
    });
    serde_json::to_vec(&body).unwrap()
}

// ===========================================================================
// TEST: create → session persisted correctly
// ===========================================================================

#[test]
fn create_persists_session_with_correct_fields() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"{"intent":"swap 100 usdc to eth","chain":"ethereum"}"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Session should be persisted in the store.
    let key = crate::session::key("test-wallet", &id);
    let raw = host.get(&key, 2 * 1024 * 1024).unwrap().unwrap();
    let sess: crate::session::Session = serde_json::from_slice(&raw).unwrap();

    assert_eq!(sess.wallet, "test-wallet");
    assert_eq!(sess.chain, "ethereum");
    assert_eq!(sess.state, "prepared");
    assert!(!sess.plan_md.is_empty());
    assert_eq!(
        sess.intents.len(),
        1,
        "should have 1 intent (route only, sufficient allowance)"
    );
    assert_eq!(sess.intents[0].label, "route");
    assert_eq!(sess.intent_states.len(), 1);
    assert_eq!(sess.intent_states[0].status, "prepared");
    assert!(sess.route.is_some());
    assert!(sess.route_request.is_some());
    assert_eq!(
        sess.route_request.as_ref().unwrap().amount_in.to_string(),
        "100000000",
        "whole-number natural-language amounts are human token units"
    );
    assert!(sess.simulation.is_some());

    // Latest pointer should be stored.
    let latest_key = "intents/test-wallet/latest";
    let latest = host.get(latest_key, 128).unwrap().unwrap();
    assert_eq!(String::from_utf8(latest).unwrap(), id);

    // Policy checks should have route_verified = pass.
    let checks = sess.policy_checks.as_array().unwrap();
    assert!(checks.iter().any(|c| {
        c.get("rule").and_then(|v| v.as_str()) == Some("defi.route_verified")
            && c.get("outcome").and_then(|v| v.as_str()) == Some("pass")
    }));
}

// ===========================================================================
// TEST: create → confirm → session staged with outbox IDs
// ===========================================================================

#[test]
fn full_lifecycle_create_confirm_stages_outbox() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Create
    let body = br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum"}"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Confirm — stage into outbox
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("confirm should succeed");

    // Verify session state
    let key = crate::session::key("test-wallet", &id);
    let raw = host.get(&key, 2 * 1024 * 1024).unwrap().unwrap();
    let sess: crate::session::Session = serde_json::from_slice(&raw).unwrap();

    assert_eq!(sess.state, "staged");
    assert_eq!(sess.staged_ids.len(), 1);
    assert!(sess.staged_ids[0].starts_with("outbox-"));
    assert_eq!(sess.intent_states[0].status, "staged");
    assert!(sess.intent_states[0].outbox_id.is_some());
    assert_eq!(
        sess.intent_states[0].outbox_id.as_ref().unwrap(),
        &sess.staged_ids[0]
    );

    // History should show prepared → staged transition.
    assert!(sess.history.iter().any(|h| h.to == "staged"));
}

// ===========================================================================
// TEST: confirm is idempotent
// ===========================================================================

#[test]
fn confirm_is_idempotent() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // First confirm
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("first confirm should succeed");

    let first_tx_counter = host.tx_counter;

    // Second confirm should be a no-op
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("second confirm should succeed (idempotent)");

    // No new tx should have been staged
    assert_eq!(host.tx_counter, first_tx_counter);
}

#[test]
fn empty_confirmation_is_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(&mut host, "test-wallet", br#"swap 100 usdc to eth"#).unwrap();

    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"").unwrap_err();
    assert!(error.contains("exactly `confirm`"), "{error}");
    assert_eq!(host.tx_counter, 0);
}

#[test]
fn stale_passkey_policy_is_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    host.vfs.insert(
        "wallets/test-wallet/addresses.json".into(),
        br#"{"wallet":"test-wallet","kind":"passkey","policy_status":"stale"}"#.to_vec(),
    );

    let error =
        crate::workflow::create(&mut host, "test-wallet", b"swap 100 usdc to eth").unwrap_err();
    assert!(error.contains("current policy signature"), "{error}");
}

// ===========================================================================
// TEST: abandon a session before staging
// ===========================================================================

#[test]
fn abandon_before_staging_works() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    crate::workflow::abandon(&mut host, "test-wallet", &id).expect("abandon should succeed");

    let key = crate::session::key("test-wallet", &id);
    let raw = host.get(&key, 2 * 1024 * 1024).unwrap().unwrap();
    let sess: crate::session::Session = serde_json::from_slice(&raw).unwrap();

    assert_eq!(sess.state, "abandoned");
    assert!(sess.terminal());
}

// ===========================================================================
// TEST: abandon after staging fails
// ===========================================================================

#[test]
fn abandon_after_staging_fails() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("confirm should succeed");

    let err = crate::workflow::abandon(&mut host, "test-wallet", &id)
        .expect_err("abandon should fail after staging");
    assert!(err.contains("cannot abandon"));
}

// ===========================================================================
// TEST: wrong wallet can't load session (key scoping)
// ===========================================================================

#[test]
fn wrong_wallet_cannot_load_session() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Loading under a different wallet should fail — session key is scoped.
    let err = crate::workflow::confirm(&mut host, "wrong-wallet", &id, b"confirm")
        .expect_err("confirm should fail with wrong wallet");
    assert!(err.contains("not found") || err.contains("wallet mismatch"));
}

// ===========================================================================
// TEST: load + serialize roundtrip preserves all fields
// ===========================================================================

#[test]
fn session_roundtrip_preserves_all_fields() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum"}"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    let json = serde_json::to_vec(&sess).unwrap();
    let sess2: crate::session::Session = serde_json::from_slice(&json).unwrap();

    assert_eq!(sess.id, sess2.id);
    assert_eq!(sess.wallet, sess2.wallet);
    assert_eq!(sess.chain, sess2.chain);
    assert_eq!(sess.state, sess2.state);
    assert_eq!(sess.intents.len(), sess2.intents.len());
    assert_eq!(sess.intent_states.len(), sess2.intent_states.len());
    assert_eq!(sess.plan_md, sess2.plan_md);
    assert_eq!(sess.history.len(), sess2.history.len());
}

// ===========================================================================
// TEST: native ETH swap creates correct route intent with value
// ===========================================================================

#[test]
fn native_eth_swap_has_correct_value() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_native());

    let body = br#"swap 1.0 eth to usdc"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    assert_eq!(sess.intents.len(), 1);
    let route_intent = &sess.intents[0];
    assert_eq!(route_intent.label, "route");
    assert_eq!(route_intent.value_wei, "1000000000000000000");
}

// ===========================================================================
// TEST: policy_overall aggregates correctly
// ===========================================================================

#[test]
fn policy_overall_aggregation() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    // Minnow-style policy accepts quote-only receiver/min-output facts with
    // explicit warnings, so the aggregate is warn rather than a silent pass.
    assert_eq!(sess.policy_overall(), "warn");
}

// ===========================================================================
// TEST: ERC-20 with sufficient allowance → no approve intent needed
// ===========================================================================

#[test]
fn erc20_sufficient_allowance_no_approve() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    // Should have exactly 1 intent (route only), no approve.
    assert_eq!(sess.intents.len(), 1);
    assert_eq!(sess.intents[0].label, "route");
    assert!(!sess.intents.iter().any(|i| i.label == "approve"));
}

// ===========================================================================
// TEST: ERC-20 with zero allowance → approve intent added
// ===========================================================================

#[test]
fn erc20_zero_allowance_adds_approve_intent() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0");

    let body = br#"swap 100.0 usdc to eth"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    // Should have 2 intents: approve + route.
    assert_eq!(sess.intents.len(), 2);
    assert_eq!(sess.intents[0].label, "approve");
    assert_eq!(sess.intents[1].label, "route");

    // Approve intent should have token and spender.
    assert!(sess.intents[0].approve_token.is_some());
    assert!(sess.intents[0].approve_spender.is_some());

    // depends_on is set during confirm, not create — verify in lifecycle test.
}

// ===========================================================================
// TEST: session not found returns error
// ===========================================================================

#[test]
fn load_nonexistent_session_errors() {
    let mut host = MockHost::new();

    let result = crate::workflow::load(&mut host, "test-wallet", "nonexistent-id");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// ===========================================================================
// TEST: history trimming prevents unbounded growth
// ===========================================================================

#[test]
fn history_trims_at_100_entries() {
    use crate::session::Session;

    let mut sess = Session {
        schema_version: 1,
        id: "test".into(),
        wallet: "w".into(),
        wallet_address: "0x0".into(),
        chain: "ethereum".into(),
        destination_chain: None,
        intent_text: "test".into(),
        route_request: None,
        route: None,
        plan_md: String::new(),
        intents: vec![],
        intent_states: vec![],
        staged_ids: vec![],
        created_ms: 0,
        updated_ms: 0,
        state: "init".into(),
        observed_before: None,
        policy_checks: serde_json::Value::Null,
        receiver_class: None,
        simulation: None,
        last_error: None,
        history: vec![],
    };

    for i in 0..105 {
        sess.transition(i, &format!("state-{i}"), "test");
    }
    assert!(sess.history.len() <= 100);
}

// ===========================================================================
// TEST: simulation failure is fail-closed before session creation
// ===========================================================================

#[test]
fn simulation_failure_rejects_session() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    host.eth_call_success = false;

    let body = br#"swap 100.0 usdc to eth"#;
    let error = crate::workflow::create(&mut host, "test-wallet", body)
        .expect_err("create must reject a failed simulation");
    assert!(error.contains("simulation failed"), "{error}");
    assert!(
        !host.store.keys().any(|key| key.ends_with("/session.json")),
        "failed simulations must not leave a stage-eligible session"
    );
}

// ===========================================================================
// TEST: create → confirm → confirm with approve (2 intents, both staged)
// ===========================================================================

#[test]
fn full_lifecycle_with_approve_two_intents() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0");

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Verify 2 intents created
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.intents.len(), 2);

    // First confirmation stages only the exact-amount approval.
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("confirm should succeed");

    let mut sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "awaiting_approval");
    assert_eq!(sess.staged_ids.len(), 1);
    assert_eq!(sess.intent_states[0].status, "staged");
    assert_eq!(sess.intent_states[1].status, "prepared");
    assert!(sess.intent_states[1].outbox_id.is_none());

    let approval_id = sess.intent_states[0].outbox_id.clone().unwrap();
    host.mark_outbox_success(&approval_id);
    host.allowance =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935".into();

    // The second confirmation verifies the receipt and allowance, simulates,
    // then stages the route.
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("route confirmation should succeed");

    sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "staged");
    assert_eq!(sess.staged_ids.len(), 2);
    assert_eq!(sess.intent_states[0].status, "confirmed");
    assert_eq!(sess.intent_states[1].status, "staged");
    assert!(sess.intent_states[1].depends_on.is_some());
}

// ===========================================================================
// BUG HUNT: confirm() should reject an abandoned session
// ===========================================================================

#[test]
fn confirm_on_abandoned_session_should_fail() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Abandon the session first
    crate::workflow::abandon(&mut host, "test-wallet", &id).expect("abandon should succeed");

    // Now try to confirm — this should be rejected
    let result = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm");

    assert!(
        result.is_err(),
        "confirm should NOT revive an abandoned session"
    );

    // Session must still be abandoned with nothing staged
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "abandoned");
    assert!(sess.staged_ids.is_empty());
    assert!(sess.terminal());
}

// ===========================================================================
// BUG HUNT: same-chain with different casing should NOT be cross-chain
// ===========================================================================

#[test]
fn same_chain_different_case_not_cross_chain() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Same chain, different casing — this is NOT cross-chain
    let body =
        br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","destination_chain":"Ethereum"}"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    // Should NOT have a cross_chain warning
    let has_cross_chain = sess
        .policy_checks
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.get("rule").and_then(|v| v.as_str()) == Some("cross_chain"));

    assert!(
        !has_cross_chain,
        "same-chain swap with different casing should NOT trigger cross_chain warning"
    );
}

// ===========================================================================
// TEST: route verification rejects wrong token_in
// ===========================================================================

#[test]
fn create_rejects_route_with_mismatched_token() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_wrong_token());

    let body = br#"swap 100.0 usdc to eth"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("does not match"),
        "should reject mismatched token: {err}"
    );
}

// ===========================================================================
// TEST: route verification rejects wrong amount
// ===========================================================================

#[test]
fn create_rejects_route_with_mismatched_amount() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_wrong_amount());

    let body = br#"swap 100.0 usdc to eth"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not match"));
}

// ===========================================================================
// TEST: route verification rejects native ETH with wrong value
// ===========================================================================

#[test]
fn create_rejects_native_route_with_wrong_value() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_native_wrong_value());

    let body = br#"swap 1.0 eth to usdc"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not match"));
}

// ===========================================================================
// TEST: Enso API error propagates
// ===========================================================================

#[test]
fn enso_api_error_propagates() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    host.enso_status = 500;

    let body = br#"swap 100.0 usdc to eth"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("status 500"));
}

// ===========================================================================
// TEST: chain ID mismatch at confirm time is rejected
// ===========================================================================

#[test]
fn chain_id_mismatch_rejected_at_confirm() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Mutate chain_id after create — simulate chain re-organization
    host.chain_ids.insert("ethereum".to_string(), 999);

    let result = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("chain_id"));
}

// ===========================================================================
// TEST: settlement status is "not_broadcast" for prepared session
// ===========================================================================

#[test]
fn settlement_not_broadcast_for_prepared_session() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Use DAI as output (non-native) so settlement tracking is active
    let body = br#"swap 100.0 usdc to dai"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    let status = crate::settlement::settlement_status(&mut host, &sess);
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("not_staged")
    );
}

// ===========================================================================
// TEST: native output uses eth_getBalance rather than being unsupported
// ===========================================================================

#[test]
fn settlement_native_output_is_trackable() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Before the route is staged, native output has the same safe lifecycle
    // classification as ERC-20 output.
    let body = br#"swap 100.0 usdc to eth"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    let status = crate::settlement::settlement_status(&mut host, &sess);
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("not_staged")
    );
}

// ===========================================================================
// TEST: settlement detects received funds after balance increase
// ===========================================================================

#[test]
fn settlement_received_after_balance_increase() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to dai"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();

    // Stage the intents so staged_ids is non-empty
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let route_id = sess.intent_states[0].outbox_id.clone().unwrap();
    host.mark_outbox_success(&route_id);

    // Now simulate balance increase on destination chain
    host.balance_override = Some("2000000000000000000".to_string());

    let status = crate::settlement::settlement_status(&mut host, &sess);

    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("destination_received")
    );
    let delta = status.get("delta").and_then(|v| v.as_str()).unwrap();
    assert_ne!(delta, "0", "delta should be non-zero");
}

// ===========================================================================
// TEST: settlement does not inspect destination before source success
// ===========================================================================

#[test]
fn settlement_source_pending_before_receipt() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to dai"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let status = crate::settlement::settlement_status(&mut host, &sess);

    // A staged entry is not a broadcast or a mined success.
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("source_pending")
    );
}

// ===========================================================================
// TEST: partial staging failure leaves session in consistent state
// ===========================================================================

#[test]
fn partial_staging_failure_is_recoverable() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0"); // creates 2 intents: approve + route

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    // Make tx_stage fail after 1 successful stage. The first confirmation
    // stages only approval and must still succeed.
    host.stage_fail_after = Some(1);
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("approval staging should succeed");

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let approval_id = sess.intent_states[0].outbox_id.clone().unwrap();
    host.mark_outbox_success(&approval_id);
    host.allowance =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935".into();

    // The route staging attempt now fails without duplicating approval.
    let result = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm");
    assert!(result.is_err(), "route staging should fail");

    // Session should have first intent staged but not second
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.intent_states[0].status, "confirmed");
    assert_eq!(sess.intent_states[1].status, "prepared");
    assert!(sess.intent_states[0].outbox_id.is_some());
    assert!(sess.intent_states[1].outbox_id.is_none());

    // Now remove the failure and retry — should stage the remaining intent
    host.stage_fail_after = None;
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("retry confirm should succeed");

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "staged");
    assert_eq!(sess.staged_ids.len(), 2);
    assert_eq!(sess.intent_states[1].status, "staged");
}

// ===========================================================================
// TEST: double confirm with approve (2 intents) is idempotent
// ===========================================================================

#[test]
fn double_confirm_with_approve_is_idempotent() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0");

    let body = br#"swap 100.0 usdc to eth"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let approval_id = sess.intent_states[0].outbox_id.clone().unwrap();
    host.mark_outbox_success(&approval_id);
    host.allowance =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935".into();
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let count_after_route = host.tx_counter;

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(
        host.tx_counter, count_after_route,
        "confirmation after the route is staged must be idempotent"
    );
}

// ===========================================================================
// TEST: slippage_bps from JSON is applied to route request
// ===========================================================================

#[test]
fn slippage_above_wallet_policy_is_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","slippage_bps":200}"#;
    let error = crate::workflow::create(&mut host, "test-wallet", body).unwrap_err();
    assert!(error.contains("max_slippage"), "{error}");
}

// ===========================================================================
// TEST: default slippage is 50 bps when not specified
// ===========================================================================

#[test]
fn default_slippage_is_50_bps() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    assert_eq!(sess.route_request.as_ref().unwrap().slippage_bps, 50);
}

// ===========================================================================
// TEST: multiple sessions for same wallet are independent
// ===========================================================================

#[test]
fn multiple_sessions_same_wallet_are_independent() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let id1 =
        crate::workflow::create(&mut host, "test-wallet", br#"swap 100.0 usdc to eth"#).unwrap();
    // Same intent — IDs differ because random() generates unique session IDs.
    let id2 =
        crate::workflow::create(&mut host, "test-wallet", br#"swap 100.0 usdc to eth"#).unwrap();

    assert_ne!(id1, id2, "sessions should have different IDs");

    // Confirm only the first
    crate::workflow::confirm(&mut host, "test-wallet", &id1, b"confirm").unwrap();

    let sess1 = crate::workflow::load(&mut host, "test-wallet", &id1).unwrap();
    let sess2 = crate::workflow::load(&mut host, "test-wallet", &id2).unwrap();

    assert_eq!(sess1.state, "staged");
    assert_eq!(sess2.state, "prepared");
    assert!(!sess1.staged_ids.is_empty());
    assert!(sess2.staged_ids.is_empty());
}

// ===========================================================================
// TEST: invalid wallet name is rejected
// ===========================================================================

#[test]
fn invalid_wallet_name_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Wallet name with slash — path traversal attempt
    let result = crate::workflow::create(&mut host, "../etc", br#"swap 100.0 usdc to eth"#);
    assert!(result.is_err());

    // Empty wallet name
    let result = crate::workflow::create(&mut host, "", br#"swap 100.0 usdc to eth"#);
    assert!(result.is_err());

    // Wallet name with space
    let result = crate::workflow::create(&mut host, "test wallet", br#"swap 100.0 usdc to eth"#);
    assert!(result.is_err());
}

// ===========================================================================
// TEST: empty intent body is rejected
// ===========================================================================

#[test]
fn empty_intent_body_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let result = crate::workflow::create(&mut host, "test-wallet", b"confirm");
    assert!(result.is_err());

    let result = crate::workflow::create(&mut host, "test-wallet", b"   ");
    assert!(result.is_err());
}

// ===========================================================================
// TEST: unparseable intent is rejected
// ===========================================================================

#[test]
fn unparseable_intent_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Missing "to" keyword
    let result = crate::workflow::create(&mut host, "test-wallet", b"hello world foo bar");
    assert!(result.is_err());

    // Missing amount
    let result = crate::workflow::create(&mut host, "test-wallet", b"swap usdc to eth");
    assert!(result.is_err());
}

// ===========================================================================
// TEST: unknown token symbol is rejected
// ===========================================================================

#[test]
fn unknown_token_symbol_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let result = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 nonexistenttoken to eth",
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("could not resolve"));
}
