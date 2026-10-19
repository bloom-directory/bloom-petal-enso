//! Enso's bridge-check API reports transport and destination execution
//! separately. Source/destination correlation is provider-asserted; a provider
//! response alone is never a receipt or proof of a received token amount.
//! Schema: https://api.enso.build/api-json, retrieved 2026-10-19.

use petal::sdk::HttpRequest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::runtime::Host;

const PROVIDERS: [&str; 5] = ["layerzero", "ccip", "relay", "arbitrum-native", "cctp"];
const CACHE_MS: u64 = 10_000;

pub fn valid_hash(value: &str) -> bool {
    value.len() == 66
        && value.starts_with("0x")
        && value[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Observation {
    pub provider: String,
    pub status: String,
    pub destination_chain_id: Option<u64>,
    pub delivery_tx: Option<String>,
    pub execution_tx: Option<String>,
    pub intent_phase: Option<String>,
    pub refund_tx: Option<String>,
    pub multi_bridge: bool,
    pub hops_linked: bool,
}

#[derive(Deserialize, Serialize)]
struct Cache {
    fetched_ms: u64,
    source_chain: u64,
    source_tx: String,
    observations: Vec<Observation>,
}

fn hash_at(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)?
        .as_str()
        .filter(|s| valid_hash(s))
        .map(str::to_owned)
}

fn linked_hops(value: &Value, source_chain: u64, source_tx: &str) -> bool {
    let Some(hops) = value.get("hops").and_then(Value::as_array) else {
        return false;
    };
    if !(2..=8).contains(&hops.len()) {
        return false;
    }
    let mut chain = source_chain;
    let mut transaction = source_tx.to_owned();
    for hop in hops {
        if hop.get("sourceChainId").and_then(Value::as_u64) != Some(chain)
            || !hop
                .get("sourceTxHash")
                .and_then(Value::as_str)
                .is_some_and(|hash| hash.eq_ignore_ascii_case(&transaction))
            || hop.get("status").and_then(Value::as_str) != Some("delivered")
        {
            return false;
        }
        let Some(destination) = hop.get("destinationChainId").and_then(Value::as_u64) else {
            return false;
        };
        let Some(hash) = hash_at(hop, "destinationTxHash") else {
            return false;
        };
        chain = destination;
        transaction = hash;
    }
    value.get("destinationChainId").and_then(Value::as_u64) == Some(chain)
        && hash_at(value, "destinationTxHash")
            .is_some_and(|hash| hash.eq_ignore_ascii_case(&transaction))
}

fn project(
    provider: &str,
    value: &Value,
    source_chain: u64,
    source_tx: &str,
) -> Option<Observation> {
    if value.get("sourceChainId")?.as_u64()? != source_chain
        || !value
            .get("sourceTxHash")?
            .as_str()?
            .eq_ignore_ascii_case(source_tx)
        || value
            .get("error")
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        return None;
    }
    let status = value.get("status")?.as_str()?;
    if ![
        "pending",
        "inflight",
        "delivered",
        "failed",
        "ready_for_manual_execution",
        "unknown",
    ]
    .contains(&status)
    {
        return None;
    }
    let intent = value.get("intent").filter(|v| !v.is_null());
    if intent
        .and_then(|v| v.get("executionChainId"))
        .and_then(Value::as_u64)
        .is_some_and(|chain| Some(chain) != value.get("destinationChainId").and_then(Value::as_u64))
    {
        return None;
    }
    let intent_phase = intent.map(|v| {
        v.get("phase")
            .and_then(Value::as_str)
            .filter(|s| {
                [
                    "awaiting_funds",
                    "executing",
                    "executed",
                    "refunding",
                    "refunded",
                    "failed",
                    "unknown",
                ]
                .contains(s)
            })
            .unwrap_or("unknown")
            .to_owned()
    });
    let execution_tx = if intent_phase.as_deref() == Some("executed") {
        let outer = hash_at(value, "destinationExecutionTxHash");
        let nested = intent.and_then(|v| hash_at(v, "executionTxHash"));
        if outer
            .as_ref()
            .zip(nested.as_ref())
            .is_some_and(|(a, b)| !a.eq_ignore_ascii_case(b))
        {
            return None;
        }
        outer.or(nested)
    } else if intent.is_none() && status == "delivered" {
        hash_at(value, "destinationTxHash")
    } else {
        None
    };
    Some(Observation {
        provider: provider.into(),
        status: status.into(),
        destination_chain_id: value.get("destinationChainId").and_then(Value::as_u64),
        delivery_tx: hash_at(value, "destinationTxHash"),
        execution_tx,
        intent_phase,
        refund_tx: hash_at(value, "refundTxHash")
            .or_else(|| intent.and_then(|v| hash_at(v, "refundTxHash"))),
        multi_bridge: value
            .get("isMultiBridge")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        hops_linked: linked_hops(value, source_chain, source_tx),
    })
}

