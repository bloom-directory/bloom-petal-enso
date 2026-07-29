petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store"]),
    read: |_ctx: &petal::Ctx| {
        petal::DispatchResponse::Read(
            b"write your Enso API key here (plain text or JSON {\"api_key\":\"...\"})\n".to_vec(),
        )
    },
    write: |_ctx: &petal::Ctx, body: &[u8]| {
        use crate::workflow::Host;
        let key = match crate::settings::parse_api_key(body) {
            Ok(value) => value,
            Err(error) => return petal::error(-3, error),
        };
        let mut host = crate::workflow::BloomHost;
        match host.put(crate::settings::API_KEY, key.expose().as_bytes(), true) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    }
);
