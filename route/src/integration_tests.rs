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
    /// If set, tx_inspect fails with this error.
    inspect_error: Option<petal::SdkError>,
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
    /// Number of Enso route requests served.
    enso_calls: usize,
    /// URLs of every Enso route request, in order.
    enso_urls: Vec<String>,
    /// Calldata of every staged transaction, in staging order.
    staged_data: Vec<String>,
    bridge_calls: usize,
    destination_calls: usize,
    confirm_on_session_read: Option<String>,
    abandon_on_stage: Option<String>,
    concurrent_result: Option<Result<(), String>>,
}

impl MockHost {
    fn new() -> Self {
        let mut chain_ids = HashMap::new();
        chain_ids.insert("ethereum".to_string(), 1);
        chain_ids.insert("base".to_string(), 8453);
        chain_ids.insert("polygon".to_string(), 137);

        let mut vfs = HashMap::new();
        vfs.insert(
            "wallets/test-wallet/0/address.evm".to_string(),
            b"0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1".to_vec(),
        );
        let mut store = HashMap::new();
        store.insert(
            "settings/test-wallet/venue.toml".to_string(),
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
            store,
            secrets,
            vfs,
            tx_counter: 0,
            staged: HashMap::new(),
            inspections: HashMap::new(),
            inspect_error: None,
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
            enso_calls: 0,
            enso_urls: Vec::new(),
            staged_data: Vec::new(),
            bridge_calls: 0,
            destination_calls: 0,
            confirm_on_session_read: None,
            abandon_on_stage: None,
            concurrent_result: None,
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
                // Machine's MinedReceipt shape: no logs.
                receipt_json: Some(
                    serde_json::json!({
                        "outcome": "success",
                        "tx_hash": format!("0x{}", "ab".repeat(32)),
                        "block_number": 1,
                        "revert_reason": null,
                    })
                    .to_string(),
                ),
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
        if req.url.contains("/bridge/check?") {
            self.bridge_calls += 1;
            return Err("destination provider tracking is not supported".into());
        }
        if req.url.contains("api.enso.finance")
            && let Some(ref body) = self.enso_response
        {
            self.enso_calls += 1;
            self.enso_urls.push(req.url.clone());
            return Ok(HttpResponse {
                status: self.enso_status,
                headers: vec![],
                body: body.clone(),
            });
        }
        Err(format!("unexpected HTTP request to {}", req.url))
    }

    fn get(&mut self, key: &str, _max: usize) -> Result<Option<Vec<u8>>, String> {
        let snapshot = self.store.get(key).cloned();
        if key.ends_with("/session.json")
            && let Some(id) = self.confirm_on_session_read.take()
        {
            self.concurrent_result = Some(crate::workflow::confirm(
                self,
                "test-wallet",
                &id,
                b"confirm",
            ));
        }
        Ok(snapshot)
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
            "eth_getCode" => Ok("\"0x\"".into()),
            "eth_getTransactionReceipt" | "eth_getTransactionByHash" => {
                self.destination_calls += 1;
                Err("destination receipt tracking is not supported".into())
            }
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
        self.staged_data.push(tx.data_hex.clone());
        let id = format!("outbox-{}", self.tx_counter);
        let staged = StagedTransaction {
            outbox_id: id.clone(),
            plan_md: format!("# Staged: {} to {}", tx.wallet, tx.to),
            approval: None,
        };
        self.staged.insert(id.clone(), staged.clone());
        if let Some(session_id) = self.abandon_on_stage.take() {
            self.concurrent_result =
                Some(crate::workflow::abandon(self, "test-wallet", &session_id));
        }
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
    ) -> Result<OutboxInspection, petal::SdkError> {
        if let Some(error) = &self.inspect_error {
            return Err(error.clone());
        }
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
    build_enso_response_native_amount(1_000_000_000_000_000_000u128) // 1.0 ETH
}

/// Build a native-ETH route response spending `wei`.
fn build_enso_response_native_amount(wei: u128) -> Vec<u8> {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};

    let amount_in = U256::from(wei);

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
            "value": wei.to_string(),
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

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"swap 100 usdc to eth on ethereum"#,
    )
    .unwrap();

    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"").unwrap_err();
    assert!(error.contains("exactly `confirm`"), "{error}");
    assert_eq!(host.tx_counter, 0);
}

#[test]
fn missing_venue_configuration_allows_canonical_route() {
    let response = String::from_utf8(build_enso_response_erc20())
        .unwrap()
        .replace(
            "0x1234567890abcdef1234567890abcdef12345678",
            "0xf75584ef6673ad213a685a1b58cc0330b8ea22cf",
        );
    let mut host = MockHost::new().with_enso_response(response.into_bytes());
    host.store.clear();
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100 usdc to eth on ethereum",
    )
    .unwrap();
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert!(host.stage_count > 0);
}

#[test]
fn venue_settings_use_namespaced_keys_and_preserve_custom_configuration() {
    let mut host = MockHost::new();
    for wallet in ["api-key", "StatusJson", "wallets"] {
        crate::policy::write_venue_config(&mut host, wallet, b"[defi]\nenabled = false\n").unwrap();
        assert!(
            host.store
                .contains_key(&format!("settings/wallets/{wallet}/venue.toml"))
        );
        assert!(
            !crate::policy::load_venue_config(&mut host, wallet)
                .unwrap()
                .defi
                .enabled
        );
    }
}

#[test]
fn venue_defaults_are_identical_when_read_and_loaded() {
    let mut host = MockHost::new();
    host.store.clear();
    let bytes = crate::policy::read_venue_config(&mut host, "test-wallet").unwrap();
    let document: toml::Value = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
    assert_eq!(document["defi"]["enabled"].as_bool(), Some(true));
    let loaded = crate::policy::load_venue_config(&mut host, "test-wallet").unwrap();
    assert!(loaded.defi.enabled);
    assert_eq!(loaded.max_slippage_bps, Some(100));
    assert!(loaded.defi.allow_unknown_protocols);
    assert!(!loaded.defi.require_calldata_verification);
    for chain in [
        "ethereum",
        "optimism",
        "bsc",
        "polygon",
        "base",
        "arbitrum",
        "avalanche",
    ] {
        assert!(loaded.defi.allowed_source_chains.contains(chain));
        assert!(loaded.defi.allowed_destination_chains.contains(chain));
        assert!(loaded.defi.allowed_routers.contains(&format!(
            "{chain}:0xf75584ef6673ad213a685a1b58cc0330b8ea22cf"
        )));
    }
}

