//! STEP output: physical-file writing.

use std::path::Path;

use crate::{ExportError, atomic::write_atomic};

use super::validate_step;

/// Validates and atomically writes a STEP physical file.
///
/// # Errors
///
/// Returns [`ExportError`] when parsing or file output fails.
pub fn write_step(source: &str, path: impl AsRef<Path>) -> Result<(), ExportError> {
    validate_step(source)?;
    write_atomic(path.as_ref(), source.as_bytes()).map_err(ExportError::from)
}
