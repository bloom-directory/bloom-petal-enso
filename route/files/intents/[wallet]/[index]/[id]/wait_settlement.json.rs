// One bounded settlement read. Clients may re-read after the retry hint.

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
        let mut result = crate::settlement::settlement_status(&mut host, &sess);
        if let Some(object) = result.as_object_mut() {
            if object.get("retry_supported") != Some(&serde_json::json!(false)) {
                object.insert("retry_interval_ms".into(), serde_json::json!(10000));
            }
        }
        petal::read_json_value(&result)
    }
);