#[test]
fn venue_defaults_cover_bloom_enso_intersection() {
    let mut host = MockHost::new();
    host.store.clear();
    let policy = crate::policy::load_venue_config(&mut host, "test-wallet").unwrap();
    let common = "0xf75584ef6673ad213a685a1b58cc0330b8ea22cf";
    for (chain, router) in [
        ("ethereum", common),
        ("base", common),
        ("arbitrum", common),
        ("optimism", common),
        ("polygon", common),
        ("bsc", common),
        ("avalanche", common),
        ("gnosis", common),
        ("hyperliquid", common),
        ("linea", "0xa146d46823f3f594b785200102be5385cafce9b5"),
        ("robinhood", "0xcfbaa9cfce952ca4f4069874ff1df8c05e37a3c7"),
        ("tempo", "0xcfbaa9cfce952ca4f4069874ff1df8c05e37a3c7"),
    ] {
        assert!(policy.defi.allowed_source_chains.contains(chain), "{chain}");
        assert!(
            policy.defi.allowed_destination_chains.contains(chain),
            "{chain}"
        );
        assert!(
            policy
                .defi
                .allowed_routers
                .contains(&format!("{chain}:{router}"))
        );
    }
    assert_eq!(policy.defi.allowed_source_chains.len(), 12);
    assert_eq!(policy.defi.allowed_destination_chains.len(), 12);
    assert!(!policy.defi.allowed_source_chains.contains("arc"));
    assert!(!policy.defi.allowed_source_chains.contains("unsupported"));
    assert!(
        !policy
            .defi
            .allowed_destination_chains
            .contains("unsupported")
    );
}

// ===========================================================================
// TEST: abandon a session before staging
// ===========================================================================

#[test]
fn abandon_before_staging_works() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let id =
        crate::workflow::create(&mut host, "test-wallet", body).expect("create should succeed");

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("confirm should succeed");

    let err = crate::workflow::abandon(&mut host, "test-wallet", &id)
        .expect_err("abandon should fail after staging");
    assert!(err.contains("is in the outbox"), "{err}");
}

// ===========================================================================
// TEST: wrong wallet can't load session (key scoping)
// ===========================================================================

#[test]
fn wrong_wallet_cannot_load_session() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let body = br#"swap 1.0 eth to usdc on ethereum"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    assert_eq!(sess.intents.len(), 1);
    let route_intent = &sess.intents[0];
    assert_eq!(route_intent.label, "route");
    assert_eq!(route_intent.value_wei, "1000000000000000000");
}

// ===========================================================================
// TEST: native ETH → cbBTC on Base resolves through the static registry
// ===========================================================================

#[test]
fn native_eth_to_cbbtc_on_base_resolves() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_native_amount(1_000_000_000_000_000));

    let body = br#"swap 0.001 ETH to cbBTC on base"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body)
        .expect("cbBTC on base should resolve");

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.chain, "base");
    let req = sess.route_request.as_ref().unwrap();
    assert_eq!(req.chain_id, 8453);
    assert_eq!(
        req.token_in,
        crate::api_types::NATIVE_TOKEN
            .parse::<alloy::primitives::Address>()
            .unwrap()
    );
    assert_eq!(
        req.token_out,
        "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"
            .parse::<alloy::primitives::Address>()
            .unwrap()
    );
    assert_eq!(req.amount_in.to_string(), "1000000000000000");
    assert_eq!(sess.intents[0].value_wei, "1000000000000000");
}

/// Public ceremony input: actual Petal output, with a deterministic mock quote.
/// This is not evidence that the shortcut executes or that funds were received.
#[test]
fn protected_swap_ceremony_fixture() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&build_enso_response_native_amount(1_000_000_000_000_000)).unwrap();
    response["tx"]["to"] = serde_json::json!("0xf75584ef6673ad213a685a1b58cc0330b8ea22cf");
    response["amountOut"] = serde_json::json!("3000000");
    let mut host = MockHost::new().with_enso_response(serde_json::to_vec(&response).unwrap());
    let policy = host
        .store
        .get_mut("settings/test-wallet/venue.toml")
        .unwrap();
    *policy = String::from_utf8(policy.clone())
        .unwrap()
        .replace(
            "0x1234567890abcdef1234567890abcdef12345678",
            "0xf75584ef6673ad213a685a1b58cc0330b8ea22cf",
        )
        .into_bytes();
    let id = crate::workflow::create(&mut host, "test-wallet", b"swap 0.001 eth to usdc on base")
        .unwrap();
    let session = load_session(&mut host, &id);
    assert_eq!(
        crate::calldata::output_minimum(
            session.route.as_ref().unwrap(),
            session.route_request.as_ref().unwrap()
        )
        .unwrap()
        .to_string(),
        "2985000"
    );
    let fixture = serde_json::json!({
        "provenance": "Enso Petal protected_swap_ceremony_fixture; mocked 3 USDC quote; no execution or settlement evidence",
        "chain_id": 8453,
        "sender": session.wallet_address,
        "transaction": session.intents.last().unwrap(),
    });
    if let Some(directory) = std::env::var_os("BLOOM_UI_FIXTURE_DIR") {
        std::fs::write(
            std::path::Path::new(&directory).join("enso-petal-swap.json"),
            serde_json::to_vec_pretty(&fixture).unwrap(),
        )
        .unwrap();
    }
}

// ===========================================================================
// TEST: policy_overall aggregates correctly
// ===========================================================================

#[test]
fn policy_overall_aggregation() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    // Same-chain receiver/minimum now come from the locally prepared safe call.
    assert_eq!(sess.policy_overall(), "pass");
}

#[test]
fn same_chain_routes_work_with_required_calldata_verification() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let policy = host
        .store
        .get_mut("settings/test-wallet/venue.toml")
        .unwrap();
    *policy = String::from_utf8(policy.clone())
        .unwrap()
        .replace(
            "require_calldata_verification = false",
            "require_calldata_verification = true",
        )
        .into_bytes();
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to eth on ethereum",
    )
    .unwrap();
    let session = load_session(&mut host, &id);
    assert!(session.plan_md.contains("Calldata minimum output:"));
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(host.staged.len(), 1);
    assert!(
        load_session(&mut host, &id)
            .policy_checks
            .to_string()
            .contains("minimum-output floor is enforced")
    );
}

#[test]
fn tampered_stored_safe_output_is_refused_before_staging() {
    use alloy::primitives::{Address, U256};
    use alloy::sol_types::{SolCall, SolValue};
    for change in ["receiver", "token", "minimum"] {
        let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
        let id = crate::workflow::create(
            &mut host,
            "test-wallet",
            b"swap 100.0 usdc to eth on ethereum",
        )
        .unwrap();
        let mut session = load_session(&mut host, &id);
        let route = session.route.as_mut().unwrap();
        let mut call =
            crate::api_types::IEnsoRouter::safeRouteSingleCall::abi_decode(&route.tx.data).unwrap();
        match change {
            "receiver" => call.receiver = Address::repeat_byte(0x77),
            "token" => {
                call.tokenOut.tokenType = 1;
                call.tokenOut.data = (Address::repeat_byte(0x77), U256::MAX)
                    .abi_encode_params()
                    .into();
            }
            _ => call.tokenOut.data = (U256::ZERO,).abi_encode_params().into(),
        }
        route.tx.data = call.abi_encode().into();
        session.intents.last_mut().unwrap().data_hex = format!("0x{}", hex::encode(&route.tx.data));
        host.store
            .insert(session.key(), serde_json::to_vec(&session).unwrap());
        assert!(
            crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").is_err(),
            "{change}"
        );
        assert!(host.staged.is_empty());
    }
}

// ===========================================================================
// TEST: ERC-20 with sufficient allowance → no approve intent needed
// ===========================================================================

