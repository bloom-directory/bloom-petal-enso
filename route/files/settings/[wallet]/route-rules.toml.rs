petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store"]),
    read: |ctx: &petal::Ctx| {
        use crate::workflow::Host;

        let wallet = match petal::param(ctx, "wallet") {
            Ok(value) => value,
            Err(response) => return response,
        };
        if let Err(error) = crate::workflow::validate_wallet_name(wallet) {
            return petal::error(-3, error);
        }
        let mut host = crate::workflow::BloomHost;
        match host.get(&crate::policy::route_rules_key(wallet), 256 * 1024) {
            Ok(Some(rules)) => petal::DispatchResponse::Read(rules),
            Ok(None) => petal::DispatchResponse::Read(br#"# configured: false
# Write validated Enso route rules for this wallet here. Rules apply only to
# this wallet; while none are configured, all of its routes are denied.
[mev]
max_slippage_bps = 100

[defi]
enabled = true
allowed_source_chains = ["ethereum"]
allowed_destination_chains = ["ethereum"]
allowed_receivers = ["class:wallet_eoa"]
denied_receivers = []
# Add only operator-approved Enso route targets, formatted as
# "<chain>:<0x-prefixed EVM address>". An empty list denies every router.
allowed_routers = []
denied_protocols = []
allow_unknown_protocols = false
require_calldata_verification = true
"#.to_vec()),
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    },
    write: |ctx: &petal::Ctx, body: &[u8]| {
        use crate::workflow::Host;

        let wallet = match petal::param(ctx, "wallet") {
            Ok(value) => value,
            Err(response) => return response,
        };
        if let Err(error) = crate::workflow::validate_wallet_name(wallet) {
            return petal::error(-3, error);
        }
        if let Err(error) = crate::policy::parse_route_rules(body) {
            return petal::error(-3, error);
        }
        let mut host = crate::workflow::BloomHost;
        match host.put(&crate::policy::route_rules_key(wallet), body, false) {
            Ok(()) => petal::DispatchResponse::Write,
            Err(error) => petal::error(-4, crate::redaction::sanitize_message(&error)),
        }
    }
);
