use std::fmt;
use std::path::PathBuf;

/// Errors raised while resolving or validating local CADX configuration.
#[derive(Debug)]
pub enum ConfigError {
    HomeDirectoryUnavailable,
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    PathIsSymlink(PathBuf),
    PathIsNotDirectory(PathBuf),
    PathIsNotFile(PathBuf),
    PathReplaced(PathBuf),
    InsecurePermissions(PathBuf),
    ConfigTooLarge {
        path: PathBuf,
        limit: u64,
    },
    InvalidYaml(PathBuf),
    UnsupportedVersion(u32),
    InvalidProvider(&'static str),
    InvalidEgressPolicy(&'static str),
    ProviderEgressDenied {
        endpoint: String,
        model: String,
    },
}

impl ConfigError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeDirectoryUnavailable => {
                formatter.write_str("could not determine the current user's home directory")
            }
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "cannot access CADX configuration path {}: {source}",
                    path.display()
                )
            }
            Self::PathIsSymlink(path) => {
                write!(
                    formatter,
                    "CADX configuration path must not be a symbolic link: {}",
                    path.display()
                )
            }
            Self::PathIsNotDirectory(path) => {
                write!(
                    formatter,
                    "CADX working directory is not a directory: {}",
                    path.display()
                )
            }
            Self::PathIsNotFile(path) => {
                write!(
                    formatter,
                    "CADX configuration path is not a regular file: {}",
                    path.display()
                )
            }
            Self::PathReplaced(path) => write!(
                formatter,
                "CADX configuration path changed while it was being opened: {}",
                path.display()
            ),
            Self::InsecurePermissions(path) => write!(
                formatter,
                "CADX configuration path has permissions visible to other users: {}",
                path.display()
            ),
            Self::ConfigTooLarge { path, limit } => write!(
                formatter,
                "CADX configuration file exceeds the {limit}-byte limit: {}",
                path.display()
            ),
            Self::InvalidYaml(path) => {
                write!(
                    formatter,
                    "CADX configuration contains invalid YAML: {}",
                    path.display()
                )
            }
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported CADX configuration version {version}"
                )
            }
            Self::InvalidProvider(message) => formatter.write_str(message),
            Self::InvalidEgressPolicy(message) => formatter.write_str(message),
            Self::ProviderEgressDenied { endpoint, model } => write!(
                formatter,
                "provider endpoint/model is not approved by the local egress policy: {endpoint} / {model}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
