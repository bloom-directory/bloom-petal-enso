petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store", "bloom:tx.outbox", "bloom:chain"]),
    read: |_ctx: &petal::Ctx| {
        petal::DispatchResponse::Read(
            b"write confirm to stage the prepared transaction into the wallet outbox\n".to_vec(),
        )
    },
    write: |ctx: &petal::Ctx, body: &[u8]| {
        let wallet = match petal::param(ctx, "wallet") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::workflow::confirm(&mut host, wallet, id, body) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    }
);
