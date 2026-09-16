petal::route_file!(spec: petal::store_read_spec(), read: |ctx: &petal::Ctx| {
    use crate::workflow::Host;

    let wallet = match petal::param(ctx, "wallet") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let owner = match crate::session::SessionOwner::scope(ctx, wallet) {
        Ok(value) => value,
        Err(error) => return petal::error(-3, error),
    };
    let mut host = crate::workflow::BloomHost;
    match host.get(&owner.latest_key(), 128) {
        Ok(Some(value)) => petal::DispatchResponse::Read(value),
        Ok(None) => petal::error(-1, "no intent session"),
        Err(error) => petal::error(-4, error),
    }
});
