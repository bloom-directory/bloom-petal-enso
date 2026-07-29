use crate::api_types::{RouteRequest, RouteResponse};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    pub at_ms: u64,
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailedSession {
    pub schema_version: u32,
    pub id: String,
    pub wallet: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub state: String,
    pub last_error: String,
}

pub fn failure_key(wallet: &str, id: &str) -> String {
    format!("intents/{wallet}/{id}/failure.json")
}

/// One EVM transaction to stage into the outbox. A session typically holds two:
/// an `approve` intent (optional) and the `route` intent carrying the Enso
/// calldata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedIntent {
    /// `"approve"` or `"route"`.
    pub label: String,
    pub to: String,
    pub value_wei: String,
    pub data_hex: String,
    pub chain: String,
    /// Token address for `approve` intents.
    #[serde(default)]
    pub approve_token: Option<String>,
    /// Spender address for `approve` intents.
    #[serde(default)]
    pub approve_spender: Option<String>,
}

/// Per-intent staging state, kept in lockstep with `Session::intents` by index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntentState {
    pub index: usize,
    /// `"prepared"`, `"staged"`, `"broadcast"`, `"mined"`, `"failed"`.
    pub status: String,
    #[serde(default)]
    pub outbox_id: Option<String>,
    #[serde(default)]
    pub tx_hash: Option<String>,
    pub updated_ms: u64,
}

/// Durable intent session: the original intent text, the resolved Enso route
/// request/response, the prepared multi-intent plan, per-intent staging state,
/// simulation results, and settlement observations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub id: String,
    pub wallet: String,
    pub wallet_address: String,
    pub chain: String,
    #[serde(default)]
    pub destination_chain: Option<String>,
    pub intent_text: String,
    /// Persisted route request so the route can be re-verified on confirm.
    #[serde(default)]
    pub route_request: Option<RouteRequest>,
    #[serde(default)]
    pub route: Option<RouteResponse>,
    pub plan_md: String,
    /// Multi-intent plan (approve + route).
    #[serde(default)]
    pub intents: Vec<PreparedIntent>,
    /// Per-intent staging state, indexed in parallel with `intents`.
    #[serde(default)]
    pub intent_states: Vec<IntentState>,
    /// Outbox IDs of staged intents.
    #[serde(default)]
    pub staged_ids: Vec<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// Top-level lifecycle state — mirrors the most advanced `intent_state`
    /// status. Values: `"prepared"`, `"staged"`, `"confirmed"`, `"failed"`.
    pub state: String,
    /// Settlement baseline observed before staging.
    #[serde(default)]
    pub observed_before: Option<String>,
    #[serde(default)]
    pub min_settlement_delta: Option<String>,
    #[serde(default)]
    pub source_tx_hashes: Vec<String>,
    #[serde(default)]
    pub policy_checks: serde_json::Value,
    #[serde(default)]
    pub receiver_class: Option<String>,
    // petal additions
    /// Simulation result from the Enso Quoter (if run).
    #[serde(default)]
    pub simulation: Option<serde_json::Value>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub history: Vec<History>,
}

impl Session {
    pub fn transition(&mut self, now: u64, next: &str, reason: &str) {
        let from = std::mem::replace(&mut self.state, next.into());
        self.updated_ms = now;
        self.history.push(History {
            at_ms: now,
            from,
            to: next.into(),
            reason: reason.chars().take(256).collect(),
        });
        if self.history.len() > 100 {
            self.history.remove(0);
        }
    }

    pub fn key(&self) -> String {
        format!("intents/{}/{}/session.json", self.wallet, self.id)
    }

    pub fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "settled_success" | "settled_failed" | "abandoned"
        )
    }
}

pub fn key(wallet: &str, id: &str) -> String {
    format!("intents/{wallet}/{id}/session.json")
}