/// Bounded, cached GETs to the five documented bridges (Stargate aliases
/// LayerZero). No provider-supplied URL or error body reaches the caller.
pub fn observe<H: Host>(
    host: &mut H,
    source_chain: u64,
    source_tx: &str,
) -> Result<Vec<Observation>, String> {
    if !valid_hash(source_tx) {
        return Err("source transaction hash is invalid".into());
    }
    let key = format!(
        "bridge-status/{source_chain}/{}",
        source_tx.to_ascii_lowercase()
    );
    let now = host.now_ms();
    if let Some(bytes) = host.get(&key, 16 * 1024)?
        && let Ok(cache) = serde_json::from_slice::<Cache>(&bytes)
        && cache.source_chain == source_chain
        && cache.source_tx.eq_ignore_ascii_case(source_tx)
        && cache.observations.len() <= PROVIDERS.len()
        && cache.observations.iter().all(|o| {
            PROVIDERS.contains(&o.provider.as_str())
                && [&o.delivery_tx, &o.execution_tx, &o.refund_tx]
                    .iter()
                    .all(|hash| hash.as_ref().is_none_or(|hash| valid_hash(hash)))
        })
        && now
            .checked_sub(cache.fetched_ms)
            .is_some_and(|age| age < CACHE_MS)
    {
        return Ok(cache.observations);
    }
    let private = host.get_secret(crate::settings::API_KEY, 8192)?;
    let runtime = host.setting("enso-api-key")?;
    let credential = crate::settings::configured_api_key(private.as_deref(), runtime.as_deref())?;
    let mut observations = Vec::new();
    for provider in PROVIDERS {
        let response = host.http(HttpRequest {
            method: "GET".into(),
            url: format!("https://api.enso.finance/api/v1/{provider}/bridge/check?chainId={source_chain}&txHash={source_tx}"),
            headers: vec![("Authorization".into(), format!("Bearer {}", credential.key.expose()))], body: Vec::new(),
        }, 512 * 1024);
        let Ok(response) = response else {
            continue;
        };
        if response.status != 200 {
            continue;
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&response.body)
            && let Some(observation) = project(provider, &value, source_chain, source_tx)
        {
            observations.push(observation);
        }
    }
    host.put(
        &key,
        &serde_json::to_vec(&Cache {
            fetched_ms: now,
            source_chain,
            source_tx: source_tx.into(),
            observations: observations.clone(),
        })
        .map_err(|_| "cannot encode bridge status")?,
        false,
    )?;
    Ok(observations)
}

/// Verify the destination network, receipt hash/status and the exact token's
/// attributable Transfer logs. Bridge delivery and intent execution use
/// different hashes. Multi-hop linkage remains explicit instead of guessed.
pub struct Destination<'a> {
    pub chain: &'a str,
    pub chain_id: u64,
    pub receiver: &'a str,
    pub token: &'a str,
    pub minimum: &'a str,
}

