petal::route_file!(spec: petal::store_read_spec().caps(&["bloom:store", "bloom:tx.outbox", "bloom:chain"]), read: |ctx: &petal::Ctx| {
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

    let mut status = serde_json::json!({
        "session": session.id,
        "wallet": session.wallet,
        "chain": session.chain,
        "state": session.state,
        "outbox_id": session.outbox_id,
        "outbox_state": session.outbox_state,
        "tx_hash": session.tx_hash,
    });

    // If we have an outbox id, inspect it for settlement info.
    if let (Some(ref outbox_id), Some(ref chain)) = (&session.outbox_id, session.chain.as_str().into().and_then(|_| Some(session.chain.as_str()))) {
        if let Ok(inspection) = host.tx_inspect(wallet, &session.chain, outbox_id) {
            status["outbox_state"] = serde_json::Value::String(inspection.state.clone());
            status["tx_hash"] = inspection.tx_hash
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null);
            if let Some(ref receipt) = inspection.receipt_json {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(receipt) {
                    status["receipt"] = parsed;
                }
            }
        }
    }

    petal::read_json_value(&status)
});