#[test]
fn erc20_sufficient_allowance_no_approve() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let result = crate::workflow::load(&mut host, "test-wallet", "0000000000000000");
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

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let error = crate::workflow::create(&mut host, "test-wallet", body)
        .expect_err("create must reject a failed simulation");
    assert!(error.message().contains("simulation failed"), "{error}");
    assert!(
        !host.store.keys().any(|key| key.ends_with("/session.json")),
        "failed simulations must not leave a stage-eligible session"
    );
}

// ===========================================================================
// TEST: a refused confirmation is readable from status.json
// ===========================================================================

#[test]
fn refused_confirmation_is_recorded_in_the_session_and_cleared_on_success() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0");
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"swap 100.0 usdc to eth on ethereum"#,
    )
    .expect("create should succeed");
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("first confirmation stages the approval");

    // Confirming again before the approval lands is refused; over a mount the
    // caller sees only EIO, so the reason must be in the session.
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(
        error.contains("broadcast it and wait for a successful receipt"),
        "{error}"
    );
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "awaiting_approval");
    assert_eq!(sess.last_error.as_deref(), Some(error.as_str()));
    assert_eq!(sess.staged_ids.len(), 1, "nothing new may be staged");

    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"yes").unwrap_err();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.last_error.as_deref(), Some(error.as_str()));

    let approval_id = sess.intent_states[0].outbox_id.clone().unwrap();
    host.mark_outbox_success(&approval_id);
    host.allowance =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935".into();
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm")
        .expect("route confirmation should succeed");
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "staged");
    assert_eq!(sess.last_error, None);

    // An idempotent retry is also a successful confirmation: a malformed
    // write after staging must not leave an error after a valid retry.
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"yes").unwrap_err();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.last_error.as_deref(), Some(error.as_str()));
    let staged_ids = sess.staged_ids.clone();
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "staged");
    assert_eq!(sess.last_error, None);
    assert_eq!(sess.staged_ids, staged_ids, "retry must not stage again");
}

// ===========================================================================
// TEST: create → confirm → confirm with approve (2 intents, both staged)
// ===========================================================================

#[test]
fn full_lifecycle_with_approve_two_intents() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0");

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

/// A staged swap can be abandoned once its outbox entry is discarded, never
/// while it could still execute.
#[test]
fn a_discarded_swap_can_be_abandoned() {
    let (mut host, id) = quoted_session(10_000);
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let route_id = load_session(&mut host, &id).intent_states[0]
        .outbox_id
        .clone()
        .unwrap();
    let error = crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap_err();
    assert!(error.contains("`cancel` to its `confirm` file"), "{error}");
    host.inspections.insert(
        route_id.clone(),
        OutboxInspection {
            outbox_id: route_id,
            state: "cancelled".into(),
            tx_hash: None,
            receipt_json: None,
        },
    );
    crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(load_session(&mut host, &id).state, "abandoned");
}

#[test]
fn abandon_and_confirm_serialize_after_approval_success() {
    for abandon_first in [false, true] {
        let mut host = MockHost::new()
            .with_enso_response(build_enso_response_erc20())
            .with_allowance("0");
        let id = crate::workflow::create(
            &mut host,
            "test-wallet",
            b"swap 100 usdc to eth on ethereum",
        )
        .unwrap();
        crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
        let approval_id = load_session(&mut host, &id).intent_states[0]
            .outbox_id
            .clone()
            .unwrap();
        host.inspections.insert(
            approval_id.clone(),
            OutboxInspection {
                outbox_id: approval_id,
                state: "success".into(),
                tx_hash: None,
                receipt_json: None,
            },
        );
        host.allowance = "100000000".into();
        if abandon_first {
            // Pause abandon after reading its snapshot; attempt a full confirm
            // before returning it. Without the lock this hides a staged swap.
            host.confirm_on_session_read = Some(id.clone());
            crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap();
            assert_eq!(load_session(&mut host, &id).state, "abandoned");
            assert_eq!(host.staged.len(), 1, "only the approval may exist");
        } else {
            // Attempt abandon after durable staging but before session save.
            host.abandon_on_stage = Some(id.clone());
            crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
            let session = load_session(&mut host, &id);
            assert_eq!(session.state, "staged");
            assert!(session.intent_states[1].outbox_id.is_some());
            assert_eq!(host.staged.len(), 2);
            let error = crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap_err();
            assert!(error.contains("discard"), "{error}");
            assert_eq!(
                load_session(&mut host, &id).intent_states[1].outbox_id,
                session.intent_states[1].outbox_id
            );
        }
        let error = host.concurrent_result.take().unwrap().unwrap_err();
        assert!(error.contains("already in progress"), "{error}");
        assert!(
            !host
                .store
                .contains_key(&format!("intents/test-wallet/{id}/confirm.lock"))
        );
    }
}

#[test]
fn abandoned_approval_history_only_claims_successful_allowances() {
    for outcome in ["success", "failed", "reverted", "cancelled"] {
        let mut host = MockHost::new()
            .with_enso_response(build_enso_response_erc20())
            .with_allowance("0");
        let id = crate::workflow::create(
            &mut host,
            "test-wallet",
            b"swap 100 usdc to eth on ethereum",
        )
        .unwrap();
        crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
        let approval_id = load_session(&mut host, &id).intent_states[0]
            .outbox_id
            .clone()
            .unwrap();
        host.inspections.insert(
            approval_id.clone(),
            OutboxInspection {
                outbox_id: approval_id,
                state: outcome.into(),
                tx_hash: None,
                receipt_json: None,
            },
        );
        crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap();
        let session = load_session(&mut host, &id);
        assert_eq!(session.state, "abandoned");
        assert_eq!(
            session
                .history
                .last()
                .unwrap()
                .reason
                .contains("new intent reuses it"),
            outcome == "success"
        );
    }
}

/// A stronger minimum already in Enso's route becomes the reviewed floor.
#[test]
fn a_stronger_calldata_minimum_becomes_the_reviewed_floor() {
    let mut host =
        MockHost::new().with_enso_response(build_enso_response_native_quoted_with_minimum(
            1_000,
            "3000000000",
            DEFAULT_ROUTER,
            serde_json::json!(["2990000000"]),
        ));
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 1.0 eth to usdc on ethereum",
    )
    .unwrap();
    assert_eq!(
        load_session(&mut host, &id)
            .route_request
            .unwrap()
            .min_amount_out,
        Some(alloy::primitives::U256::from(2_990_000_000u64))
    );
}

/// A staging failure after a passing simulation is what the session reports.
#[test]
fn a_staging_failure_replaces_the_previous_error() {
    let (mut host, id) = quoted_session(10_000);
    host.eth_call_success = false;
    assert!(crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").is_err());
    assert!(
        load_session(&mut host, &id)
            .last_error
            .unwrap()
            .contains("simulation failed")
    );
    host.eth_call_success = true;
    host.stage_fail_after = Some(0);
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    let sess = load_session(&mut host, &id);
    assert_eq!(sess.last_error.as_deref(), Some(error.as_str()));
    assert!(error.contains("staging the swap failed"), "{error}");
    assert!(sess.simulation.is_some());
}

/// A same-chain session from before output checks, with an unguarded route,
/// is refused at confirm rather than staged.
#[test]
fn an_unguarded_same_chain_route_is_never_staged() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to dai on ethereum",
    )
    .unwrap();
    let mut session = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let original: crate::api_types::RouteResponse =
        serde_json::from_slice(&build_enso_response_erc20()).unwrap();
    session.route.as_mut().unwrap().tx.data = original.tx.data.clone();
    session.intents.last_mut().unwrap().data_hex = format!("0x{}", hex::encode(&original.tx.data));
    host.put(
        &session.key(),
        &serde_json::to_vec(&session).unwrap(),
        false,
    )
    .unwrap();
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(error.contains("no on-chain output check"), "{error}");
    assert_eq!(host.stage_count, 0);
}

