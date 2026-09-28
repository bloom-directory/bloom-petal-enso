petal::route_file!(
    spec: petal::static_dir_spec().caps(&["bloom:vfs.read"]),
    ctx_list: |_ctx: &petal::Ctx| {
        let names = petal::sdk::vfs_list("wallets", 1024 * 1024)
            .map_err(|error| petal::error(-4, error.message()))?;
        Ok(names.into_iter()
            .filter(|name| name != "registrations" && petal::validate_wallet_id(name).is_ok())
            .map(petal::dir).collect())
    }
);
