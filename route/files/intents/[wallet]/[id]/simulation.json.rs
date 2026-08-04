petal::route_file!(
    spec: petal::store_read_spec().caps(&["bloom:store", "bloom:chain"]),
    read: |ctx: &petal::Ctx| {
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

        // If a cached simulation exists, return it.
        if let Some(ref cached) = session.simulation {
            return petal::read_json_value(cached);
        }

        // Otherwise, run a fresh eth_call simulation.
        let result = crate::simulation::simulate_route(&mut host, &session);
        petal::read_json_value(&result)
    }
);
