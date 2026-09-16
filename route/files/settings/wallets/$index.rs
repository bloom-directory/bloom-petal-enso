petal::route_file!(
    spec: petal::store_dir_spec().caps(&["bloom:store"]),
    ctx_list: |_ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let mut host = crate::workflow::BloomHost;
        let keys = host
            .list("settings/", 1024 * 1024)
            .map_err(|error| petal::error(-4, crate::redaction::sanitize_message(&error)))?;
        let mut children = std::collections::BTreeSet::new();
        for key in keys {
            for prefix in ["settings/wallets/", "settings/"] {
                if let Some(rest) = key.strip_prefix(prefix)
                    && let Some((wallet, file)) = rest.split_once('/')
                    && file == "venue.toml"
                    && petal::validate_wallet_id(wallet).is_ok()
                {
                    children.insert(wallet.to_string());
                }
            }
        }
        Ok(children.into_iter().map(petal::dir).collect())
    }
);
