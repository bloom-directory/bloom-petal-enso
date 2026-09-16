petal::route_file!(
    spec: petal::store_dir_spec().caps(&["bloom:store"]),
    ctx_list: |ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let wallet = petal::param(ctx, "wallet")?;
        let owner = crate::session::SessionOwner::scope(ctx, wallet)
            .map_err(|error| petal::error(-3, error))?;
        let prefix = owner.store_prefix();
        let mut host = crate::workflow::BloomHost;
        let keys = host
            .list(&prefix, 1024 * 1024)
            .map_err(|error| petal::error(-4, error))?;
        let mut ids = std::collections::BTreeSet::new();
        for key in keys {
            if let Some(rest) = key.strip_prefix(&prefix)
                && let Some((id, file)) = rest.split_once('/')
                && matches!(file, "session.json" | "failure.json")
                && petal::is_safe_segment(id)
            {
                ids.insert(id.to_string());
            }
        }
        let mut children = vec![petal::writable("new"), petal::file("latest")];
        children.extend(ids.into_iter().map(petal::dir));
        Ok(children)
    }
);
