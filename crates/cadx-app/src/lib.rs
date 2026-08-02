//! Application use cases for CADX.
//!
//! This layer coordinates domain commands and kernel evaluation without taking
//! dependencies on egui, filesystem dialogs, concrete CAD kernels, or AI
//! providers. A transaction becomes visible only after both domain validation
//! and kernel evaluation succeed.

mod error;
mod import;
mod session;

pub use error::SessionError;
pub use import::{StepImportPlan, StepImportPlanError, plan_step_import};
pub use session::{DEFAULT_HISTORY_LIMIT, DocumentSession, DocumentState, TransactionOutcome};
