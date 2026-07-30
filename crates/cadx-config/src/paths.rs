use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::ConfigError;

pub const CADX_DIRECTORY_NAME: &str = ".cadx";
pub const CONFIG_FILE_NAME: &str = "config.yaml";
pub const EGRESS_POLICY_FILE_NAME: &str = "egress-policy.yaml";
pub const PREFERENCES_FILE_NAME: &str = "preferences.yaml";
pub const PROJECTS_DIRECTORY_NAME: &str = "projects";
pub const DEFAULT_PROJECT_FILE_NAME: &str = "Untitled.cadx";

const DEFAULT_CONFIG: &str = r#"version: 1

provider:
  endpoint: "https://api.openai.com/v1"
  model: "gpt-5.6-luna"
  api_key: ""
  timeout_seconds: 45
"#;

const DEFAULT_EGRESS_POLICY: &str = r#"version: 1

allowed_providers:
  - endpoint: "https://api.openai.com/v1"
    models:
      - "gpt-5.6-luna"
"#;

/// Returns the private, user-scoped CADX working directory (`~/.cadx`).
pub fn cadx_home() -> Result<PathBuf, ConfigError> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(platform_home_directory)
        .ok_or(ConfigError::HomeDirectoryUnavailable)?;
    Ok(home.join(CADX_DIRECTORY_NAME))
}

/// Returns the default provider configuration path (`~/.cadx/config.yaml`).
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    Ok(cadx_home()?.join(CONFIG_FILE_NAME))
}

/// Returns the independent local provider egress policy path.
pub fn default_egress_policy_path() -> Result<PathBuf, ConfigError> {
    Ok(cadx_home()?.join(EGRESS_POLICY_FILE_NAME))
}

/// Returns the user-interface preference path (`~/.cadx/preferences.yaml`).
pub fn default_preferences_path() -> Result<PathBuf, ConfigError> {
    Ok(cadx_home()?.join(PREFERENCES_FILE_NAME))
}

/// Returns the default native-project path under CADX's working directory.
pub fn default_project_path() -> Result<PathBuf, ConfigError> {
    Ok(cadx_home()
        .map(|directory| directory.join(PROJECTS_DIRECTORY_NAME))?
        .join(DEFAULT_PROJECT_FILE_NAME))
}

/// Creates and validates the private CADX working and project directories.
pub fn ensure_default_working_directory() -> Result<PathBuf, ConfigError> {
    let home = cadx_home()?;
    ensure_working_directory_at(&home)?;
    Ok(home)
}

/// Initializes a non-secret provider configuration template if it does not exist.
///
/// The template contains an empty `api_key`. Users must provide their credential
/// themselves; this function never reads environment variables or writes keys.
pub fn initialize_default_config_if_missing() -> Result<PathBuf, ConfigError> {
    let home = cadx_home()?;
    initialize_config_at(&home)
}

/// Initializes both the provider template and the independent egress policy.
pub fn initialize_default_files_if_missing() -> Result<(), ConfigError> {
    let home = cadx_home()?;
    initialize_config_at(&home)?;
    initialize_egress_policy_at(&home)?;
    Ok(())
}

