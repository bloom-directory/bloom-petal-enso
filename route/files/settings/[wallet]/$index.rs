petal::route_file!(
    spec: petal::static_dir_spec(),
    ctx_list: |ctx: &petal::Ctx| {
        let wallet = petal::param(ctx, "wallet")?;
        crate::workflow::validate_wallet_name(wallet).map_err(|error| petal::error(-3, error))?;
        Ok(vec![petal::writable("route-rules.toml")])
    }
);
