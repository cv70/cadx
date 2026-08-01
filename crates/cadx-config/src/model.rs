use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ConfigError;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub provider: ProviderConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            provider: ProviderConfig::default(),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderConfig {
    pub endpoint: Option<String>,
    pub model: String,
    pub api_key: Option<String>,
    pub adapter: Option<String>,
    pub timeout_seconds: u64,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderConfig")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "REDACTED"))
            .field("adapter", &self.adapter)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            model: "gpt-4.1-mini".into(),
            api_key: None,
            adapter: None,
            timeout_seconds: 45,
        }
    }
}

impl ProviderConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.model.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "provider.model must not be empty".into(),
            ));
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > 3_600 {
            return Err(ConfigError::Invalid(
                "provider.timeout_seconds must be between 1 and 3600".into(),
            ));
        }
        if self
            .endpoint
            .as_deref()
            .is_some_and(|endpoint| endpoint.trim().is_empty())
        {
            return Err(ConfigError::Invalid(
                "provider.endpoint must be a non-empty URL when configured".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub language: Option<String>,
    pub cjk_font: Option<PathBuf>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            language: None,
            cjk_font: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub config: Config,
    pub preferences: Preferences,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_yaml_without_logging_secrets() {
        let config: Config = serde_yaml::from_str(
            r"
version: 1
provider:
  endpoint: https://example.test/v1
  model: gpt-5.6-luna
  api_key: secret-value
  timeout_seconds: 90
",
        )
        .unwrap();
        assert_eq!(config.provider.model, "gpt-5.6-luna");
        assert_eq!(config.provider.timeout_seconds, 90);
        assert_eq!(config.provider.api_key.as_deref(), Some("secret-value"));
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-value"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn defaults_missing_sections() {
        let config: Config = serde_yaml::from_str("version: 1\n").unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn rejects_unsafe_timeout() {
        let config = ProviderConfig {
            timeout_seconds: 0,
            ..ProviderConfig::default()
        };
        assert!(config.validate().is_err());
    }
}
