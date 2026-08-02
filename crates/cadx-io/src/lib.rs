//! Validated CAD and manufacturing exchange formats for CADX.
//!
//! This crate is the exchange boundary between kernel-neutral application data
//! and external files. Format adapters validate before committing bytes, while
//! kernel-native B-Rep data remains behind
//! [`ExchangeKernel`](cadx_core::kernel::ExchangeKernel).

mod atomic;
mod document;
mod error;
mod step;
mod stl;
mod threemf;

pub use document::{DOCUMENT_EXTENSION, DocumentFileError, load_document, save_document};
pub use error::ExportError;
pub use step::{
    StepBodyColor, StepImport, StepImportAssembly, StepImportBody, StepImportOccurrence,
    parse_step, read_step, validate_step, write_step,
};
pub use stl::{encode_binary_stl, write_binary_stl};
pub use threemf::{encode_3mf, validate_3mf, write_3mf};
