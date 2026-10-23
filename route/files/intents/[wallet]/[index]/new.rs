petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:http", "bloom:store", "bloom:chain", "bloom:vfs.read"]),
    read: |_ctx: &petal::Ctx| {
        petal::DispatchResponse::Read(
            br#"{"intent":"swap 1 ETH to USDC","chain":"ethereum"}"#.to_vec(),
        )
    },
    write: |ctx: &petal::Ctx, body: &[u8]| {
        let account = match crate::wallet::account(ctx) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        match crate::workflow::create_for_account(&mut host, &account, body) {
            Ok(_) => petal::DispatchResponse::Write,
            Err(crate::workflow::CreateError::InvalidInput(message)) => {
                petal::error(-3, crate::redaction::sanitize_message(&message))
            }
            Err(crate::workflow::CreateError::Account { code, message }) => {
                petal::error(code, crate::redaction::sanitize_message(&message))
            }
            Err(crate::workflow::CreateError::Failed(message)) => {
                petal::error(-4, crate::redaction::sanitize_message(&message))
            }
        }
    }
);
