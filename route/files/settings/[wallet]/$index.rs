petal::route_file!(
    spec: petal::static_dir_spec().caps(&["bloom:vfs.read"]),
    ctx_list: |ctx: &petal::Ctx| {
        let wallet = petal::wallet_param(ctx)?;
        let names = petal::sdk::vfs_list(&format!("wallets/{wallet}"), 1024 * 1024)
            .map_err(|error| petal::error(-4, error.message()))?;
        Ok(names.into_iter()
            .filter(|name| name.parse::<u32>().is_ok_and(|index| index.to_string() == *name))
            .map(petal::dir).collect())
    }
);
