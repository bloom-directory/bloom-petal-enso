petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store"]),
    read: |ctx: &petal::Ctx| {
        let wallet = match petal::wallet_param(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::policy::read_venue_config(&mut host, wallet) {
            Ok(value) => petal::DispatchResponse::Read(value),
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    },
    write: |ctx: &petal::Ctx, body: &[u8]| {
        let wallet = match petal::wallet_param(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::policy::write_venue_config(&mut host, wallet, body) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-3, crate::redaction::sanitize_message(&error)),
        }
    }
);
