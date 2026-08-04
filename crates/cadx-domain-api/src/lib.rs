//! The stable, geometry-neutral plugin protocol shared by CADX domain packs.
//!
//! This crate must stay independent of the CAD kernel, document implementation,
//! and egui. Domain packs receive read-only context and return business
//! actions; the host decides how to translate geometry actions into a checked
//! core transaction.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DomainId {
    Mcad,
    Aec,
    Ecad,
}

impl DomainId {
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Mcad => "mcad",
            Self::Aec => "aec",
            Self::Ecad => "ecad",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainManifest {
    pub id: DomainId,
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub priority: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainTool {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub category: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainFieldKind {
    Text,
    Integer,
    Decimal,
    LengthMm,
    AngleDeg,
    Boolean,
    Select,
    EntityReference,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainSelectOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DomainFieldSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: DomainFieldKind,
    pub default_value: Option<&'static str>,
    pub unit: Option<&'static str>,
    pub options: &'static [DomainSelectOption],
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DomainPanelSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub fields: &'static [DomainFieldSchema],
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct DomainInspectorSchema {
    pub panels: &'static [DomainPanelSchema],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DomainFieldValue {
    Text(String),
    Integer(i64),
    Decimal(f64),
    Boolean(bool),
    EntityReference(u64),
    Color(String),
}

impl DomainFieldValue {
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) | Self::Color(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_decimal(&self) -> Option<f64> {
        match self {
            Self::Decimal(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_boolean(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_entity_reference(&self) -> Option<u64> {
        match self {
            Self::EntityReference(value) => Some(*value),
            _ => None,
        }
    }
}

pub type DomainParameters = BTreeMap<String, DomainFieldValue>;

impl DomainPanelSchema {
    #[must_use]
    pub fn field(&self, id: &str) -> Option<&DomainFieldSchema> {
        self.fields.iter().find(|field| field.id == id)
    }

    /// Merges schema defaults into supplied values and validates all values.
    ///
    /// # Errors
    ///
    /// Returns one or more field-level issues when a required value is absent,
    /// a value has the wrong type, or a select value is not declared.
    pub fn resolve_parameters(
        &self,
        supplied: &DomainParameters,
    ) -> Result<DomainParameters, Vec<DomainIssue>> {
        let mut resolved = supplied.clone();
        let mut issues = Vec::new();

        for field in self.fields {
            if !resolved.contains_key(field.id) {
                if let Some(default) = field.default_value {
                    match parse_default(field.kind, default) {
                        Ok(value) => {
                            resolved.insert(field.id.into(), value);
                        }
                        Err(message) => issues.push(parameter_issue(field.id, message)),
                    }
                } else if field.required {
                    issues.push(parameter_issue(field.id, "required value is missing"));
                }
            }

            if let Some(value) = resolved.get(field.id)
                && let Err(message) = validate_field_value(field, value)
            {
                issues.push(parameter_issue(field.id, message));
            }
        }

        for id in supplied.keys() {
            if self.field(id).is_none() {
                issues.push(parameter_issue(id, "field is not declared by this panel"));
            }
        }

        if issues.is_empty() {
            Ok(resolved)
        } else {
            Err(issues)
        }
    }
}

fn parse_default(kind: DomainFieldKind, value: &str) -> Result<DomainFieldValue, &'static str> {
    match kind {
        DomainFieldKind::Text | DomainFieldKind::Select => Ok(DomainFieldValue::Text(value.into())),
        DomainFieldKind::Integer => value
            .parse()
            .map(DomainFieldValue::Integer)
            .map_err(|_| "schema default is not an integer"),
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(DomainFieldValue::Decimal)
            .ok_or("schema default is not a finite number"),
        DomainFieldKind::Boolean => value
            .parse()
            .map(DomainFieldValue::Boolean)
            .map_err(|_| "schema default is not a boolean"),
        DomainFieldKind::EntityReference => value
            .parse()
            .map(DomainFieldValue::EntityReference)
            .map_err(|_| "schema default is not an entity reference"),
        DomainFieldKind::Color => Ok(DomainFieldValue::Color(value.into())),
    }
}

fn validate_field_value(
    field: &DomainFieldSchema,
    value: &DomainFieldValue,
) -> Result<(), &'static str> {
    let valid_type = match field.kind {
        DomainFieldKind::Text | DomainFieldKind::Select => value.as_text().is_some(),
        DomainFieldKind::Integer => value.as_integer().is_some(),
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => {
            value.as_decimal().is_some_and(f64::is_finite)
        }
        DomainFieldKind::Boolean => value.as_boolean().is_some(),
        DomainFieldKind::EntityReference => value.as_entity_reference().is_some(),
        DomainFieldKind::Color => matches!(value, DomainFieldValue::Color(_)),
    };
    if !valid_type {
        return Err("value type does not match the schema");
    }
    if field.kind == DomainFieldKind::Select
        && !field
            .options
            .iter()
            .any(|option| value.as_text() == Some(option.value))
    {
        return Err("value is not one of the declared options");
    }
    Ok(())
}

fn parameter_issue(field: &str, message: impl Into<String>) -> DomainIssue {
    DomainIssue {
        code: format!("INVALID_PARAMETER.{field}"),
        severity: DomainIssueSeverity::Error,
        message: message.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainSolverStage {
    Modeling,
    Constraint,
    Analysis,
    Routing,
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainSolver {
    pub id: &'static str,
    pub label: &'static str,
    pub stage: DomainSolverStage,
    pub description: &'static str,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainShaderStage {
    Render,
    Overlay,
    Compute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainShader {
    pub id: &'static str,
    pub label: &'static str,
    pub stage: DomainShaderStage,
    pub entry_point: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainAiTool {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub schema_id: &'static str,
    /// Stable [`DomainTool::id`] executed after the model selects this tool.
    pub executable_tool_id: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DomainContext {
    pub document_name: String,
    pub selected_feature_ids: Vec<u64>,
    pub visible_solid_count: usize,
    pub active_feature_count: usize,
    pub selected_feature_name: Option<String>,
    #[serde(default)]
    pub spatial_entities: Vec<DomainSpatialEntity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainSpatialEntity {
    pub feature_id: u64,
    pub name: String,
    pub minimum_mm: [f64; 3],
    pub maximum_mm: [f64; 3],
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainIssueSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainIssue {
    pub code: String,
    pub severity: DomainIssueSeverity,
    pub message: String,
}

/// A domain pack is a pure business plugin. Implementations may parse natural
/// language and perform domain checks, but they never receive kernel objects.
pub trait DomainPack: Send + Sync {
    fn manifest(&self) -> DomainManifest;
    fn tools(&self) -> &'static [DomainTool];
    fn inspector_schema(&self) -> DomainInspectorSchema {
        DomainInspectorSchema::default()
    }
    fn tool_panel(&self, _tool_id: &str) -> Option<DomainPanelSchema> {
        None
    }
    fn solvers(&self) -> &'static [DomainSolver] {
        &[]
    }
    fn shaders(&self) -> &'static [DomainShader] {
        &[]
    }
    fn ai_tools(&self) -> &'static [DomainAiTool] {
        &[]
    }
    /// Executes a registered domain tool without accessing host or kernel
    /// state. Packs return business actions; the host owns their transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DomainExecutionError::UnknownTool`] when the tool is not part
    /// of this pack. Implementations may report parameter or domain failures.
    fn execute_tool(
        &self,
        request: &DomainToolRequest,
    ) -> Result<DomainExecution, DomainExecutionError> {
        let manifest = self.manifest();
        if !self.tools().iter().any(|tool| tool.id == request.tool_id) {
            return Err(DomainExecutionError::UnknownTool {
                domain: manifest.id,
                tool_id: request.tool_id.clone(),
            });
        }
        Ok(DomainExecution::with_action(
            format!("Open {}", request.tool_id),
            DomainAction::OpenPanel {
                panel: request.tool_id.clone(),
            },
        ))
    }
    fn route_natural_language(&self, input: &str, context: &DomainContext) -> DomainRoute;
    fn validate_export(&self, format: ExportFormat, context: &DomainContext) -> Vec<DomainIssue>;
}

/// Compile-time registered, runtime-filterable domain pack bus.
///
/// The bus stores only the small [`DomainPack`] SPI. It never knows about a
/// geometry kernel, document entity, renderer, or domain implementation type.
#[derive(Default)]
pub struct DomainRegistry {
    packs: Vec<Arc<dyn DomainPack>>,
    enabled: BTreeSet<DomainId>,
}

impl DomainRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, pack: Arc<dyn DomainPack>) {
        let id = pack.manifest().id;
        if self
            .packs
            .iter()
            .all(|candidate| candidate.manifest().id != id)
        {
            self.packs.push(pack);
            self.enabled.insert(id);
        }
    }

    pub fn set_enabled(&mut self, id: DomainId, enabled: bool) {
        if self.packs.iter().any(|pack| pack.manifest().id == id) {
            if enabled {
                self.enabled.insert(id);
            } else {
                self.enabled.remove(&id);
            }
        }
    }

    #[must_use]
    pub fn is_enabled(&self, id: DomainId) -> bool {
        self.enabled.contains(&id)
    }

    #[must_use]
    pub fn enabled_packs(&self) -> Vec<Arc<dyn DomainPack>> {
        self.packs
            .iter()
            .filter(|pack| self.enabled.contains(&pack.manifest().id))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn manifests(&self) -> Vec<DomainManifest> {
        self.packs
            .iter()
            .filter(|pack| self.enabled.contains(&pack.manifest().id))
            .map(|pack| pack.manifest())
            .collect()
    }

    #[must_use]
    pub fn registered_manifests(&self) -> Vec<DomainManifest> {
        self.packs.iter().map(|pack| pack.manifest()).collect()
    }

    #[must_use]
    pub fn pack(&self, id: DomainId) -> Option<Arc<dyn DomainPack>> {
        self.packs
            .iter()
            .find(|pack| pack.manifest().id == id)
            .cloned()
    }

    #[must_use]
    pub fn inspector_schema(&self, id: DomainId) -> Option<DomainInspectorSchema> {
        self.pack(id).map(|pack| pack.inspector_schema())
    }

    #[must_use]
    pub fn solvers(&self, id: DomainId) -> Vec<DomainSolver> {
        self.pack(id)
            .map_or_else(Vec::new, |pack| pack.solvers().to_vec())
    }

    #[must_use]
    pub fn shaders(&self, id: DomainId) -> Vec<DomainShader> {
        self.pack(id)
            .map_or_else(Vec::new, |pack| pack.shaders().to_vec())
    }

    #[must_use]
    pub fn ai_tools(&self, id: DomainId) -> Vec<DomainAiTool> {
        self.pack(id)
            .map_or_else(Vec::new, |pack| pack.ai_tools().to_vec())
    }

    /// Executes a tool through the enabled pack boundary.
    ///
    /// # Errors
    ///
    /// Returns a registration, enablement, parameter, or pack execution error.
    pub fn execute(
        &self,
        id: DomainId,
        request: &DomainToolRequest,
    ) -> Result<DomainExecution, DomainExecutionError> {
        let pack = self
            .pack(id)
            .ok_or(DomainExecutionError::PackNotRegistered(id))?;
        if !self.is_enabled(id) {
            return Err(DomainExecutionError::PackDisabled(id));
        }
        pack.execute_tool(request)
    }

    #[must_use]
    pub fn route(&self, input: &str, context: &DomainContext) -> Option<(DomainId, DomainRoute)> {
        self.enabled_packs()
            .into_iter()
            .map(|pack| {
                let manifest = pack.manifest();
                (
                    manifest.id,
                    manifest.priority,
                    pack.route_natural_language(input, context),
                )
            })
            .max_by(|(_, first_priority, first), (_, second_priority, second)| {
                first
                    .confidence
                    .total_cmp(&second.confidence)
                    .then_with(|| first_priority.cmp(second_priority))
            })
            .map(|(id, _, route)| (id, route))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPack(DomainId);

    impl DomainPack for TestPack {
        fn manifest(&self) -> DomainManifest {
            DomainManifest {
                id: self.0,
                name: "test",
                version: "0.1",
                description: "test",
                priority: 0,
            }
        }

        fn tools(&self) -> &'static [DomainTool] {
            &[]
        }

        fn route_natural_language(&self, _input: &str, _context: &DomainContext) -> DomainRoute {
            DomainRoute {
                action: DomainAction::GenerateBom,
                confidence: 0.5,
                rationale: "test".into(),
            }
        }

        fn validate_export(
            &self,
            _format: ExportFormat,
            _context: &DomainContext,
        ) -> Vec<DomainIssue> {
            Vec::new()
        }
    }

    #[test]
    fn packs_can_be_disabled_without_unregistering_them() {
        let mut registry = DomainRegistry::new();
        registry.register(Arc::new(TestPack(DomainId::Mcad)));
        registry.register(Arc::new(TestPack(DomainId::Mcad)));
        assert_eq!(registry.manifests().len(), 1);
        registry.set_enabled(DomainId::Mcad, false);
        assert!(registry.manifests().is_empty());
    }

    #[test]
    fn registry_exposes_pack_pipeline_descriptors() {
        static SOLVERS: [DomainSolver; 1] = [DomainSolver {
            id: "solver",
            label: "Solver",
            stage: DomainSolverStage::Modeling,
            description: "test",
            inputs: &["input"],
            outputs: &["output"],
        }];

        struct DescribedPack;

        impl DomainPack for DescribedPack {
            fn manifest(&self) -> DomainManifest {
                DomainManifest {
                    id: DomainId::Aec,
                    name: "described",
                    version: "0.1",
                    description: "test",
                    priority: 0,
                }
            }

            fn tools(&self) -> &'static [DomainTool] {
                &[]
            }

            fn solvers(&self) -> &'static [DomainSolver] {
                &SOLVERS
            }

            fn route_natural_language(
                &self,
                _input: &str,
                _context: &DomainContext,
            ) -> DomainRoute {
                DomainRoute {
                    action: DomainAction::OpenPanel {
                        panel: "test".into(),
                    },
                    confidence: 1.0,
                    rationale: "test".into(),
                }
            }

            fn validate_export(
                &self,
                _format: ExportFormat,
                _context: &DomainContext,
            ) -> Vec<DomainIssue> {
                Vec::new()
            }
        }

        let mut registry = DomainRegistry::new();
        registry.register(Arc::new(DescribedPack));
        assert_eq!(registry.solvers(DomainId::Aec)[0].id, "solver");
    }

    #[test]
    fn panel_resolves_defaults_and_rejects_unknown_select_values() {
        static OPTIONS: [DomainSelectOption; 2] = [
            DomainSelectOption {
                value: "first",
                label: "First",
            },
            DomainSelectOption {
                value: "second",
                label: "Second",
            },
        ];
        static FIELDS: [DomainFieldSchema; 2] = [
            DomainFieldSchema {
                id: "size",
                label: "Size",
                kind: DomainFieldKind::LengthMm,
                default_value: Some("12.5"),
                unit: Some("mm"),
                options: &[],
                required: true,
            },
            DomainFieldSchema {
                id: "mode",
                label: "Mode",
                kind: DomainFieldKind::Select,
                default_value: Some("first"),
                unit: None,
                options: &OPTIONS,
                required: true,
            },
        ];
        let panel = DomainPanelSchema {
            id: "test",
            label: "Test",
            fields: &FIELDS,
        };
        let defaults = panel.resolve_parameters(&DomainParameters::new()).unwrap();
        assert_eq!(defaults["size"].as_decimal(), Some(12.5));
        assert_eq!(defaults["mode"].as_text(), Some("first"));

        let invalid = [("mode".into(), DomainFieldValue::Text("third".into()))]
            .into_iter()
            .collect();
        assert!(panel.resolve_parameters(&invalid).is_err());
    }

    #[test]
    fn registry_blocks_execution_when_pack_is_disabled() {
        let mut registry = DomainRegistry::new();
        registry.register(Arc::new(TestPack(DomainId::Mcad)));
        registry.set_enabled(DomainId::Mcad, false);
        let error = registry
            .execute(
                DomainId::Mcad,
                &DomainToolRequest::new("missing", DomainContext::default()),
            )
            .unwrap_err();
        assert_eq!(error, DomainExecutionError::PackDisabled(DomainId::Mcad));
    }
}
