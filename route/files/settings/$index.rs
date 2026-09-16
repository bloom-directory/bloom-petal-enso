petal::route_file!(
    spec: petal::store_dir_spec().caps(&["bloom:store"]),
    ctx_list: |_ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let mut host = crate::workflow::BloomHost;
        let keys = host
            .list("settings/", 1024 * 1024)
            .map_err(|error| petal::error(-4, crate::redaction::sanitize_message(&error)))?;
        let mut children = std::collections::BTreeMap::from([
            ("api-key".to_string(), petal::writable("api-key")),
            ("status.json".to_string(), petal::file("status.json")),
        ]);
        for key in keys {
            if let Some(rest) = key.strip_prefix("settings/")
                && let Some((wallet, file)) = rest.split_once('/')
                && file == "venue.toml"
                && petal::validate_wallet_id(wallet).is_ok()
            {
                children.insert(wallet.to_string(), petal::dir(wallet));
            }
        }
        Ok(children.into_values().collect())
    }
);
