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

        if !session.terminal() && session.state != "staged" {
            return petal::error(-1, "receipt not available — session is not terminal or staged");
        }

        // Gather outbox receipts for all staged intents.
        let mut tx_receipts: Vec<serde_json::Value> = Vec::new();
        for state in &session.intent_states {
            if let Some(ref oid) = state.outbox_id {
                let label = session
                    .intents
                    .get(state.index)
                    .map(|i| i.label.as_str())
                    .unwrap_or("?");
                match host.tx_inspect(&session.wallet, &session.chain, oid) {
                    Ok(insp) => {
                        tx_receipts.push(serde_json::json!({
                            "label": label,
                            "outbox_id": oid,
                            "state": insp.state,
                            "tx_hash": insp.tx_hash,
                            "receipt": insp.receipt_json.as_deref()
                                .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()),
                        }));
                    }
                    Err(_) => {}
                }
            }
        }

        // Settlement status.
        let settlement = crate::settlement::settlement_status(&mut host, &session);

        petal::read_json_value(&serde_json::json!({
            "session": session.id,
            "wallet": session.wallet,
            "chain": session.chain,
            "destination_chain": session.destination_chain,
            "state": session.state,
            "intent_text": session.intent_text,
            "primary_tx_hash": session.primary_tx_hash(),
            "primary_outbox_id": session.primary_outbox_id(),
            "transactions": tx_receipts,
            "settlement": settlement,
            "history": session.history,
            "last_error": session.last_error,
        }))
    }
);
