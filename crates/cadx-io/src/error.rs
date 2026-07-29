use std::fmt;
use std::path::PathBuf;

use cadx_core::WorkspaceError;

#[derive(Debug)]
pub enum ProjectError {
    Io(std::io::Error),
    Archive(zip::result::ZipError),
    Serialization(serde_json::Error),
    Workspace(WorkspaceError),
    UnsupportedFormatVersion(u32),
    InvalidManifest(String),
    InvalidArchive(String),
    MissingEntry(&'static str),
    UnexpectedEntry(String),
    DuplicateEntry(String),
    EntryTooLarge { entry: String, limit: u64 },
    IntegrityMismatch { expected: u32, actual: u32 },
    InvalidPath(PathBuf),
}

impl From<std::io::Error> for ProjectError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<zip::result::ZipError> for ProjectError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Archive(error)
    }
}

impl From<serde_json::Error> for ProjectError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

impl From<WorkspaceError> for ProjectError {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Archive(error) => error.fmt(formatter),
            Self::Serialization(error) => write!(formatter, "invalid project JSON: {error}"),
            Self::Workspace(error) => write!(formatter, "invalid project workspace: {error}"),
            Self::UnsupportedFormatVersion(version) => write!(
                formatter,
                "project format version {version} is newer than this CADX build"
            ),
            Self::InvalidManifest(message) | Self::InvalidArchive(message) => {
                formatter.write_str(message)
            }
            Self::MissingEntry(entry) => write!(formatter, "project archive is missing {entry}"),
            Self::UnexpectedEntry(entry) => {
                write!(formatter, "unexpected project archive entry {entry}")
            }
            Self::DuplicateEntry(entry) => {
                write!(formatter, "duplicate project archive entry {entry}")
            }
            Self::EntryTooLarge { entry, limit } => {
                write!(
                    formatter,
                    "project archive entry {entry} exceeds {limit} bytes"
                )
            }
            Self::IntegrityMismatch { expected, actual } => write!(
                formatter,
                "project workspace checksum mismatch: expected {expected:08x}, got {actual:08x}"
            ),
            Self::InvalidPath(path) => write!(formatter, "invalid project path {}", path.display()),
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Archive(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::Workspace(error) => Some(error),
            _ => None,
        }
    }
}
