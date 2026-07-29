//! Versioned, local `.cadx` project persistence.
//!
//! A project is a small ZIP archive containing a manifest and one lossless
//! workspace payload. The archive is never extracted to disk while loading,
//! which avoids path traversal and lets the loader enforce strict entry and
//! size limits before deserializing untrusted data.

mod archive;
mod dxf;
mod error;
mod pdf;
mod project;

#[cfg(test)]
mod tests;

pub use dxf::{
    DXF_EXTENSION, DxfExchangeError, DxfExportReport, DxfImportPlan, DxfImportReport,
    MAX_DXF_BYTES, MAX_DXF_ENTITIES, MAX_DXF_LAYERS, MAX_DXF_VERTICES, export_dxf, plan_dxf_import,
};
pub use error::ProjectError;
pub use pdf::{
    MAX_PDF_BYTES, MAX_PDF_ENTITIES, MAX_PDF_PATH_SEGMENTS, MAX_PDF_TEXT_BYTES, PDF_EXTENSION,
    PdfExportError, PdfExportOptions, PdfExportReport, PdfOrientation, PdfPageSize, export_pdf,
};
pub use project::{
    CURRENT_PROJECT_FORMAT_VERSION, PROJECT_EXTENSION, ProjectLoad, ProjectManifest,
    ProjectSaveReport, RECOVERY_SUFFIX, discard_recovery, load_recovery, load_workspace,
    recovery_exists, recovery_path, save_recovery, save_workspace,
};
