//! Bloom wallet-id validation compatible with the wallet surface.

/// Validate the wallet names accepted by Bloom itself.
pub fn validate_id(value: &str) -> Result<&str, String> {
    if value.len() == 42
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(
            "wallet must be a Bloom wallet id, not an on-chain address; use the id under /bloom/wallets/<id>"
                .into(),
        );
    }
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "wallet must be a Bloom wallet id containing 1-64 ASCII alphanumeric, '-' or '_' characters"
                .into(),
        );
    }
    Ok(value)
}

/// Read and validate the generated `[wallet]` route parameter.
pub fn param(ctx: &petal::Ctx) -> Result<&str, petal::DispatchResponse> {
    let value = petal::param(ctx, "wallet")?;
    validate_id(value).map_err(petal::route_invalid)
}

/// The wallet and account Bloom selected for this route.
///
/// The SDK reads the pair from trusted context and rejects anything that is not
/// a single path segment. Enso's own wallet grammar is checked on top, so this
/// route refuses the same names as every other route here.
pub fn account(ctx: &petal::Ctx) -> Result<petal::WalletAccount, petal::DispatchResponse> {
    let account = petal::wallet_account(ctx)?;
    validate_id(account.wallet()).map_err(petal::route_invalid)?;
    Ok(account)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NewIntent;
    impl petal::RouteIdentity for NewIntent {
        const PATH: &'static str = "intents/[wallet]/[index]/new";
        const CANONICAL_PATH: &'static str = "intents/[wallet]/[index]/new";
        const PARAMS: &'static [(&'static str, usize)] = &[("wallet", 1), ("index", 2)];
    }

    fn selected(
        wallet: &str,
        number: u32,
    ) -> Result<petal::WalletAccount, petal::DispatchResponse> {
        let ctx = petal::testing::RouteCtx::new(&format!("intents/{wallet}/{number}/new"))
            .account(wallet, number)
            .bind::<NewIntent>();
        account(&ctx)
    }

    #[test]
    fn the_create_route_refuses_a_wallet_bloom_would_never_select() {
        // Each is refused at the boundary, before any read or request, by the
        // SDK (empty, `..`) or by enso's grammar on top of it (space, dot).
        for wallet in ["", "..", "test wallet", "wallet.name"] {
            assert!(
                matches!(
                    selected(wallet, 0),
                    Err(petal::DispatchResponse::Error { code: -3, .. })
                ),
                "{wallet:?}"
            );
        }
        let alice = selected("Alice", 1).expect("a Bloom wallet name");
        assert_eq!((alice.wallet(), alice.number()), ("Alice", 1));
    }

    #[test]
    fn matches_bloom_wallet_name_grammar() {
        for valid in ["a", "Alice", "release-test", "release_test", "A1"] {
            assert_eq!(validate_id(valid), Ok(valid));
        }
        for invalid in ["", "../etc", "test wallet", "wallet.name"] {
            assert!(validate_id(invalid).is_err(), "{invalid}");
        }
    }
}
