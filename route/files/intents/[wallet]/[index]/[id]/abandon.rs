petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store", "bloom:tx.outbox"]),
    read: |_ctx: &petal::Ctx| {
        petal::DispatchResponse::Read(
            b"write abandon before staging, after an approval finishes, or after the swap is discarded or fails; pending outbox entries must finish first\n"
                .to_vec(),
        )
    },
    write: |ctx: &petal::Ctx, body: &[u8]| {
        let wallet = match crate::wallet::param(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::workflow::abandon_with_body(&mut host, wallet, id, body) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    }
);
