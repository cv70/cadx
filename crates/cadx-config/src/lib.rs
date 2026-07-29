//! Persistent CADX working-directory and provider configuration.
//!
//! Configuration is deliberately kept outside project archives. Provider
//! credentials are parsed from the private user configuration file and exposed
//! only through an accessor; they are never included in `Debug` output.

mod error;
mod paths;
mod preferences;
mod settings;

#[cfg(test)]
mod tests;

pub use error::ConfigError;
pub use paths::{
    cadx_home, default_config_path, default_preferences_path, default_project_path,
    ensure_default_working_directory, initialize_default_config_if_missing,
};
pub use preferences::{CadxPreferences, UiLanguage};
pub use settings::{CadxConfig, ProviderSettings};
