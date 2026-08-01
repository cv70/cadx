use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{CONFIG_VERSION, Config, Preferences, Settings};

pub const CONFIG_FILE: &str = "config.yaml";
pub const PREFERENCES_FILE: &str = "preferences.yaml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStore {
    root: PathBuf,
}

impl ConfigStore {
    /// Locates the canonical `~/.cadx` settings directory.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::HomeUnavailable`] when the platform cannot
    /// determine the current user's home directory.
    pub fn discover() -> Result<Self, ConfigError> {
        let home = home::home_dir().ok_or(ConfigError::HomeUnavailable)?;
        Ok(Self::at(home.join(".cadx")))
    }

    /// Creates a store rooted at an explicit directory.
    ///
    /// This constructor is intended for dependency injection and tests.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    #[must_use]
    pub fn preferences_path(&self) -> PathBuf {
        self.root.join(PREFERENCES_FILE)
    }

    #[must_use]
    pub fn load(&self) -> Settings {
        let mut warnings = Vec::new();
        let config = match self.load_config() {
            Ok(config) => config,
            Err(error) => {
                warnings.push(format!("{CONFIG_FILE}: {error}"));
                Config::default()
            }
        };
        let preferences = match self.load_preferences() {
            Ok(preferences) => preferences,
            Err(error) => {
                warnings.push(format!("{PREFERENCES_FILE}: {error}"));
                Preferences::default()
            }
        };
        Settings {
            config,
            preferences,
            warnings,
        }
    }

    /// Loads and validates `config.yaml` from this store.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the file cannot be read, parsed, or validated.
    pub fn load_config(&self) -> Result<Config, ConfigError> {
        let path = self.config_path();
        let config: Config = Self::read_yaml_or_default(&path)?;
        validate_version(config.version)?;
        config.provider.validate()?;
        Ok(config)
    }

    /// Loads and validates `preferences.yaml` from this store.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the file cannot be read, parsed, or validated.
    pub fn load_preferences(&self) -> Result<Preferences, ConfigError> {
        let path = self.preferences_path();
        let preferences: Preferences = Self::read_yaml_or_default(&path)?;
        validate_version(preferences.version)?;
        Ok(preferences)
    }

    /// Atomically persists validated provider configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when validation, serialization, or output fails.
    pub fn save_config(&self, config: &Config) -> Result<(), ConfigError> {
        validate_version(config.version)?;
        config.provider.validate()?;
        self.write_yaml(&self.config_path(), config)
    }

    /// Atomically persists validated desktop preferences.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when validation, serialization, or output fails.
    pub fn save_preferences(&self, preferences: &Preferences) -> Result<(), ConfigError> {
        validate_version(preferences.version)?;
        self.write_yaml(&self.preferences_path(), preferences)
    }

    fn read_yaml_or_default<T>(path: &Path) -> Result<T, ConfigError>
    where
        T: DeserializeOwned + Default,
    {
        if !path.exists() {
            return Ok(T::default());
        }
        let source = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        serde_yaml::from_str(&source).map_err(|source| ConfigError::Yaml {
            path: path.to_owned(),
            source,
        })
    }

    fn write_yaml<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), ConfigError> {
        fs::create_dir_all(&self.root).map_err(|source| ConfigError::Io {
            path: self.root.clone(),
            source,
        })?;
        let content = serde_yaml::to_string(value).map_err(|source| ConfigError::Yaml {
            path: path.to_owned(),
            source,
        })?;
        write_private_atomic(path, content.as_bytes())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("home directory is unavailable")]
    HomeUnavailable,
    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid YAML in {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

fn validate_version(version: u32) -> Result<(), ConfigError> {
    if version == 0 || version > CONFIG_VERSION {
        return Err(ConfigError::UnsupportedVersion(version));
    }
    Ok(())
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".cadx-settings-")
        .tempfile_in(parent)
        .map_err(|source| ConfigError::Io {
            path: parent.to_owned(),
            source,
        })?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| ConfigError::Io {
            path: temporary.path().to_owned(),
            source,
        })?;
    temporary.persist(path).map_err(|error| ConfigError::Io {
        path: path.to_owned(),
        source: error.error,
    })?;
    sync_parent(parent).map_err(|source| ConfigError::Io {
        path: parent.to_owned(),
        source,
    })
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_store() -> ConfigStore {
        ConfigStore::at(std::env::temp_dir().join(format!(
            "cadx-config-test-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }

    #[test]
    fn isolated_store_round_trips_and_overwrites_both_files() {
        let store = test_store();
        let mut config = Config::default();
        config.provider.model = "first-model".into();
        let preferences = Preferences {
            language: Some("zh-CN".into()),
            ..Preferences::default()
        };

        store.save_config(&config).unwrap();
        store.save_preferences(&preferences).unwrap();
        config.provider.model = "replacement-model".into();
        store.save_config(&config).unwrap();
        store.save_preferences(&preferences).unwrap();
        assert_eq!(store.load_config().unwrap(), config);
        assert_eq!(store.load_preferences().unwrap(), preferences);
        assert!(
            !fs::read_dir(store.root())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".cadx-settings-"))
        );
        fs::remove_dir_all(store.root()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn settings_files_are_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;

        let store = test_store();
        store.save_config(&Config::default()).unwrap();
        let mode = fs::metadata(store.config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        fs::remove_dir_all(store.root()).unwrap();
    }

    #[test]
    fn missing_files_use_defaults() {
        let store = test_store();
        assert_eq!(store.load(), Settings::default());
    }
}
