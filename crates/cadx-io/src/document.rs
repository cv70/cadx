use std::{fs, path::PathBuf};

use cadx_core::{
    domain::CadDocument,
    persistence::{self, PersistenceError},
};
use thiserror::Error;

use crate::{atomic::AtomicWriteError, atomic::write_atomic};

pub const DOCUMENT_EXTENSION: &str = "cadx";

#[derive(Debug, Error)]
pub enum DocumentFileError {
    #[error("CADX document content is invalid: {0}")]
    Content(#[from] PersistenceError),
    #[error("could not access document file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl From<AtomicWriteError> for DocumentFileError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Io {
            path: error.path,
            source: error.source,
        }
    }
}

/// Reads, decodes, and validates a CADX document file.
///
/// # Errors
///
/// Returns [`DocumentFileError`] when file access or content validation fails.
pub fn load_document(path: impl Into<PathBuf>) -> Result<CadDocument, DocumentFileError> {
    let path = path.into();
    let source = fs::read_to_string(&path).map_err(|source| DocumentFileError::Io {
        path: path.clone(),
        source,
    })?;
    persistence::decode(&source).map_err(DocumentFileError::from)
}

/// Encodes and atomically writes a validated CADX document file.
///
/// # Errors
///
/// Returns [`DocumentFileError`] when encoding or durable file output fails.
pub fn save_document(
    document: &CadDocument,
    path: impl Into<PathBuf>,
) -> Result<(), DocumentFileError> {
    let path = path.into();
    let source = persistence::encode(document)?;
    write_atomic(&path, source.as_bytes()).map_err(DocumentFileError::from)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn file_round_trip_preserves_document() {
        let path = std::env::temp_dir().join(format!(
            "cadx-document-test-{}-{}.cadx",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let document = CadDocument::demo();
        save_document(&document, path.clone()).unwrap();
        assert_eq!(load_document(path.clone()).unwrap(), document);
        fs::remove_file(path).unwrap();
    }
}
