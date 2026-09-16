petal::route_file!(spec: petal::store_read_spec(), read: |ctx: &petal::Ctx| {
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

    let req = match session.route_request.as_ref() {
        Some(r) => r,
        None => return petal::error(-1, "no route request available"),
    };
    let route = match session.route.as_ref() {
        Some(r) => r,
        None => return petal::error(-1, "no route available"),
    };

    // Build the ceremony review payload — the prepared transaction(s)
    // that bloom's ceremony system reads to present for user approval.
    let primary_intent = session
        .intents
        .iter()
        .rev()
        .find(|i| i.label == "route")
        .or(session.intents.last());

    let approval = session
        .intent_states
        .iter()
        .rev()
        .find_map(|s| s.approval.clone());

    petal::read_json_value(&serde_json::json!({
        "petal": "enso",
        "session": session.id,
        "wallet": session.wallet,
        "wallet_address": session.wallet_address,
        "chain": session.chain,
        "chain_id": req.chain_id,
        "destination_chain": session.destination_chain,
        "intent_text": session.intent_text,
        "token_in": format!("0x{:x}", req.token_in),
        "token_out": format!("0x{:x}", req.token_out),
        "amount_in": req.amount_in.to_string(),
        "amount_out": route.amount_out.clone(),
        "slippage_bps": req.slippage_bps,
        "receiver": req.receiver.map(|a| format!("0x{:x}", a)),
        "receiver_class": session.receiver_class,
        "state": session.state,
        "primary_transaction": primary_intent.map(|i| serde_json::json!({
            "to": i.to,
            "value_wei": i.value_wei,
            "data_hex": i.data_hex,
            "chain": i.chain,
        })),
        "intent_count": session.intents.len(),
        "approval": approval,
        "policy_overall": session.policy_overall(),
    }))
});