/// A cross-chain swap whose quote changed while the owner approved the token
/// allowance can be abandoned and started again, not left stuck.
#[test]
fn a_cross_chain_swap_refused_after_its_approval_can_be_abandoned() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&build_enso_response_erc20()).unwrap();
    response["route"][0]["destinationChainId"] = serde_json::json!(8453);
    response["amountOut"] = serde_json::json!("1000");
    let mut host = MockHost::new()
        .with_enso_response(serde_json::to_vec(&response).unwrap())
        .with_allowance("0");
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","destination_chain":"base"}"#,
    )
    .unwrap();
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let approval_id = sess.intent_states[0].outbox_id.clone().unwrap();

    // While the approval is still pending, abandoning says what to do first.
    let pending = crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap_err();
    assert!(pending.contains("still pending"), "{pending}");

    host.mark_outbox_success(&approval_id);
    host.allowance =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935".into();
    // The bridge quote changed during the approval, so the swap is refused.
    host.now += 120_000;
    response["amountOut"] = serde_json::json!("1001");
    response["tx"]["data"] =
        serde_json::json!(format!("{}00", response["tx"]["data"].as_str().unwrap()));
    host.enso_response = Some(serde_json::to_vec(&response).unwrap());
    assert!(crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").is_err());

    crate::workflow::abandon(&mut host, "test-wallet", &id).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(sess.state, "abandoned");
}

// ===========================================================================
// BUG HUNT: confirm() should reject an abandoned session
// ===========================================================================

#[test]
fn confirm_on_abandoned_session_should_fail() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.message().contains("does not match"),
        "should reject mismatched token: {err}"
    );
}

// ===========================================================================
// TEST: route verification rejects wrong amount
// ===========================================================================

#[test]
fn create_rejects_route_with_mismatched_amount() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_wrong_amount());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("does not match"));
}

// ===========================================================================
// TEST: route verification rejects native ETH with wrong value
// ===========================================================================

#[test]
fn create_rejects_native_route_with_wrong_value() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_native_wrong_value());

    let body = br#"swap 1.0 eth to usdc on ethereum"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("does not match"));
}

// ===========================================================================
// TEST: Enso API error propagates
// ===========================================================================

#[test]
fn enso_api_error_propagates() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    host.enso_status = 500;

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let result = crate::workflow::create(&mut host, "test-wallet", body);

    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("status 500"));
}

// ===========================================================================
// TEST: chain ID mismatch at confirm time is rejected
// ===========================================================================

#[test]
fn chain_id_mismatch_rejected_at_confirm() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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
    let body = br#"swap 100.0 usdc to dai on ethereum"#;
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
    let body = br#"swap 100.0 usdc to eth on ethereum"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();

    let status = crate::settlement::settlement_status(&mut host, &sess);
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("not_staged")
    );
}

// ===========================================================================
// TEST: settlement compares the balance increase with the route minimum
// ===========================================================================

#[test]
fn protected_settlement_is_not_downgraded_by_fees() {
    for output in ["dai", "eth"] {
        let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

        let body = format!("swap 100.0 usdc to {output} on ethereum");
        let id = crate::workflow::create(&mut host, "test-wallet", body.as_bytes()).unwrap();
        crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();

        let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
        let route_id = sess.intent_states[0].outbox_id.clone().unwrap();
        host.mark_outbox_success(&route_id);
        let before: u128 = sess.observed_before.as_deref().unwrap().parse().unwrap();
        let minimum = 49_750_000_000_000_000_u128;

        host.balance_override = Some((before + minimum).to_string());
        let status = crate::settlement::settlement_status(&mut host, &sess);
        assert_eq!(status["status"], "destination_minimum_enforced");
        assert_eq!(status["minimum_expected"], minimum.to_string());
        assert_eq!(status["minimum_source"], "router_calldata");
        assert!(status.get("retry_interval_ms").is_none());

        host.balance_override = Some((before + minimum - 1).to_string());
        let status = crate::settlement::settlement_status(&mut host, &sess);
        assert_eq!(status["status"], "destination_minimum_enforced");
        assert_eq!(status["delta"], (minimum - 1).to_string());
        assert!(status.get("retry_interval_ms").is_none());

        host.balance_override = Some((before - 1).to_string());
        let status = crate::settlement::settlement_status(&mut host, &sess);
        assert_eq!(status["status"], "destination_minimum_enforced");
        assert_eq!(status["observed_after"], (before - 1).to_string());

        let mut no_baseline = sess.clone();
        no_baseline.observed_before = None;
        host.balance_override = Some("invalid balance".into());
        let status = crate::settlement::settlement_status(&mut host, &no_baseline);
        assert_eq!(status["status"], "destination_minimum_enforced");
        assert!(status["observation_error"].is_string());
        assert!(status.get("delta").is_none());
    }
}

// ===========================================================================
// TEST: settlement does not inspect destination before source success
// ===========================================================================

#[test]
fn settlement_source_pending_before_receipt() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to dai on ethereum"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();

    let sess = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let status = crate::settlement::settlement_status(&mut host, &sess);

    // A staged entry is not a broadcast or a mined success.
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("source_pending")
    );
    assert_eq!(status["retry_interval_ms"], 10000);

    // A transient inspection failure is not terminal: keep polling.
    host.inspect_error = Some(petal::SdkError::Message(
        "backend: read EVM outbox receipt: busy".into(),
    ));
    let status = crate::settlement::settlement_status(&mut host, &sess);
    assert_eq!(status["status"], "source_pending");
    assert_eq!(status["retry_interval_ms"], 10000);
    assert!(status["error"].as_str().unwrap().contains("busy"));

    // A refused or missing entry never changes on retry: report a final error.
    for refusal in [
        petal::HostStatus::Denied,
        petal::HostStatus::NotFound,
        petal::HostStatus::Invalid,
    ] {
        host.inspect_error = Some(petal::SdkError::Host(refusal));
        let status = crate::settlement::settlement_status(&mut host, &sess);
        assert_eq!(status["status"], "error", "{status}");
        assert!(status.get("retry_interval_ms").is_none(), "{status}");
    }
}

// ===========================================================================
// TEST: partial staging failure leaves session in consistent state
// ===========================================================================

#[test]
fn partial_staging_failure_is_recoverable() {
    let mut host = MockHost::new()
        .with_enso_response(build_enso_response_erc20())
        .with_allowance("0"); // creates 2 intents: approve + route

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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
fn slippage_above_venue_configuration_is_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","slippage_bps":200}"#;
    let error = crate::workflow::create(&mut host, "test-wallet", body)
        .unwrap_err()
        .to_string();
    assert!(error.contains("max_slippage"), "{error}");
}

