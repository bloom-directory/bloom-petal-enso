petal::route_file!(spec: petal::store_read_spec().caps(&["bloom:store"]), read: |ctx: &petal::Ctx| {
    let wallet = match petal::param(ctx, "wallet") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match petal::param(ctx, "id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut host = crate::workflow::BloomHost;
    match crate::workflow::load(&mut host, wallet, id) {
        Ok(session) => {
            let dest = session.destination_chain.as_deref().unwrap_or(&session.chain);
            petal::DispatchResponse::Read(dest.as_bytes().to_vec())
        }
        Err(error) => petal::error(-1, error),
    }
});
