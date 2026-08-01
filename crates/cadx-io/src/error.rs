use std::path::PathBuf;

use thiserror::Error;

use crate::atomic::AtomicWriteError;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("cannot export an empty scene")]
    EmptyScene,
    #[error("scene contains too many triangles for binary STL")]
    TooManyTriangles,
    #[error("feature {feature_id} has invalid mesh: {message}")]
    InvalidMesh { feature_id: u64, message: String },
    #[error("feature {feature_id} triangle {triangle} is invalid: {message}")]
    InvalidTriangle {
        feature_id: u64,
        triangle: usize,
        message: String,
    },
    #[error("feature {feature_id} has invalid color: {message}")]
    InvalidColor { feature_id: u64, message: String },
    #[error("invalid STEP exchange data: {0}")]
    InvalidStep(String),
    #[error("invalid 3MF exchange data: {0}")]
    InvalidThreeMf(String),
    #[error("could not write export file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl From<AtomicWriteError> for ExportError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Io {
            path: error.path,
            source: error.source,
        }
    }
}