#[test]
fn omitted_mev_section_keeps_the_default_slippage_ceiling() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let config = String::from_utf8(
        host.store
            .remove("settings/test-wallet/venue.toml")
            .unwrap(),
    )
    .unwrap()
    .replace("[mev]\nmax_slippage_bps = 100\n\n", "");
    host.store.insert(
        "settings/wallets/test-wallet/venue.toml".into(),
        config.into_bytes(),
    );

    let body = br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","slippage_bps":200}"#;
    let error = crate::workflow::create(&mut host, "test-wallet", body)
        .unwrap_err()
        .to_string();
    assert!(error.contains("max_slippage"), "{error}");
}

#[test]
fn plan_shows_slippage_as_a_percentage() {
    for (bps, shown) in [(5, "5 bps (0.05%)"), (100, "100 bps (1.00%)")] {
        let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
        let body = format!(
            r#"{{"intent":"swap 100.0 usdc to eth","chain":"ethereum","slippage_bps":{bps}}}"#
        );
        let id = crate::workflow::create(&mut host, "test-wallet", body.as_bytes()).unwrap();
        let plan = load_session(&mut host, &id).plan_md;
        assert!(plan.contains(shown), "{plan}");
    }
}

// ===========================================================================
// TEST: default slippage is 50 bps when not specified
// ===========================================================================

#[test]
fn default_slippage_is_50_bps() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    let body = br#"swap 100.0 usdc to eth on ethereum"#;
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

    let id1 = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"swap 100.0 usdc to eth on ethereum"#,
    )
    .unwrap();
    // Same intent — IDs differ because random() generates unique session IDs.
    let id2 = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"swap 100.0 usdc to eth on ethereum"#,
    )
    .unwrap();

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
fn mixed_case_bloom_wallet_name_is_supported() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let venue = host
        .store
        .get("settings/test-wallet/venue.toml")
        .cloned()
        .unwrap();
    host.store
        .insert("settings/wallets/Alice/venue.toml".to_string(), venue);
    host.vfs.insert(
        "wallets/Alice/0/address.evm".to_string(),
        b"0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1".to_vec(),
    );

    let id = crate::workflow::create(&mut host, "Alice", b"swap 100.0 usdc to eth on ethereum")
        .expect("mixed-case Bloom wallet names must remain usable");
    let session = crate::workflow::load(&mut host, "Alice", &id).unwrap();
    assert_eq!(session.wallet, "Alice");
    assert!(
        host.store
            .contains_key(&format!("intents/Alice/{id}/session.json"))
    );
}

#[test]
fn invalid_wallet_name_rejected() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());

    // Wallet name with slash — path traversal attempt
    let result = crate::workflow::create(
        &mut host,
        "../etc",
        br#"swap 100.0 usdc to eth on ethereum"#,
    );
    assert!(result.is_err());

    // Empty wallet name
    let result = crate::workflow::create(&mut host, "", br#"swap 100.0 usdc to eth on ethereum"#);
    assert!(result.is_err());

    // Wallet name with space
    let result = crate::workflow::create(
        &mut host,
        "test wallet",
        br#"swap 100.0 usdc to eth on ethereum"#,
    );
    assert!(result.is_err());
}

#[test]
fn oversized_intent_body_is_rejected_before_external_work() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let body = vec![b'a'; crate::input::MAX_NEW_BODY_BYTES + 1];
    let error = crate::workflow::create(&mut host, "test-wallet", &body)
        .unwrap_err()
        .to_string();
    assert!(error.contains("intent body"), "{error}");
    assert_eq!(host.stage_count, 0);
}

#[test]
fn abandon_requires_exact_command() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to eth on ethereum",
    )
    .unwrap();
    let error =
        crate::workflow::abandon_with_body(&mut host, "test-wallet", &id, b"yes").unwrap_err();
    assert!(error.contains("exactly `abandon`"), "{error}");
    assert_eq!(
        crate::workflow::load(&mut host, "test-wallet", &id)
            .unwrap()
            .state,
        "prepared"
    );
}

#[test]
fn active_confirmation_lock_prevents_duplicate_staging() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to eth on ethereum",
    )
    .unwrap();
    let lock_key = format!("intents/test-wallet/{id}/confirm.lock");
    let lock = serde_json::to_vec(&serde_json::json!({
        "created_ms": host.now,
        "nonce": "held",
    }))
    .unwrap();
    host.put_new(&lock_key, &lock, false).unwrap();

    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(error.contains("already in progress"), "{error}");
    assert_eq!(host.stage_count, 0);
}

#[test]
fn stale_confirmation_lock_is_recovered() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to eth on ethereum",
    )
    .unwrap();
    let lock_key = format!("intents/test-wallet/{id}/confirm.lock");
    let stale = serde_json::to_vec(&serde_json::json!({
        "created_ms": 1,
        "nonce": "stale",
    }))
    .unwrap();
    host.put_new(&lock_key, &stale, false).unwrap();

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(host.stage_count, 1);
    assert!(!host.store.contains_key(&lock_key));
}

#[test]
fn confirm_rejects_tampered_prepared_transaction() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to eth on ethereum",
    )
    .unwrap();
    let mut session = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    session.intents.last_mut().unwrap().to = "0x000000000000000000000000000000000000dead".into();
    host.put(
        &session.key(),
        &serde_json::to_vec(&session).unwrap(),
        false,
    )
    .unwrap();

    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(error.contains("does not match"), "{error}");
    assert_eq!(host.stage_count, 0);
}

#[test]
fn create_rejects_mismatched_destination_metadata() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&build_enso_response_erc20()).unwrap();
    response["route"][0]["destinationChainId"] = serde_json::json!(137);
    let mut host = MockHost::new().with_enso_response(serde_json::to_vec(&response).unwrap());
    let body = br#"{
        "intent":"swap 100.0 usdc to eth",
        "chain":"ethereum",
        "destination_chain":"base"
    }"#;
    let error = crate::workflow::create(&mut host, "test-wallet", body)
        .unwrap_err()
        .to_string();
    assert!(error.contains("destination chain"), "{error}");
}

#[test]
fn confirm_accepts_legacy_cross_chain_destination_metadata() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&build_enso_response_erc20()).unwrap();
    response["route"][0]["destinationChainId"] = serde_json::json!(8453);
    let mut host = MockHost::new().with_enso_response(serde_json::to_vec(&response).unwrap());
    let body = br#"{
        "intent":"swap 100.0 usdc to eth",
        "chain":"ethereum",
        "destination_chain":"base"
    }"#;
    let id = crate::workflow::create(&mut host, "test-wallet", body).unwrap();
    let mut session = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    session.route.as_mut().unwrap().destination_chain_id = None;
    host.put(
        &session.key(),
        &serde_json::to_vec(&session).unwrap(),
        false,
    )
    .unwrap();

    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(host.stage_count, 1);
}

