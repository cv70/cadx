//! Typed configuration and preference storage for CADX.
//!
//! All runtime settings are loaded from versioned YAML files below
//! `~/.cadx`. Callers receive validated values and never consult provider or
//! preference environment variables.

mod model;
mod store;

pub use model::{CONFIG_VERSION, Config, Preferences, ProviderConfig, Settings};
pub use store::{CONFIG_FILE, ConfigError, ConfigStore, PREFERENCES_FILE};