pub fn settlement<H: Host>(
    host: &mut H,
    source_chain: u64,
    source_tx: &str,
    destination: Destination<'_>,
) -> Value {
    let Destination {
        chain: destination_chain,
        chain_id: destination_chain_id,
        receiver,
        token,
        minimum,
    } = destination;
    let observations = match observe(host, source_chain, source_tx) {
        Ok(observations) => observations,
        Err(_) => return json!({"status":"bridge_status_unavailable", "source_tx_hash":source_tx}),
    };
    let mut result = json!({"status":"bridge_pending", "source_tx_hash":source_tx,
        "linkage_assurance":"provider_asserted", "bridges":observations});
    for observation in &observations {
        if observation.destination_chain_id != Some(destination_chain_id) {
            continue;
        }
        if observation.multi_bridge && !observation.hops_linked {
            result["status"] = json!("multi_bridge_unverified");
            continue;
        }
        if observation.refund_tx.is_some()
            || observation.intent_phase.as_deref() == Some("refunded")
        {
            result["status"] = json!("bridge_refund_reported");
            continue;
        }
        let Some(hash) = observation.execution_tx.as_deref() else {
            if observation.status == "ready_for_manual_execution" {
                result["status"] = json!("bridge_manual_execution_required");
            } else if observation.status == "failed"
                || observation.intent_phase.as_deref() == Some("failed")
            {
                result["status"] = json!("destination_execution_failed");
            } else if observation.status == "delivered" {
                result["status"] = json!("bridge_delivered_execution_pending");
            }
            continue;
        };
        match host.chain_id(destination_chain) {
            Ok(id) if id == destination_chain_id => {}
            _ => {
                result["status"] = json!("destination_network_mismatch");
                continue;
            }
        }
        let receipt = host
            .chain_read(
                destination_chain,
                "eth_getTransactionReceipt",
                &json!([hash]).to_string(),
            )
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let Some(receipt) = receipt.filter(|v| {
            v.get("transactionHash")
                .and_then(Value::as_str)
                .is_some_and(|actual| actual.eq_ignore_ascii_case(hash))
        }) else {
            result["status"] = json!("destination_receipt_pending");
            continue;
        };
        if receipt.get("status").and_then(Value::as_str) != Some("0x1") {
            result["status"] = json!(
                if receipt.get("status").and_then(Value::as_str) == Some("0x0") {
                    "destination_execution_failed"
                } else {
                    "destination_receipt_unverified"
                }
            );
            continue;
        }
        result["destination_execution_tx_hash"] = json!(hash);
        if token.eq_ignore_ascii_case(crate::api_types::NATIVE_TOKEN) {
            // A direct value transfer is attributable from the mined transaction.
            // Internal callbacks need trace evidence and remain unverified.
            let transaction = host
                .chain_read(
                    destination_chain,
                    "eth_getTransactionByHash",
                    &json!([hash]).to_string(),
                )
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if let Some(transaction) = transaction.filter(|v| {
                v.get("hash")
                    .and_then(Value::as_str)
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(hash))
                    && v.get("to")
                        .and_then(Value::as_str)
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(receiver))
                    && v.get("from")
                        .and_then(Value::as_str)
                        .is_some_and(|sender| !sender.eq_ignore_ascii_case(receiver))
                    && v.get("blockHash") == receipt.get("blockHash")
                    && receipt
                        .get("blockHash")
                        .and_then(Value::as_str)
                        .is_some_and(valid_hash)
            }) && let Some(amount) = transaction
                .get("value")
                .and_then(Value::as_str)
                .and_then(|value| value.strip_prefix("0x"))
                .and_then(|value| alloy::primitives::U256::from_str_radix(value, 16).ok())
            {
                result["receipt_transfer_amount"] = json!(amount.to_string());
                result["minimum_expected"] = json!(minimum);
                let code = host.chain_read(destination_chain, "eth_getCode", &json!([receiver, {"blockHash":receipt["blockHash"],"requireCanonical":true}]).to_string())
                    .ok().and_then(|text| serde_json::from_str::<String>(&text).ok());
                if !amount.is_zero()
                    && !crate::settlement::lt_decimal(&amount.to_string(), minimum)
                    && code.as_deref() == Some("0x")
                {
                    result["status"] = json!("destination_received_provider_linked");
                    return result;
                }
            }
            result["status"] = json!("destination_native_amount_unverified");
            continue;
        }
        let amount =
            crate::settlement::transfer_amount_to_receiver(&receipt.to_string(), token, receiver);
        result["receipt_transfer_amount"] = json!(amount);
        result["minimum_expected"] = json!(minimum);
        if amount
            .as_deref()
            .is_some_and(|amount| !crate::settlement::lt_decimal(amount, minimum))
        {
            result["status"] = json!("destination_received_provider_linked");
            return result;
        }
        result["status"] = json!("destination_below_minimum");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi_hop_linkage_requires_every_delivered_chain_and_transaction() {
        let source = format!("0x{}", "11".repeat(32));
        let middle = format!("0x{}", "22".repeat(32));
        let end = format!("0x{}", "33".repeat(32));
        let mut value = json!({"sourceChainId":1,"sourceTxHash":source,"destinationChainId":8453,"destinationTxHash":end,"status":"delivered","isMultiBridge":true,"hops":[
            {"sourceChainId":1,"sourceTxHash":source,"destinationChainId":42161,"destinationTxHash":middle,"status":"delivered"},
            {"sourceChainId":42161,"sourceTxHash":middle,"destinationChainId":8453,"destinationTxHash":end,"status":"delivered"}]});
        assert!(
            project("layerzero", &value, 1, &source)
                .unwrap()
                .hops_linked
        );
        value["hops"][1]["sourceTxHash"] = json!(source);
        assert!(
            !project("layerzero", &value, 1, &source)
                .unwrap()
                .hops_linked
        );
        value["hops"][1]["sourceTxHash"] = json!(middle);
        value["hops"][0]["status"] = json!("inflight");
        assert!(
            !project("layerzero", &value, 1, &source)
                .unwrap()
                .hops_linked
        );
        value["intent"] = json!({"phase":"executed","executionChainId":10});
        assert!(project("layerzero", &value, 1, &source).is_none());
    }
    #[test]
    fn intent_delivery_never_becomes_execution_and_hash_mismatches_refuse() {
        let source = format!("0x{}", "11".repeat(32));
        let delivery = format!("0x{}", "22".repeat(32));
        let execution = format!("0x{}", "33".repeat(32));
        let mut response = json!({"sourceChainId":1,"sourceTxHash":source,"destinationChainId":8453,"destinationTxHash":delivery,"status":"delivered","intent":{"phase":"awaiting_funds"}});
        assert!(
            project("relay", &response, 1, &source)
                .unwrap()
                .execution_tx
                .is_none()
        );
        response["intent"]["phase"] = json!("executed");
        response["destinationExecutionTxHash"] = json!(execution);
        assert_eq!(
            project("relay", &response, 1, &source)
                .unwrap()
                .execution_tx
                .as_deref(),
            Some(execution.as_str())
        );
        response["intent"]["executionTxHash"] = json!(delivery);
        assert!(project("relay", &response, 1, &source).is_none());
        response["sourceChainId"] = json!(56);
        assert!(project("relay", &response, 1, &source).is_none());
    }
}