#[test]
fn unrelated_balance_increase_is_not_claimed_as_settlement() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100.0 usdc to dai on ethereum",
    )
    .unwrap();
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let mut session = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let route_id = session.intent_states[0].outbox_id.clone().unwrap();
    host.mark_outbox_success(&route_id);
    host.balance_override = Some("2000000000000000000".into());
    // Legacy unprotected calldata cannot prove the minimum was enforced.
    session.route.as_mut().unwrap().tx.data = vec![0].into();

    let status = crate::settlement::settlement_status(&mut host, &session);
    assert_eq!(
        status.get("status").and_then(|value| value.as_str()),
        Some("destination_observed_unattributed")
    );
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
    let result = crate::workflow::create(&mut host, "test-wallet", b"swap usdc to eth on ethereum");
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
        b"swap 100.0 nonexistenttoken to eth on ethereum",
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("could not resolve"));
}

#[test]
fn legacy_venue_restrictions_survive_until_explicit_override() {
    let mut host = MockHost::new();
    host.store.insert(
        "settings/test-wallet/venue.toml".into(),
        b"[defi]\nenabled = false\n".to_vec(),
    );
    assert!(
        !crate::policy::load_venue_config(&mut host, "test-wallet")
            .unwrap()
            .defi
            .enabled
    );
    crate::policy::write_venue_config(&mut host, "test-wallet", b"[defi]\nenabled = true\n")
        .unwrap();
    let policy = crate::policy::load_venue_config(&mut host, "test-wallet").unwrap();
    assert!(policy.defi.enabled);
    assert!(policy.defi.allowed_source_chains.is_empty());
    assert!(policy.defi.require_calldata_verification);
}

#[test]
fn default_venue_rejects_noncanonical_router() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    host.store.clear();
    let error = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100 usdc to eth on ethereum",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("router"), "{error}");
    assert_eq!(host.stage_count, 0);
}

#[test]
fn venue_disable_is_rechecked_before_staging() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 100 usdc to eth on ethereum",
    )
    .unwrap();
    crate::policy::write_venue_config(&mut host, "test-wallet", b"[defi]\nenabled = false\n")
        .unwrap();
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(error.contains("disabled"), "{error}");
    assert_eq!(host.stage_count, 0);
}

#[test]
fn malformed_saved_venue_configuration_does_not_use_enabled_defaults() {
    let mut host = MockHost::new();
    host.store.insert(
        "settings/wallets/test-wallet/venue.toml".into(),
        b"invalid toml".to_vec(),
    );
    assert!(crate::policy::load_venue_config(&mut host, "test-wallet").is_err());
}

#[test]
fn create_on_account_one_reads_only_its_numbered_address() {
    let mut host = MockHost::new().with_enso_response(build_enso_response_erc20());
    let address = host
        .vfs
        .remove("wallets/test-wallet/0/address.evm")
        .unwrap();
    host.vfs
        .insert("wallets/test-wallet/1/address.evm".into(), address);
    let id = crate::workflow::create_for_account(
        &mut host,
        "test-wallet",
        1,
        b"swap 100.0 usdc to eth on ethereum",
    )
    .expect("account one has the selected EVM address");
    let session = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    assert_eq!(session.wallet, "test-wallet");
}

// ===========================================================================
// TEST: quote freshness at staging
// ===========================================================================

const DEFAULT_ROUTER: &str = "0x1234567890abcdef1234567890abcdef12345678";

/// A native 1 ETH route whose Router V2 action bytes carry Enso's quote
/// metadata, as live Enso calldata does.
fn build_enso_response_native_quoted(
    quoted_at_secs: u64,
    amount_out: &str,
    router: &str,
) -> Vec<u8> {
    use alloy::primitives::U256;
    use alloy::sol_types::{SolCall, SolValue};

    let amount_in = U256::from(1_000_000_000_000_000_000u128);
    let token = crate::api_types::IEnsoRouter::Token {
        tokenType: 0,
        data: (amount_in,).abi_encode_params().into(),
    };
    let metadata = format!(
        r#"{{"Source":"Enso-test","AmountOut":"{amount_out}","Timestamp":{quoted_at_secs}}}"#
    );
    let route_data = crate::api_types::IEnsoRouter::routeSingleCall {
        tokenIn: token,
        data: metadata.into_bytes().into(),
    }
    .abi_encode();
    let body = serde_json::json!({
        "tx": {
            "to": router,
            "data": format!("0x{}", hex::encode(&route_data)),
            "value": "1000000000000000000",
            "from": "0x742d35cc6634c0532925a3b844bc9e7595f0beb1",
        },
        "amountOut": amount_out,
        "gas": "120000",
        "route": [{"protocol": "uniswap-v3"}],
        "priceImpact": 0.3
    });
    serde_json::to_vec(&body).unwrap()
}

/// [`build_enso_response_native_quoted`] with Enso's reported `minAmountOut`.
fn build_enso_response_native_quoted_with_minimum(
    quoted_at_secs: u64,
    amount_out: &str,
    router: &str,
    min_amount_out: serde_json::Value,
) -> Vec<u8> {
    let mut body: serde_json::Value = serde_json::from_slice(&build_enso_response_native_quoted(
        quoted_at_secs,
        amount_out,
        router,
    ))
    .unwrap();
    body["minAmountOut"] = min_amount_out;
    // Unlike quote metadata, this floor is enforced by Router V2 itself.
    use alloy::sol_types::{SolCall, SolValue};
    let route: crate::api_types::RouteResponse = serde_json::from_value(body.clone()).unwrap();
    let original =
        crate::api_types::IEnsoRouter::routeSingleCall::abi_decode(&route.tx.data).unwrap();
    let value = match &body["minAmountOut"] {
        serde_json::Value::Array(items) => &items[0],
        value => value,
    };
    let minimum = alloy::primitives::U256::from_str_radix(value.as_str().unwrap(), 10).unwrap();
    let guarded = crate::api_types::IEnsoRouter::safeRouteSingleCall {
        tokenIn: original.tokenIn,
        tokenOut: crate::api_types::IEnsoRouter::Token {
            tokenType: 1,
            data: (
                "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
                    .parse::<alloy::primitives::Address>()
                    .unwrap(),
                minimum,
            )
                .abi_encode_params()
                .into(),
        },
        receiver: route.tx.from,
        data: original.data,
    }
    .abi_encode();
    body["tx"]["data"] = serde_json::json!(format!("0x{}", hex::encode(guarded)));
    serde_json::to_vec(&body).unwrap()
}

/// The `Timestamp` the test builder embeds in each quote's calldata, used
/// only to tell which quote was staged. Production code never reads it.
fn quote_marker(calldata: &[u8]) -> Option<u64> {
    let marker = b"{\"Source\":\"Enso";
    let start = calldata.windows(marker.len()).position(|w| w == marker)?;
    let value: serde_json::Value = serde_json::Deserializer::from_slice(&calldata[start..])
        .into_iter()
        .next()?
        .ok()?;
    value.get("Timestamp")?.as_u64()
}

