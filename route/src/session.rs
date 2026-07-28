use crate::api_types::RouteResponse;
use crate::input::NewIntentBody;
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

/// Durable intent session: the original intent text, the Enso route response,
/// the prepared transaction, simulation results, and settlement observations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub id: String,
    pub wallet: String,
    pub wallet_address: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub state: String,
    pub intent_text: String,
    pub chain: String,
    pub destination_chain: Option<String>,
    #[serde(default)]
    pub request_body: NewIntentBody,
    #[serde(default)]
    pub route: Option<RouteResponse>,
    /// Whether the route's input token/amount was verified against the request.
    #[serde(default)]
    pub route_verified: bool,
    /// Simulation result from the Enso Quoter (if run).
    #[serde(default)]
    pub simulation: Option<serde_json::Value>,
    /// Prepared EVM transaction for the outbox.
    #[serde(default)]
    pub prepared_tx: Option<PreparedTx>,
    #[serde(default)]
    pub outbox_id: Option<String>,
    #[serde(default)]
    pub outbox_state: Option<String>,
    #[serde(default)]
    pub tx_hash: Option<String>,
    #[serde(default)]
    pub plan_md: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub history: Vec<History>,
}

/// Prepared EVM transaction — what gets staged into the outbox on confirm.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedTx {
    pub to: String,
    pub value_wei: String,
    pub data_hex: String,
    pub chain: String,
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
