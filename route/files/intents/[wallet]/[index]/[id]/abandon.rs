petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store", "bloom:tx.outbox"]),
    read: |_ctx: &petal::Ctx| {
        petal::DispatchResponse::Read(
            b"write abandon before staging, after an approval finishes, or after the swap is discarded or fails; pending outbox entries must finish first, and a transaction that may be in the outbox unrecorded is named once before the next abandon closes the intent\n"
                .to_vec(),
        )
    },
    write: |ctx: &petal::Ctx, body: &[u8]| {
        let account = match crate::wallet::account(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::workflow::abandon_with_body(&mut host, &account, id, body) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    }
);