fn load_session(host: &mut MockHost, id: &str) -> crate::session::Session {
    let raw = host
        .get(&crate::session::key("test-wallet", id), 2 * 1024 * 1024)
        .unwrap()
        .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

/// Create a native swap quoted at t=1000s, then let `delay_ms` pass.
fn quoted_session(delay_ms: u64) -> (MockHost, String) {
    let mut host = MockHost::new().with_enso_response(build_enso_response_native_quoted(
        1_000,
        "3000000000",
        DEFAULT_ROUTER,
    ));
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        b"swap 1.0 eth to usdc on ethereum",
    )
    .expect("create should succeed");
    host.now += delay_ms;
    (host, id)
}

#[test]
fn fresh_quote_is_staged_without_requoting() {
    let (mut host, id) = quoted_session(10_000);
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(host.enso_calls, 1);
    let sess = load_session(&mut host, &id);
    assert_eq!(sess.state, "staged");
    assert_eq!(
        quote_marker(&hex::decode(host.staged_data[0].trim_start_matches("0x")).unwrap()),
        Some(1_000)
    );
}

#[test]
fn aged_quote_is_refreshed_before_staging_within_reviewed_bounds() {
    // Quoted at 1000s, staged two minutes later: a policy ceremony in between
    // would otherwise leave Bloom less than three minutes for the approval.
    let (mut host, id) = quoted_session(120_000);
    // A slightly lower fresh quote is still inside the 0.5% slippage the
    // owner reviewed (minimum 2,985,000,000), and Enso holds the staged
    // transaction to exactly that floor.
    host.enso_response = Some(build_enso_response_native_quoted_with_minimum(
        1_120,
        "2990000000",
        DEFAULT_ROUTER,
        serde_json::json!(["2985000000"]),
    ));
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(host.enso_calls, 2);
    // The refresh asked for the reviewed floor, not the reviewed slippage.
    let refresh_url = &host.enso_urls[1];
    assert!(
        refresh_url.contains("minAmountOut=2985000000"),
        "{refresh_url}"
    );
    assert!(!refresh_url.contains("slippage="), "{refresh_url}");
    assert!(
        host.enso_urls[0].contains("slippage=50"),
        "{}",
        host.enso_urls[0]
    );

    let sess = load_session(&mut host, &id);
    assert_eq!(sess.state, "staged");
    assert_eq!(sess.last_error, None);
    assert_eq!(sess.route.as_ref().unwrap().amount_out, "2990000000");
    assert!(
        sess.plan_md
            .contains("Expected output (current Enso quote): 2990000000")
    );
    assert!(sess.plan_md.contains("Calldata minimum output: 2985000000"));
    assert!(sess.plan_md.contains("This is the owner-reviewed floor"));
    assert!(
        sess.plan_md
            .contains("50 bps (0.50%), applied to the first reviewed quote")
    );
    assert!(
        !sess
            .plan_md
            .contains("Expected output (current Enso quote): 3000000000")
    );
    assert!(
        sess.history
            .iter()
            .any(|entry| entry.reason.contains("Enso quote refreshed before staging"))
    );
    assert_eq!(
        quote_marker(&hex::decode(host.staged_data[0].trim_start_matches("0x")).unwrap()),
        Some(1_120),
        "the staged transaction must carry the fresh quote"
    );
    // The refreshed quote is dated from when this Petal fetched it.
    let fresh = sess.route.as_ref().unwrap();
    let fetched = fresh.fetched_at_ms.expect("refresh records its fetch time");
    assert!(fetched > 1_120_000, "{fetched}");
    let view = crate::quote::status_view(fresh, fetched).unwrap();
    assert_eq!(view["expires_at_ms"], fetched + 300_000);
    assert_eq!(view["expires_from"], "fetch_time");
    assert_eq!(view["expired"], false);
}

