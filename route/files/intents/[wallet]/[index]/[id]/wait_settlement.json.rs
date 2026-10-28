// One bounded settlement read. Only results with `retry_interval_ms` can still change.

petal::route_file!(
    spec: petal::store_read_spec().caps(&["bloom:store", "bloom:chain", "bloom:tx.outbox"]),
    read: |ctx: &petal::Ctx| {
        let wallet = match crate::wallet::param(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        let sess = match crate::workflow::load(&mut host, wallet, id) {
            Ok(s) => s,
            Err(error) => return petal::error(-1, error),
        };

        // If not staged yet, nothing to wait for.
        if sess.staged_ids.is_empty() {
            return petal::read_json_value(&serde_json::json!({
                "status": "not_staged",
                "message": "no transactions have been staged into the outbox yet",
            }));
        }

        // Check settlement immediately.
        petal::read_json_value(&crate::settlement::settlement_status(&mut host, &sess))
    }
);
