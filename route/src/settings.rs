use serde::Serialize;

pub const API_KEY: &str = "credentials/enso-api-key";
const EMBEDDED_ENSO_API_KEY: Option<&str> = option_env!("ENSO_API_KEY");

#[derive(Clone)]
pub struct ApiKey(String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

impl ApiKey {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    EmbeddedRelease,
    PrivateStore,
    RuntimeSetting,
    Unconfigured,
}

#[derive(Debug)]
pub struct ResolvedApiKey {
    pub key: ApiKey,
    pub source: CredentialSource,
}

#[derive(Debug, Serialize)]
pub struct CredentialStatus {
    pub configured: bool,
    pub source: CredentialSource,
    pub storage: &'static str,
    pub encrypted_at_rest: bool,
}

pub fn parse_api_key(body: &[u8]) -> Result<ApiKey, String> {
    if body.is_empty() || body.len() > 8192 {
        return Err("API key must be 1..=8192 bytes".into());
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| "API key must be UTF-8")?
        .trim();
    let token = if text.starts_with('{') {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            api_key: String,
        }
        serde_json::from_str::<Input>(text)
            .map_err(|_| "API key JSON must contain only api_key")?
            .api_key
    } else {
        text.to_string()
    };
    let token = token.trim();
    if token.is_empty()
        || token.len() > 8192
        || token.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("API key must be 1..=8192 non-whitespace characters".into());
    }
    Ok(ApiKey(token.into()))
}

/// Resolve the API key from the private store first, then the embedded release
/// credential and legacy runtime setting `enso-api-key`.
pub fn resolve_api_key(
    embedded: Option<&str>,
    private_store: Option<&[u8]>,
    setting: Option<&str>,
) -> Result<ResolvedApiKey, String> {
    if let Some(raw) = private_store {
        return Ok(ResolvedApiKey {
            key: parse_api_key(raw)?,
            source: CredentialSource::PrivateStore,
        });
    }
    if let Some(raw) = embedded {
        return Ok(ResolvedApiKey {
            key: parse_api_key(raw.as_bytes())?,
            source: CredentialSource::EmbeddedRelease,
        });
    }
    if let Some(raw) = setting {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(ResolvedApiKey {
                key: parse_api_key(trimmed.as_bytes())?,
                source: CredentialSource::RuntimeSetting,
            });
        }
    }
    Err("Enso API key is not configured".into())
}

pub fn configured_api_key(
    private_store: Option<&[u8]>,
    setting: Option<&str>,
) -> Result<ResolvedApiKey, String> {
    resolve_api_key(EMBEDDED_ENSO_API_KEY, private_store, setting)
}

pub fn status(
    embedded: Option<&str>,
    private_store: Option<&[u8]>,
    setting: Option<&str>,
) -> CredentialStatus {
    let (configured, source, storage, encrypted_at_rest) = if let Some(raw) = private_store {
        (
            parse_api_key(raw).is_ok(),
            CredentialSource::PrivateStore,
            "petal secret store",
            true,
        )
    } else if let Some(raw) = embedded {
        (
            parse_api_key(raw.as_bytes()).is_ok(),
            CredentialSource::EmbeddedRelease,
            "release artifact",
            false,
        )
    } else if let Some(raw) = setting.filter(|raw| !raw.trim().is_empty()) {
        (
            parse_api_key(raw.trim().as_bytes()).is_ok(),
            CredentialSource::RuntimeSetting,
            "Bloom runtime configuration",
            false,
        )
    } else {
        (false, CredentialSource::Unconfigured, "none", false)
    };
    CredentialStatus {
        configured,
        source,
        storage,
        encrypted_at_rest,
    }
}

pub fn configured_status(private_store: Option<&[u8]>, setting: Option<&str>) -> CredentialStatus {
    status(EMBEDDED_ENSO_API_KEY, private_store, setting)
}

#[cfg(test)]
mod tests {
    use super::*;
    const TEST_KEY: &str = "enso_test_must_never_appear";

    #[test]
    fn parses_plain_key() {
        let key = parse_api_key(b"enso_abc123").unwrap();
        assert_eq!(key.expose(), "enso_abc123");
    }

    #[test]
    fn parses_json_key() {
        let key = parse_api_key(br#"{"api_key":"enso_xyz"}"#).unwrap();
        assert_eq!(key.expose(), "enso_xyz");
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_api_key(b"").is_err());
        assert!(parse_api_key(b"   ").is_err());
    }

    #[test]
    fn runtime_setting_is_not_claimed_as_encrypted() {
        let status = status(None, None, Some("enso_abc"));
        assert_eq!(status.source, CredentialSource::RuntimeSetting);
        assert!(!status.encrypted_at_rest);
        assert_eq!(status.storage, "Bloom runtime configuration");
    }

    #[test]
    fn private_store_key_overrides_embedded_and_runtime_values() {
        let resolved =
            resolve_api_key(Some(TEST_KEY), Some(b"enso_private"), Some("enso_runtime")).unwrap();
        assert_eq!(resolved.source, CredentialSource::PrivateStore);
        assert_eq!(resolved.key.expose(), "enso_private");

        let status = status(Some(TEST_KEY), Some(b"enso_private"), Some("enso_runtime"));
        assert!(status.configured);
        assert_eq!(status.source, CredentialSource::PrivateStore);
        assert_eq!(status.storage, "petal secret store");
        assert!(status.encrypted_at_rest);
    }

    #[test]
    fn embedded_release_key_overrides_runtime_setting() {
        let embedded = resolve_api_key(Some(TEST_KEY), None, Some("enso_runtime")).unwrap();
        assert_eq!(embedded.source, CredentialSource::EmbeddedRelease);
        assert_eq!(embedded.key.expose(), TEST_KEY);

        let runtime = resolve_api_key(None, None, Some("enso_runtime")).unwrap();
        assert_eq!(runtime.source, CredentialSource::RuntimeSetting);
        assert_eq!(runtime.key.expose(), "enso_runtime");
    }

    #[test]
    fn malformed_embedded_key_fails_closed_without_leaking() {
        let malformed = "enso_test_must_never_appear malformed";
        let error = resolve_api_key(Some(malformed), None, Some("enso_runtime")).unwrap_err();
        let status = status(Some(malformed), None, Some("enso_runtime"));

        assert!(!error.contains(malformed));
        assert!(!status.configured);
        assert_eq!(status.source, CredentialSource::EmbeddedRelease);
        assert!(!format!("{status:?}").contains(malformed));
    }

    #[test]
    fn malformed_private_key_does_not_fall_back_to_embedded_key() {
        let malformed = "enso_private_must_never_appear malformed";
        let error = resolve_api_key(Some(TEST_KEY), Some(malformed.as_bytes()), None).unwrap_err();
        let status = status(Some(TEST_KEY), Some(malformed.as_bytes()), None);

        assert!(!error.contains(malformed));
        assert!(!status.configured);
        assert_eq!(status.source, CredentialSource::PrivateStore);
        assert!(!format!("{status:?}").contains(malformed));
    }

    #[test]
    fn debug_and_serialized_status_do_not_reveal_embedded_key() {
        let resolved = resolve_api_key(Some(TEST_KEY), None, None).unwrap();
        let status = status(Some(TEST_KEY), None, None);
        let output = format!(
            "{resolved:?}\n{status:?}\n{}",
            serde_json::to_string(&status).unwrap()
        );

        assert!(!output.contains(TEST_KEY));
        assert!(output.contains("embedded_release"));
    }
}