#[test]
fn a_higher_refreshed_quote_does_not_raise_the_reviewed_floor() {
    // Reviewed at 3,000,000,000 less 0.5%: floor 2,985,000,000. The market
    // then rises, and Enso returns a safe route held to exactly that floor.
    // The first refresh is saved but not staged (simulation fails); the next
    // one compares against what the owner reviewed, not that replacement.
    let (mut host, id) = quoted_session(120_000);
    host.eth_call_success = false;
    host.enso_response = Some(build_enso_response_native_quoted_with_minimum(
        1_120,
        "3030000000",
        DEFAULT_ROUTER,
        serde_json::json!(["2985000000"]),
    ));
    assert!(crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").is_err());
    host.eth_call_success = true;
    host.now += 120_000;
    host.enso_response = Some(build_enso_response_native_quoted_with_minimum(
        1_240,
        "3000000000",
        DEFAULT_ROUTER,
        serde_json::json!(["2985000000"]),
    ));
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let sess = load_session(&mut host, &id);
    assert_eq!(sess.state, "staged");
    assert_eq!(
        sess.route_request.as_ref().unwrap().min_amount_out,
        Some(alloy::primitives::U256::from(2_985_000_000u64)),
        "a replacement quote must not become the new floor"
    );
}

#[test]
fn refreshed_quote_outside_reviewed_bounds_is_refused_and_reported() {
    for (fresh, expected) in [
        (
            build_enso_response_native_quoted(1_120, "2984999999", DEFAULT_ROUTER),
            "is below the reviewed minimum output",
        ),
        // The quote clears the floor, but the transaction's own minimum does
        // not: it could fill below what the owner reviewed.
        (
            build_enso_response_native_quoted_with_minimum(
                1_120,
                "2990000000",
                DEFAULT_ROUTER,
                serde_json::json!("2984999999"),
            ),
            "route's minimum output (2984999999)",
        ),
        (
            build_enso_response_native_quoted(
                1_120,
                "3000000000",
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            ),
            "instead of the reviewed",
        ),
    ] {
        let (mut host, id) = quoted_session(120_000);
        host.enso_response = Some(fresh);
        let error =
            crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(error.contains("create a new one"), "{error}");
        assert!(host.staged.is_empty(), "nothing may be staged");

        let sess = load_session(&mut host, &id);
        assert_eq!(sess.state, "prepared");
        assert_eq!(sess.last_error.as_deref(), Some(error.as_str()));
        assert_eq!(sess.route.as_ref().unwrap().amount_out, "3000000000");
    }
}

#[test]
fn temporary_refresh_failure_says_to_retry_not_abandon() {
    let (mut host, id) = quoted_session(120_000);
    host.enso_status = 503;
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(error.contains("write `confirm` again"), "{error}");
    assert!(!error.contains("abandon"), "{error}");
    assert!(host.staged.is_empty());
    let sess = load_session(&mut host, &id);
    assert_eq!(sess.state, "prepared");

    // Once Enso answers again, the same session stages.
    host.enso_status = 200;
    host.enso_response = Some(build_enso_response_native_quoted_with_minimum(
        1_120,
        "3000000000",
        DEFAULT_ROUTER,
        serde_json::json!("2985000000"),
    ));
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    assert_eq!(load_session(&mut host, &id).state, "staged");
}

#[test]
fn expired_staged_quote_error_names_the_outbox_entry_and_next_step() {
    let message = crate::quote::expired_staged_error("0002-63206");
    assert!(message.contains("0002-63206"));
    assert!(message.contains("pending/0002-63206/confirm") && message.contains("create a new one"));
}

// ===========================================================================
// TEST: create separates input mistakes from backend failures
// ===========================================================================

#[test]
fn create_reports_input_mistakes_as_invalid_input() {
    use crate::workflow::CreateError;

    for (body, expected) in [
        (&b"swap 1.0 eth to usdc"[..], "name the source chain"),
        (
            &br#"{"intent":"swap 1.0 eth to usdc"}"#[..],
            "name the source chain",
        ),
        (
            &b"swap usdc to eth on ethereum"[..],
            "could not parse intent",
        ),
        (
            &b"swap 1.0 notatoken to usdc on ethereum"[..],
            "could not resolve token symbol",
        ),
    ] {
        let mut host = MockHost::new().with_enso_response(build_enso_response_native());
        match crate::workflow::create(&mut host, "test-wallet", body) {
            Err(CreateError::InvalidInput(message)) => {
                assert!(message.contains(expected), "{message}");
            }
            other => panic!(
                "{:?} must be invalid input, got {other:?}",
                String::from_utf8_lossy(body)
            ),
        }
        assert_eq!(host.enso_calls, 0, "input mistakes must not reach Enso");
    }

    // A well-formed intent that fails downstream is a backend failure.
    let mut host = MockHost::new().with_enso_response(build_enso_response_native());
    host.eth_call_success = false;
    assert!(matches!(
        crate::workflow::create(&mut host, "test-wallet", b"swap 1.0 eth to usdc on ethereum"),
        Err(CreateError::Failed(message)) if message.contains("simulation failed")
    ));
}

// ===========================================================================
// TEST: routes declare the host capabilities their workflows use
// ===========================================================================

/// The mock host does not enforce capabilities, but Bloom does: a workflow
/// call the route never declared is denied at runtime (`confirm` refreshing
/// an aged quote once failed live with "denied"). Pin each declaration.
#[test]
fn routes_declare_capabilities_their_workflows_need() {
    let confirm = include_str!("../files/intents/[wallet]/[index]/[id]/confirm.rs");
    for cap in [
        "bloom:http",
        "bloom:store",
        "bloom:tx.outbox",
        "bloom:chain",
    ] {
        assert!(
            confirm.contains(&format!("\"{cap}\"")),
            "confirm must declare {cap}"
        );
    }
    let create = include_str!("../files/intents/[wallet]/[index]/new.rs");
    for cap in ["bloom:http", "bloom:store", "bloom:chain"] {
        assert!(
            create.contains(&format!("\"{cap}\"")),
            "new must declare {cap}"
        );
    }
    let status = include_str!("../files/intents/[wallet]/[index]/[id]/status.json.rs");
    for cap in ["bloom:store", "bloom:tx.outbox"] {
        assert!(
            status.contains(&format!("\"{cap}\"")),
            "status.json must declare {cap}"
        );
    }
    let abandon = include_str!("../files/intents/[wallet]/[index]/[id]/abandon.rs");
    for cap in ["bloom:store", "bloom:tx.outbox"] {
        assert!(
            abandon.contains(&format!("\"{cap}\"")),
            "abandon must declare {cap}"
        );
    }
}

#[test]
fn cross_chain_quote_refresh_requires_the_same_reviewed_shortcut() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&build_enso_response_erc20()).unwrap();
    response["route"][0]["destinationChainId"] = serde_json::json!(8453);
    let mut host = MockHost::new().with_enso_response(serde_json::to_vec(&response).unwrap());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","destination_chain":"base"}"#,
    )
    .unwrap();
    let session = crate::workflow::load(&mut host, "test-wallet", &id).unwrap();
    let reviewed = session.route.as_ref().unwrap();
    let request = session.route_request.as_ref().unwrap();
    assert!(crate::quote::verify_refreshed(reviewed, reviewed, request).is_ok());
    let mut changed = reviewed.clone();
    use alloy::sol_types::SolCall;
    let mut call =
        crate::api_types::IEnsoRouter::routeSingleCall::abi_decode(&changed.tx.data).unwrap();
    call.data = vec![1].into();
    changed.tx.data = call.abi_encode().into();
    assert!(
        crate::quote::verify_refreshed(reviewed, &changed, request)
            .unwrap_err()
            .contains("cross-chain shortcut changed")
    );
}

#[test]
fn cross_chain_floor_survives_repeated_refresh_and_reload_and_status_is_honest() {
    let mut response: serde_json::Value =
        serde_json::from_slice(&build_enso_response_erc20()).unwrap();
    response["route"][0]["destinationChainId"] = serde_json::json!(8453);
    response["amountOut"] = serde_json::json!("1000");
    let mut host = MockHost::new().with_enso_response(serde_json::to_vec(&response).unwrap());
    let id = crate::workflow::create(
        &mut host,
        "test-wallet",
        br#"{"intent":"swap 100.0 usdc to eth","chain":"ethereum","destination_chain":"base"}"#,
    )
    .unwrap();
    let original = load_session(&mut host, &id);
    assert_eq!(
        original
            .route_request
            .as_ref()
            .unwrap()
            .min_amount_out
            .unwrap()
            .to_string(),
        "995"
    );
    // Failed simulation keeps the route unstaged so confirm can refresh again.
    host.eth_call_success = false;
    response["amountOut"] = serde_json::json!("995");
    host.enso_response = Some(serde_json::to_vec(&response).unwrap());
    for _ in 0..2 {
        host.now += 120_000;
        assert!(crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").is_err());
        let reloaded = load_session(&mut host, &id);
        assert_eq!(
            reloaded
                .route_request
                .as_ref()
                .unwrap()
                .min_amount_out
                .unwrap()
                .to_string(),
            "995"
        );
        assert_eq!(reloaded.route.as_ref().unwrap().amount_out, "995");
        assert!(host.enso_urls.last().unwrap().contains("minAmountOut=995"));
    }
    response["amountOut"] = serde_json::json!("992");
    host.enso_response = Some(serde_json::to_vec(&response).unwrap());
    host.now += 120_000;
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(
        error.contains("minimum") || error.contains("floor"),
        "{error}"
    );
    host.eth_call_success = true;
    response["amountOut"] = serde_json::json!("995");
    host.enso_response = Some(serde_json::to_vec(&response).unwrap());
    crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap();
    let sess = load_session(&mut host, &id);
    host.mark_outbox_success(sess.primary_outbox_id().unwrap());
    let result = crate::settlement::settlement_status(&mut host, &sess);
    assert_eq!(result["status"], "destination_unverified");
    assert_eq!(result["minimum_expected"], "995");
    assert_eq!(result["retry_supported"], false);
    assert_eq!(host.bridge_calls, 0);
    assert_eq!(host.destination_calls, 0);
}

#[test]
fn legacy_session_refresh_requires_new_review() {
    let (mut host, id) = quoted_session(120_000);
    let mut sess = load_session(&mut host, &id);
    sess.schema_version = 1;
    sess.route_request.as_mut().unwrap().min_amount_out = None;
    host.put(&sess.key(), &serde_json::to_vec(&sess).unwrap(), false)
        .unwrap();
    let error = crate::workflow::confirm(&mut host, "test-wallet", &id, b"confirm").unwrap_err();
    assert!(error.contains("new review"), "{error}");
    assert_eq!(host.stage_count, 0);
}
