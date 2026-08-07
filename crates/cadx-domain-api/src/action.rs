//! Business actions, tool requests, artifacts, and execution results.

use crate::{DomainContext, DomainId, DomainIssue, DomainParameters};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainAction {
    CreateSolidBox {
        name: String,
        size_mm: [f64; 3],
        position_mm: [f64; 3],
    },
    CreateSolidCylinder {
        name: String,
        radius_mm: f64,
        height_mm: f64,
        position_mm: [f64; 3],
    },
    CreateProfileExtrusion {
        name: String,
        profile_mm: Vec<[f64; 2]>,
        height_mm: f64,
        position_mm: [f64; 3],
    },
    CreatePcbBoard {
        name: String,
        width_mm: f64,
        height_mm: f64,
        thickness_mm: f64,
        layers: u16,
    },
    PlacePcbComponent {
        reference: String,
        value: String,
        footprint: String,
        position_mm: [f64; 2],
        rotation_deg: f64,
        side: String,
        model_3d: Option<String>,
    },
    UpsertDomainMetadata {
        entity_key: String,
        namespace: String,
        values: DomainParameters,
    },
    RunCheck {
        check: String,
    },
    GenerateBom,
    Export {
        format: String,
    },
    OpenPanel {
        panel: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainToolRequest {
    pub tool_id: String,
    #[serde(default)]
    pub parameters: DomainParameters,
    pub context: DomainContext,
}

impl DomainToolRequest {
    #[must_use]
    pub fn new(tool_id: impl Into<String>, context: DomainContext) -> Self {
        Self {
            tool_id: tool_id.into(),
            parameters: DomainParameters::new(),
            context,
        }
    }

    #[must_use]
    pub fn with_parameters(mut self, parameters: DomainParameters) -> Self {
        self.parameters = parameters;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainArtifactKind {
    Report,
    Drawing,
    Bom,
    Schedule,
    Manufacturing,
    Exchange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainArtifact {
    pub name: String,
    pub media_type: String,
    pub kind: DomainArtifactKind,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DomainExecution {
    pub summary: String,
    #[serde(default)]
    pub actions: Vec<DomainAction>,
    #[serde(default)]
    pub issues: Vec<DomainIssue>,
    #[serde(default)]
    pub artifacts: Vec<DomainArtifact>,
}

impl DomainExecution {
    #[must_use]
    pub fn with_action(summary: impl Into<String>, action: DomainAction) -> Self {
        Self {
            summary: summary.into(),
            actions: vec![action],
            issues: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainExecutionError {
    #[error("domain pack {0:?} is not registered")]
    PackNotRegistered(DomainId),
    #[error("domain pack {0:?} is disabled")]
    PackDisabled(DomainId),
    #[error("tool {tool_id} is not registered by {domain:?}")]
    UnknownTool { domain: DomainId, tool_id: String },
    #[error("tool parameters are invalid")]
    InvalidParameters(Vec<DomainIssue>),
    #[error("domain tool failed: {0}")]
    ToolFailed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainRoute {
    pub action: DomainAction,
    pub confidence: f32,
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Step,
    Stl,
    Ifc,
    Gerber,
    Drill,
    Drawing,
    Bom,
}
