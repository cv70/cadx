use std::fmt;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::ConfigError;
use crate::egress::{canonicalize_model, canonicalize_provider_endpoint};
use crate::paths::{ensure_default_working_directory, open_private_config_file};
use serde::Deserialize;

pub const CURRENT_CONFIG_VERSION: u32 = 1;
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;
pub const DEFAULT_PROVIDER_TIMEOUT_SECONDS: u64 = 45;
pub const MAX_PROVIDER_TIMEOUT_SECONDS: u64 = 300;

/// The root schema of `~/.cadx/config.yaml`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CadxConfig {
    pub version: u32,
    pub provider: ProviderSettings,
}

impl fmt::Debug for CadxConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CadxConfig")
            .field("version", &self.version)
            .field("provider", &self.provider)
            .finish()
    }
}

impl CadxConfig {
    /// Loads the standard user-scoped CADX configuration file.
    pub fn load_default() -> Result<Self, ConfigError> {
        let path = ensure_default_working_directory()?.join(crate::paths::CONFIG_FILE_NAME);
        load_config(&path)
    }

    /// Loads a configuration file at an explicit path. This is useful for
    /// controlled embedding and focused tests; the default application path is
    /// always `~/.cadx/config.yaml`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        load_config(path.as_ref())
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CURRENT_CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        self.provider.validate()
    }
}

/// OpenAI Responses-compatible provider settings loaded from local YAML.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSettings {
    pub endpoint: String,
    pub model: String,
    api_key: String,
    #[serde(default = "default_provider_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl fmt::Debug for ProviderSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderSettings")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("api_key", &"REDACTED")
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

impl ProviderSettings {
    /// Returns the credential only for constructing an in-memory provider request.
    /// Callers must not persist or log the returned value.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.api_key.trim().is_empty() {
            return Err(ConfigError::InvalidProvider("provider API key is required"));
        }
        if self.model.trim().is_empty() {
            return Err(ConfigError::InvalidProvider("provider model is required"));
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > MAX_PROVIDER_TIMEOUT_SECONDS {
            return Err(ConfigError::InvalidProvider(
                "provider timeout_seconds must be between 1 and 300",
            ));
        }
        canonicalize_provider_endpoint(&self.endpoint).map_err(ConfigError::InvalidProvider)?;
        canonicalize_model(&self.model).map_err(ConfigError::InvalidProvider)?;
        Ok(())
    }
}

fn default_provider_timeout_seconds() -> u64 {
    DEFAULT_PROVIDER_TIMEOUT_SECONDS
}

fn load_config(path: &Path) -> Result<CadxConfig, ConfigError> {
    let (mut file, metadata) = open_private_config_file(path)?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::ConfigTooLarge {
            path: path.into(),
            limit: MAX_CONFIG_BYTES,
        });
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| ConfigError::io(path, error))?;
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::ConfigTooLarge {
            path: path.into(),
            limit: MAX_CONFIG_BYTES,
        });
    }
    let config = serde_yaml::from_slice::<CadxConfig>(&contents)
        .map_err(|_| ConfigError::InvalidYaml(path.into()))?;
    config.validate()?;
    Ok(config)
}
