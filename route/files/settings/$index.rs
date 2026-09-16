petal::route_file!(
    spec: petal::store_dir_spec().caps(&["bloom:store"]),
    ctx_list: |_ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let prefix = "settings/";
        let mut host = crate::workflow::BloomHost;
        let keys = host
            .list(prefix, 1024 * 1024)
            .map_err(|error| petal::error(-4, error))?;
        let mut wallets = std::collections::BTreeSet::new();
        for key in keys {
            if let Some(rest) = key.strip_prefix(prefix)
                && let Some((wallet, file)) = rest.split_once('/')
                && file == "route-rules.toml"
                && crate::workflow::validate_wallet_name(wallet).is_ok()
            {
                wallets.insert(wallet.to_string());
            }
        }
        let mut children = vec![petal::file("status.json"), petal::writable("api-key")];
        children.extend(wallets.into_iter().map(petal::dir));
        Ok(children)
    }
);
