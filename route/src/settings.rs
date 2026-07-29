use serde::Serialize;

pub const API_KEY: &str = "credentials/enso-api-key";

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

/// Resolve the API key from the private store first, then the runtime setting
/// `enso-api-key`.
pub fn resolve_api_key(
    private_store: Option<&[u8]>,
    setting: Option<&str>,
) -> Result<ResolvedApiKey, String> {
    if let Some(raw) = private_store {
        return Ok(ResolvedApiKey {
            key: parse_api_key(raw)?,
            source: CredentialSource::PrivateStore,
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

pub fn configured_status(private_store: Option<&[u8]>, setting: Option<&str>) -> CredentialStatus {
    let resolved = resolve_api_key(private_store, setting);
    let source = match resolved {
        Ok(r) => r.source,
        Err(_) => CredentialSource::Unconfigured,
    };
    let (storage, encrypted_at_rest) = match source {
        CredentialSource::PrivateStore => ("petal secret store", true),
        CredentialSource::RuntimeSetting => ("Bloom runtime configuration", false),
        CredentialSource::Unconfigured => ("none", false),
    };
    CredentialStatus {
        configured: !matches!(source, CredentialSource::Unconfigured),
        source,
        storage,
        encrypted_at_rest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let status = configured_status(None, Some("enso_abc"));
        assert_eq!(status.source, CredentialSource::RuntimeSetting);
        assert!(!status.encrypted_at_rest);
        assert_eq!(status.storage, "Bloom runtime configuration");
    }
}
