petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store"]),
    read: |ctx: &petal::Ctx| {
        let wallet = match crate::wallet::param(ctx) {
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
        use crate::workflow::Host;
        let wallet = match crate::wallet::param(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let key = match crate::policy::validate_venue_config_write(wallet, body) {
            Ok(value) => value,
            Err(error) => return petal::error(-3, crate::redaction::sanitize_message(&error)),
        };
        let mut host = crate::workflow::BloomHost;
        match host.put(&key, body, false) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    }
);
