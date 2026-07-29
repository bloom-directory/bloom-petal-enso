petal::route_file!(
    spec: petal::store_read_spec().caps(&["bloom:store", "bloom:tx.outbox"]),
    read: |ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let wallet = match petal::param(ctx, "wallet") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        let session = match crate::workflow::load(&mut host, wallet, id) {
            Ok(value) => value,
            Err(error) => return petal::error(-1, error),
        };

        if session.staged_ids.is_empty() {
            return petal::error(-1, "no transactions staged");
        }

        // Inspect outbox for each staged intent.
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for state in &session.intent_states {
            if let Some(ref oid) = state.outbox_id {
                let label = session
                    .intents
                    .get(state.index)
                    .map(|i| i.label.as_str())
                    .unwrap_or("?");
                match host.tx_inspect(&session.wallet, &session.chain, oid) {
                    Ok(insp) => {
                        entries.push(serde_json::json!({
                            "intent_index": state.index,
                            "label": label,
                            "outbox_id": oid,
                            "state": insp.state,
                            "tx_hash": insp.tx_hash,
                            "receipt": insp.receipt_json.as_deref()
                                .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()),
                            "approval": state.approval,
                        }));
                    }
                    Err(_) => {
                        entries.push(serde_json::json!({
                            "intent_index": state.index,
                            "label": label,
                            "outbox_id": oid,
                            "state": "unknown",
                        }));
                    }
                }
            }
        }

        petal::read_json_value(&serde_json::json!({
            "session": session.id,
            "wallet": session.wallet,
            "chain": session.chain,
            "session_state": session.state,
            "primary_outbox_id": session.primary_outbox_id(),
            "primary_tx_hash": session.primary_tx_hash(),
            "entries": entries,
        }))
    }
);
