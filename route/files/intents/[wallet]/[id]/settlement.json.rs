petal::route_file!(
    spec: petal::store_read_spec().caps(&["bloom:store", "bloom:tx.outbox", "bloom:chain"]),
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

        // Get balance-based settlement status.
        let settlement = crate::settlement::settlement_status(&mut host, &session);

        // Inspect outbox for each staged intent.
        let mut outbox_states: Vec<serde_json::Value> = Vec::new();
        for state in &session.intent_states {
            if let Some(ref oid) = state.outbox_id {
                match host.tx_inspect(&session.wallet, &session.chain, oid) {
                    Ok(insp) => {
                        outbox_states.push(serde_json::json!({
                            "intent_index": state.index,
                            "label": session.intents.get(state.index).map(|i| i.label.as_str()).unwrap_or("?"),
                            "outbox_id": oid,
                            "state": insp.state,
                            "tx_hash": insp.tx_hash,
                            "receipt": insp.receipt_json.as_deref().and_then(|r| serde_json::from_str(r).ok()),
                        }));
                    }
                    Err(_) => {}
                }
            }
        }

        petal::read_json_value(&serde_json::json!({
            "session": session.id,
            "wallet": session.wallet,
            "chain": session.chain,
            "state": session.state,
            "settlement": settlement,
            "outbox": outbox_states,
            "staged_ids": session.staged_ids,
            "source_tx_hashes": session.source_tx_hashes,
        }))
    }
);