pub(crate) fn initialize_config_at(home: &Path) -> Result<PathBuf, ConfigError> {
    ensure_working_directory_at(home)?;
    let path = home.join(CONFIG_FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            validate_config_metadata(&path, &metadata)?;
            Ok(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match write_default_config(&path) {
                Ok(()) => Ok(path),
                Err(error) if is_already_exists(&error) => {
                    let metadata = fs::symlink_metadata(&path)
                        .map_err(|error| ConfigError::io(&path, error))?;
                    validate_config_metadata(&path, &metadata)?;
                    Ok(path)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(ConfigError::io(path, error)),
    }
}

pub(crate) fn initialize_egress_policy_at(home: &Path) -> Result<PathBuf, ConfigError> {
    ensure_working_directory_at(home)?;
    let path = home.join(EGRESS_POLICY_FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            validate_config_metadata(&path, &metadata)?;
            Ok(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match write_private_file(&path, DEFAULT_EGRESS_POLICY.as_bytes()) {
                Ok(()) => Ok(path),
                Err(error) if is_already_exists(&error) => {
                    let metadata = fs::symlink_metadata(&path)
                        .map_err(|error| ConfigError::io(&path, error))?;
                    validate_config_metadata(&path, &metadata)?;
                    Ok(path)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(ConfigError::io(path, error)),
    }
}

fn ensure_working_directory_at(home: &Path) -> Result<(), ConfigError> {
    ensure_private_directory(home)?;
    ensure_private_directory(&home.join(PROJECTS_DIRECTORY_NAME))
}

fn is_already_exists(error: &ConfigError) -> bool {
    matches!(
        error,
        ConfigError::Io { source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists
    )
}

pub(crate) fn ensure_private_directory(path: &Path) -> Result<(), ConfigError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_directory_metadata(path, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {
                set_private_directory_permissions(path)?;
                Ok(())
            }
            Err(create_error) if create_error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata =
                    fs::symlink_metadata(path).map_err(|error| ConfigError::io(path, error))?;
                validate_directory_metadata(path, &metadata)
            }
            Err(create_error) => Err(ConfigError::io(path, create_error)),
        },
        Err(error) => Err(ConfigError::io(path, error)),
    }
}

pub(crate) fn validate_private_config_file(path: &Path) -> Result<(), ConfigError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| ConfigError::io(path, error))?;
    validate_config_metadata(path, &metadata)
}

fn write_default_config(path: &Path) -> Result<(), ConfigError> {
    write_private_file(path, DEFAULT_CONFIG.as_bytes())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), ConfigError> {
    let mut file = private_create_new(path)?;
    let write_result = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(path);
        return Err(ConfigError::io(path, error));
    }
    Ok(())
}

/// Opens a private config file while checking that a path replacement did not
/// occur between the metadata check and the actual open.
pub(crate) fn open_private_config_file(path: &Path) -> Result<(File, fs::Metadata), ConfigError> {
    let expected = fs::symlink_metadata(path).map_err(|error| ConfigError::io(path, error))?;
    validate_config_metadata(path, &expected)?;
    let file = File::open(path).map_err(|error| ConfigError::io(path, error))?;
    let opened = file
        .metadata()
        .map_err(|error| ConfigError::io(path, error))?;
    validate_config_metadata(path, &opened)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if expected.dev() != opened.dev() || expected.ino() != opened.ino() {
            return Err(ConfigError::PathReplaced(path.into()));
        }
    }
    Ok((file, opened))
}

#[cfg(unix)]
pub(crate) fn private_create_new(path: &Path) -> Result<File, ConfigError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| ConfigError::io(path, error))
}

#[cfg(not(unix))]
pub(crate) fn private_create_new(path: &Path) -> Result<File, ConfigError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| ConfigError::io(path, error))
}

fn validate_directory_metadata(path: &Path, metadata: &fs::Metadata) -> Result<(), ConfigError> {
    if metadata.file_type().is_symlink() {
        return Err(ConfigError::PathIsSymlink(path.into()));
    }
    if !metadata.is_dir() {
        return Err(ConfigError::PathIsNotDirectory(path.into()));
    }
    validate_private_permissions(path, metadata)
}

fn validate_config_metadata(path: &Path, metadata: &fs::Metadata) -> Result<(), ConfigError> {
    if metadata.file_type().is_symlink() {
        return Err(ConfigError::PathIsSymlink(path.into()));
    }
    if !metadata.is_file() {
        return Err(ConfigError::PathIsNotFile(path.into()));
    }
    validate_private_permissions(path, metadata)
}

#[cfg(unix)]
fn validate_private_permissions(path: &Path, metadata: &fs::Metadata) -> Result<(), ConfigError> {
    use std::os::unix::fs::MetadataExt;

    if metadata.mode() & 0o077 != 0 {
        return Err(ConfigError::InsecurePermissions(path.into()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_permissions(_path: &Path, _metadata: &fs::Metadata) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| ConfigError::io(path, error))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn platform_home_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(target_os = "windows"))]
fn platform_home_directory() -> Option<PathBuf> {
    None
}
