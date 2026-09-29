petal::route_file!(
    spec: petal::store_read_spec().caps(&["bloom:store", "bloom:tx.outbox"]),
    read: |ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let wallet = match crate::wallet::param(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::workflow::load(&mut host, wallet, id) {
            Ok(session) => {
                // Derive route_verified from policy_checks.
                let route_verified = session
                    .policy_checks
                    .as_array()
                    .and_then(|checks| {
                        checks.iter().find(|c| {
                            c.get("rule")
                                .and_then(|v| v.as_str())
                                .map(|s| s == "route_verified")
                                .unwrap_or(false)
                        })
                    })
                    .and_then(|c| c.get("outcome"))
                    .and_then(|v| v.as_str())
                    .map(|s| s == "pass")
                    .unwrap_or(false);

                // Get primary outbox info from the last intent state (the route tx).
                let primary = session
                    .intent_states
                    .iter()
                    .rev()
                    .find(|s| s.outbox_id.is_some());

                // Bloom refuses a staged route once its Enso quote expires, and
                // that refusal never reaches this Petal. Say so here instead of
                // leaving a staged intent that looks confirmable.
                let now = host.now_ms();
                let quote = session
                    .route
                    .as_ref()
                    .and_then(|route| crate::quote::status_view(route, now));
                let quote_expired = quote
                    .as_ref()
                    .and_then(|view| view.get("expired"))
                    .and_then(|expired| expired.as_bool())
                    .unwrap_or(false);
                let last_error = session.last_error.clone().or_else(|| {
                    let outbox_id = primary
                        .filter(|state| {
                            session.state == "staged" && quote_expired && state.tx_hash.is_none()
                        })
                        .and_then(|state| state.outbox_id.clone())?;
                    // Only while the outbox has not moved it past pending.
                    let unsent = host
                        .tx_inspect(&session.wallet, &session.chain, &outbox_id)
                        .map_or(true, |inspection| inspection.state == "pending");
                    unsent.then(|| crate::quote::expired_staged_error(&outbox_id))
                });

                petal::read_json_value(&serde_json::json!({
                    "id": session.id,
                    "wallet": session.wallet,
                    "state": session.state,
                    "updated_ms": session.updated_ms,
                    "route_verified": route_verified,
                    "intent_count": session.intents.len(),
                    "intent_states": session.intent_states,
                    "staged_ids": session.staged_ids,
                    "primary_outbox_id": primary.and_then(|s| s.outbox_id.clone()),
                    "primary_tx_hash": primary.and_then(|s| s.tx_hash.clone()),
                    "policy_checks": session.policy_checks,
                    "quote": quote,
                    "last_error": last_error,
                    "history": session.history,
                }))
            }
            Err(error) => match host.get(
                &crate::session::failure_key(wallet, id),
                64 * 1024,
            ) {
                Ok(Some(raw)) => petal::DispatchResponse::Read(raw),
                _ => petal::error(-1, error),
            },
        }
    }
);
