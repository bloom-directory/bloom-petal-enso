use crate::api_types::{RouteRequest, RouteResponse};
use petal::Ctx;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    pub at_ms: u64,
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// The account that owns an intent session. The flat `/petals/enso/...`
/// mount and account 0 deliberately share the legacy `intents/<wallet>/`
/// store tree. Numbered accounts use separate trees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionOwner {
    wallet: String,
    account: u32,
}

impl SessionOwner {
    pub fn scope(ctx: &Ctx, wallet: &str) -> Result<Self, String> {
        Self::from_params(
            wallet,
            petal::route_param(ctx, "bloom.wallet"),
            petal::route_param(ctx, "bloom.account"),
        )
    }

    pub fn from_params(
        wallet: &str,
        mounted_wallet: Option<&str>,
        account: Option<&str>,
    ) -> Result<Self, String> {
        petal::validate_wallet_id(wallet)?;
        if let Some(mounted) = mounted_wallet
            && mounted != wallet
        {
            return Err(format!(
                "intent wallet {wallet:?} is not the mounted wallet {mounted:?}"
            ));
        }
        let account = match account {
            None => 0,
            Some(raw) => raw
                .parse::<u32>()
                .map_err(|error| format!("bloom.account must be a u32: {error}"))?,
        };
        Ok(Self {
            wallet: wallet.to_owned(),
            account,
        })
    }

    pub fn wallet(&self) -> &str {
        &self.wallet
    }

    pub fn account(&self) -> u32 {
        self.account
    }

    pub fn store_prefix(&self) -> String {
        if self.account == 0 {
            format!("intents/{}/", self.wallet)
        } else {
            format!("account-intents/{}/{}/", self.account, self.wallet)
        }
    }

    pub fn vfs_account_root(&self) -> String {
        format!("wallets/{}/{}", self.wallet, self.account)
    }

    pub fn key(&self, id: &str, file: &str) -> String {
        format!("{}{id}/{file}", self.store_prefix())
    }

    pub fn latest_key(&self) -> String {
        format!("{}latest", self.store_prefix())
    }
}

pub fn failure_key(owner: &SessionOwner, id: &str) -> String {
    owner.key(id, "failure.json")
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
    /// Outbox ID of a prerequisite intent that must be mined before this one
    /// is broadcast. Used for ERC-20 approve → route ordering.
    #[serde(default)]
    pub depends_on: Option<String>,
    /// Owner-safe approval data returned by `tx_stage` (`action_id` and
    /// `expires_ms`). Present when the staged transaction requires ceremony
    /// approval before broadcast; the owner-only ceremony URL is never exposed
    /// through a Petal route.
    #[serde(default)]
    pub approval: Option<serde_json::Value>,
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
    #[serde(default)]
    pub account: u32,
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
    /// Top-level lifecycle state. Approval-dependent routes pause in
    /// `"awaiting_approval"` and are never staged until the prerequisite has
    /// a successful receipt.
    pub state: String,
    /// Settlement baseline observed before staging.
    #[serde(default)]
    pub observed_before: Option<String>,
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
            self.history.drain(..1);
        }
    }

    pub fn key(&self) -> String {
        if self.account == 0 {
            format!("intents/{}/{}/session.json", self.wallet, self.id)
        } else {
            format!(
                "account-intents/{}/{}/{}/session.json",
                self.account, self.wallet, self.id
            )
        }
    }

    pub fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "settled_success" | "settled_failed" | "approval_failed" | "abandoned"
        )
    }

    /// The primary outbox ID — the last staged intent (the route tx).
    pub fn primary_outbox_id(&self) -> Option<&str> {
        self.intent_states
            .iter()
            .rev()
            .find_map(|s| s.outbox_id.as_deref())
    }

    /// The primary tx hash — the last staged intent's hash.
    pub fn primary_tx_hash(&self) -> Option<&str> {
        self.intent_states
            .iter()
            .rev()
            .find_map(|s| s.tx_hash.as_deref())
    }

    /// Aggregate policy outcome: `"deny"` if any check denies, `"warn"` if any
    /// warns (and none deny), `"pass"` otherwise.
    pub fn policy_overall(&self) -> &'static str {
        let mut overall = "pass";
        for check in self.policy_checks.as_array().into_iter().flatten() {
            match check.get("outcome").and_then(|v| v.as_str()) {
                Some("deny") => return "deny",
                Some("warn") if overall != "deny" => overall = "warn",
                _ => {}
            }
        }
        overall
    }
}

pub fn key(wallet: &str, id: &str) -> String {
    format!("intents/{wallet}/{id}/session.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_mount_and_account_zero_share_legacy_state() {
        let flat = SessionOwner::from_params("wallet", None, None).unwrap();
        let zero = SessionOwner::from_params("wallet", Some("wallet"), Some("0")).unwrap();
        let one = SessionOwner::from_params("wallet", Some("wallet"), Some("1")).unwrap();

        assert_eq!(flat, zero);
        assert_eq!(flat.store_prefix(), "intents/wallet/");
        assert_eq!(one.store_prefix(), "account-intents/1/wallet/");
        assert_eq!(one.vfs_account_root(), "wallets/wallet/1");
    }

    #[test]
    fn mounted_scope_rejects_foreign_wallets_and_bad_account_numbers() {
        assert!(SessionOwner::from_params("other", Some("wallet"), Some("1")).is_err());
        assert!(SessionOwner::from_params("wallet", Some("wallet"), Some("-1")).is_err());
        assert!(SessionOwner::from_params("wallet", Some("wallet"), Some("01x")).is_err());
    }

    #[test]
    fn account_state_keys_do_not_overlap() {
        let zero = SessionOwner::from_params("wallet", Some("wallet"), Some("0")).unwrap();
        let one = SessionOwner::from_params("wallet", Some("wallet"), Some("1")).unwrap();
        assert_eq!(
            zero.key("0123456789abcdef", "session.json"),
            "intents/wallet/0123456789abcdef/session.json"
        );
        assert_eq!(
            one.key("0123456789abcdef", "session.json"),
            "account-intents/1/wallet/0123456789abcdef/session.json"
        );
    }
}
