petal::route_file!(spec: petal::store_read_spec(), read: |ctx: &petal::Ctx| {
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
    match crate::workflow::load(&mut host, wallet, id) {
        Ok(session) => petal::read_json_value(&serde_json::json!({
            "id": session.id,
            "wallet": session.wallet,
            "state": session.state,
            "updated_ms": session.updated_ms,
            "route_verified": session.route_verified,
            "outbox_id": session.outbox_id,
            "outbox_state": session.outbox_state,
            "tx_hash": session.tx_hash,
            "last_error": session.last_error,
            "history": session.history,
        })),
        Err(error) => match host.get(&crate::session::failure_key(wallet, id), 64 * 1024) {
            Ok(Some(raw)) => petal::DispatchResponse::Read(raw),
            _ => petal::error(-1, error),
        },
    }
});
