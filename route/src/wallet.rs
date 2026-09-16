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

#[cfg(test)]
mod tests {
    use super::*;

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
