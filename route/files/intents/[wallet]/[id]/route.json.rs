petal::route_file!(spec: petal::store_read_spec(), read: |ctx: &petal::Ctx| {
    use crate::workflow::Host;

    let wallet = match petal::param(ctx, "wallet") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match petal::param(ctx, "id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut host = crate::workflow::BloomHost;
    let session = match crate::workflow::load(&mut host, wallet, id) {
        Ok(value) => value,
        Err(error) => return petal::error(-1, error),
    };
    match session.route {
        Some(ref route) => petal::read_json_value(&serde_json::to_value(route).unwrap_or_default()),
        None => petal::error(-1, "route not available"),
    }
});
