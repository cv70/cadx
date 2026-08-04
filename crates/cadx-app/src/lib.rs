//! Application use cases for CADX.
//!
//! This layer coordinates domain commands and kernel evaluation without taking
//! dependencies on egui, filesystem dialogs, concrete CAD kernels, or AI
//! providers. A transaction becomes visible only after both domain validation
//! and kernel evaluation succeed.

mod bus;
mod error;
mod import;
mod session;

pub use bus::{
    CommandStream, CoreBus, CoreBusError, CoreEvent, DEFAULT_EVENT_LOG_LIMIT, EventDispatcher,
    StreamId, TransactionId, TransactionMetadata, TransactionSource,
};
pub use error::SessionError;
pub use import::{StepImportPlan, StepImportPlanError, plan_step_import};
pub use session::{
    DEFAULT_HISTORY_LIMIT, DocumentDiff, DocumentSession, DocumentState, FeatureChange,
    TransactionOutcome, TransactionPreview,
};
