petal::route_file!(spec: petal::store_read_spec().caps(&["bloom:store"]), read: |ctx: &petal::Ctx| {
    use crate::workflow::Host;

    let mut host = crate::workflow::BloomHost;
    let private_store = host.get_secret(crate::settings::API_KEY, 8192).unwrap_or(None);
    let runtime = host.setting("enso-api-key").unwrap_or(None);
    let status = crate::settings::configured_status(
        private_store.as_deref(),
        runtime.as_deref(),
    );
    petal::read_json_value(&serde_json::json!({
        "configured": status.configured,
        "source": status.source,
        "storage": status.storage,
        "encrypted_at_rest": status.encrypted_at_rest,
    }))
});
