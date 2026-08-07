//! The stable, geometry-neutral plugin protocol shared by CADX domain packs.
//!
//! This crate must stay independent of the CAD kernel, document implementation,
//! and egui. Domain packs receive read-only context and return business
//! actions; the host decides how to translate geometry actions into a checked
//! core transaction.

mod action;
mod context;
mod extension;
mod issue;
mod manifest;
mod pack;
mod registry;
mod schema;

pub use action::{
    DomainAction, DomainArtifact, DomainArtifactKind, DomainExecution, DomainExecutionError,
    DomainRoute, DomainToolRequest, ExportFormat,
};
pub use context::{DomainContext, DomainSpatialEntity};
pub use extension::{
    DomainAiTool, DomainShader, DomainShaderStage, DomainSolver, DomainSolverStage,
};
pub use issue::{DomainIssue, DomainIssueSeverity};
pub use manifest::{DomainId, DomainManifest, DomainTool};
pub use pack::DomainPack;
pub use registry::DomainRegistry;
pub use schema::{
    DomainFieldKind, DomainFieldSchema, DomainFieldValue, DomainInspectorSchema, DomainPanelSchema,
    DomainParameters, DomainSelectOption,
};
