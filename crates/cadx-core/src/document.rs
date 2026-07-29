use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::constraint::{SketchConstraint, validate_constraint};
use crate::expression::{ExpressionError, ParameterExpression, is_valid_parameter_name};
use crate::{CURRENT_SCHEMA_VERSION, ConstraintId, EntityId, LayerId, ParameterId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Units {
    #[default]
    Millimeters,
    Meters,
    Inches,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    pub color: [u8; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    Line {
        start: Point2,
        end: Point2,
    },
    Circle {
        center: Point2,
        radius: f64,
    },
    Arc {
        center: Point2,
        radius: f64,
        start_angle: f64,
        sweep_angle: f64,
    },
    AlignedDimension {
        start: Point2,
        end: Point2,
        /// Signed perpendicular distance from the measured segment to the
        /// dimension line. Positive offsets use the segment's left normal.
        offset: f64,
        /// Optional DXF-compatible text template. `<>` expands to the measured
        /// value; `None` uses the measured value without an override.
        text_override: Option<String>,
    },
    Rectangle {
        origin: Point2,
        width: f64,
        height: f64,
    },
    SketchProfile {
        points: Vec<Point2>,
        closed: bool,
    },
    Extrude {
        profile: EntityId,
        distance: f64,
    },
    Wall {
        start: Point2,
        end: Point2,
        thickness: f64,
    },
    Room {
        boundary: Vec<Point2>,
        area: f64,
    },
    Text {
        position: Point2,
        content: String,
    },
}

impl EntityKind {
    pub fn domain(&self) -> Domain {
        match self {
            Self::Line { .. }
            | Self::Circle { .. }
            | Self::Arc { .. }
            | Self::AlignedDimension { .. }
            | Self::Rectangle { .. }
            | Self::Text { .. } => Domain::Drafting,
            Self::SketchProfile { .. } | Self::Extrude { .. } => Domain::Mechanical,
            Self::Wall { .. } | Self::Room { .. } => Domain::Architecture,
        }
    }

    fn validate(&self, document: &CadDocument) -> Result<(), CommandError> {
        match self {
            Self::Line { start, end } | Self::Wall { start, end, .. }
                if !finite_point(*start) || !finite_point(*end) =>
            {
                Err(CommandError::InvalidGeometry(
                    "line endpoints must be finite".into(),
                ))
            }
            Self::Circle { center, radius } if !finite_point(*center) || !positive(*radius) => Err(
                CommandError::InvalidGeometry("circle radius must be positive".into()),
            ),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } if !finite_point(*center)
                || !positive(*radius)
                || !start_angle.is_finite()
                || !sweep_angle.is_finite()
                || *sweep_angle <= 0.0
                || *sweep_angle >= std::f64::consts::TAU =>
            {
                Err(CommandError::InvalidGeometry(
                    "arc center, radius, and angular span must be valid".into(),
                ))
            }
            Self::AlignedDimension {
                start,
                end,
                offset,
                text_override,
            } if !finite_point(*start)
                || !finite_point(*end)
                || !offset.is_finite()
                || *offset == 0.0
                || (end.x - start.x).hypot(end.y - start.y) <= f64::EPSILON
                || text_override.as_ref().is_some_and(|text| {
                    text.len() > 4_096 || text.chars().any(|character| character == '\0')
                }) =>
            {
                Err(CommandError::InvalidGeometry(
                    "aligned dimension points, offset, and text must be valid".into(),
                ))
            }
            Self::Rectangle {
                origin,
                width,
                height,
            } if !finite_point(*origin) || !positive(*width) || !positive(*height) => {
                Err(CommandError::InvalidGeometry(
                    "rectangle dimensions must be finite and positive".into(),
                ))
            }
            Self::SketchProfile { points, closed } if *closed && points.len() < 3 => Err(
                CommandError::InvalidGeometry("a closed sketch needs at least three points".into()),
            ),
            Self::SketchProfile { points, .. }
                if points.iter().any(|point| !finite_point(*point)) =>
            {
                Err(CommandError::InvalidGeometry(
                    "sketch points must be finite".into(),
                ))
            }
            Self::Extrude { profile, distance } if !positive(*distance) => Err(
                CommandError::InvalidGeometry("extrude distance must be positive".into()),
            ),
            Self::Extrude { profile, .. } => match document.entities.get(profile) {
                Some(Entity {
                    kind: EntityKind::SketchProfile { closed: true, .. },
                    ..
                }) => Ok(()),
                Some(_) => Err(CommandError::InvalidReference(
                    "an extrude requires a closed sketch profile".into(),
                )),
                None => Err(CommandError::EntityMissing(*profile)),
            },
            Self::Wall { thickness, .. } if !positive(*thickness) => Err(
                CommandError::InvalidGeometry("wall thickness must be positive".into()),
            ),
            Self::Room { boundary, area }
                if boundary.len() < 3
                    || !positive(*area)
                    || boundary.iter().any(|point| !finite_point(*point)) =>
            {
                Err(CommandError::InvalidGeometry(
                    "room boundary and area must be valid".into(),
                ))
            }
            Self::Text { position, content }
                if !finite_point(*position) || content.trim().is_empty() =>
            {
                Err(CommandError::InvalidGeometry(
                    "text needs a position and content".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Domain {
    Drafting,
    Mechanical,
    Architecture,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub layer: LayerId,
    pub name: String,
    pub visible: bool,
    pub kind: EntityKind,
    pub parameter_refs: BTreeSet<ParameterId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub id: ParameterId,
    pub name: String,
    pub value: f64,
    pub unit: Units,
    /// Optional formula evaluated in the document's units. `value` remains the
    /// editable literal used when no formula is present.
    #[serde(default)]
    pub expression: Option<ParameterExpression>,
}

impl Parameter {
    pub fn literal(id: ParameterId, name: impl Into<String>, value: f64, unit: Units) -> Self {
        Self {
            id,
            name: name.into(),
            value,
            unit,
            expression: None,
        }
    }

    pub fn formula(
        id: ParameterId,
        name: impl Into<String>,
        source: impl Into<String>,
        unit: Units,
    ) -> Result<Self, ExpressionError> {
        Ok(Self {
            id,
            name: name.into(),
            value: 0.0,
            unit,
            expression: Some(ParameterExpression::new(source)?),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CadDocument {
    pub schema_version: u32,
    pub metadata: DocumentMetadata,
    pub units: Units,
    pub layers: BTreeMap<LayerId, Layer>,
    pub entities: BTreeMap<EntityId, Entity>,
    pub parameters: BTreeMap<ParameterId, Parameter>,
    #[serde(default)]
    pub constraints: BTreeMap<ConstraintId, SketchConstraint>,
    #[serde(default = "default_next_layer_id")]
    pub(crate) next_layer_id: LayerId,
    #[serde(default = "default_next_entity_id")]
    pub(crate) next_entity_id: EntityId,
    #[serde(default = "default_next_parameter_id")]
    pub(crate) next_parameter_id: ParameterId,
    #[serde(default = "default_next_constraint_id")]
    pub(crate) next_constraint_id: ConstraintId,
}

const fn default_next_layer_id() -> LayerId {
    2
}

const fn default_next_entity_id() -> EntityId {
    1
}

const fn default_next_parameter_id() -> ParameterId {
    1
}

const fn default_next_constraint_id() -> ConstraintId {
    1
}

impl CadDocument {
    pub fn new(title: impl Into<String>) -> Self {
        let mut layers = BTreeMap::new();
        layers.insert(
            1,
            Layer {
                id: 1,
                name: "Concept".into(),
                visible: true,
                locked: false,
                color: [73, 184, 165, 255],
            },
        );
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            metadata: DocumentMetadata {
                title: title.into(),
                description: String::new(),
            },
            units: Units::Millimeters,
            layers,
            entities: BTreeMap::new(),
            parameters: BTreeMap::new(),
            constraints: BTreeMap::new(),
            next_layer_id: 2,
            next_entity_id: 1,
            next_parameter_id: 1,
            next_constraint_id: 1,
        }
    }

    pub fn next_entity_id(&self) -> EntityId {
        self.next_entity_id
    }

    pub fn next_layer_id(&self) -> LayerId {
        self.next_layer_id
    }

    pub fn next_parameter_id(&self) -> ParameterId {
        self.next_parameter_id
    }

    pub fn next_constraint_id(&self) -> ConstraintId {
        self.next_constraint_id
    }

    pub fn summary(&self) -> DocumentSummary {
        let mut domains = BTreeSet::new();
        for entity in self.entities.values() {
            domains.insert(entity.kind.domain());
        }
        DocumentSummary {
            title: self.metadata.title.clone(),
            entity_count: self.entities.len(),
            layer_count: self.layers.len(),
            domains: domains.into_iter().collect(),
        }
    }

    /// Validates the persisted document as a whole, including dependencies that
    /// can only be checked after a multi-command transaction has completed.
    pub fn validate(&self) -> Result<(), CommandError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(CommandError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.layers.is_empty() {
            return Err(CommandError::InvalidDocument(
                "a document must contain at least one layer".into(),
            ));
        }
        let mut layer_names = BTreeSet::new();
        for (id, layer) in &self.layers {
            if *id != layer.id {
                return Err(CommandError::InvalidDocument(format!(
                    "layer map key {id} does not match layer id {}",
                    layer.id
                )));
            }
            if layer.id == LayerId::MAX || layer.name.trim().is_empty() {
                return Err(CommandError::InvalidDocument(
                    "layer ids and names must be valid".into(),
                ));
            }
            if !layer_names.insert(layer.name.trim().to_ascii_lowercase()) {
                return Err(CommandError::InvalidLayer(format!(
                    "layer name {} is not unique",
                    layer.name
                )));
            }
        }
        let mut parameter_names = BTreeSet::new();
        for (id, parameter) in &self.parameters {
            if *id != parameter.id {
                return Err(CommandError::InvalidDocument(format!(
                    "parameter map key {id} does not match parameter id {}",
                    parameter.id
                )));
            }
            if parameter.id == ParameterId::MAX
                || !is_valid_parameter_name(&parameter.name)
                || !parameter.value.is_finite()
            {
                return Err(CommandError::InvalidDocument(
                    "parameter ids, names, and values must be valid".into(),
                ));
            }
            if !parameter_names.insert(parameter.name.clone()) {
                return Err(CommandError::InvalidParameter(format!(
                    "parameter name {} is not unique",
                    parameter.name
                )));
            }
        }
        self.evaluate_parameter_values()
            .map_err(|error| CommandError::InvalidParameter(error.to_string()))?;
        for (id, entity) in &self.entities {
            if *id != entity.id {
                return Err(CommandError::InvalidDocument(format!(
                    "entity map key {id} does not match entity id {}",
                    entity.id
                )));
            }
            if entity.id == EntityId::MAX {
                return Err(CommandError::InvalidDocument(
                    "entity id space is exhausted".into(),
                ));
            }
            validate_entity(entity, self)?;
            if let Some(parameter_id) = entity
                .parameter_refs
                .iter()
                .find(|parameter_id| !self.parameters.contains_key(parameter_id))
            {
                return Err(CommandError::InvalidReference(format!(
                    "entity {} references missing parameter {parameter_id}",
                    entity.id
                )));
            }
        }
        for (id, constraint) in &self.constraints {
            if *id != constraint.id {
                return Err(CommandError::InvalidDocument(format!(
                    "constraint map key {id} does not match constraint id {}",
                    constraint.id
                )));
            }
            validate_constraint(constraint, self)
                .map_err(|error| CommandError::InvalidConstraint(error.to_string()))?;
        }
        validate_next_id("layer", self.next_layer_id, self.layers.keys().copied())?;
        validate_next_id("entity", self.next_entity_id, self.entities.keys().copied())?;
        validate_next_id(
            "parameter",
            self.next_parameter_id,
            self.parameters.keys().copied(),
        )?;
        validate_next_id(
            "constraint",
            self.next_constraint_id,
            self.constraints.keys().copied(),
        )?;
        Ok(())
    }

    /// Evaluates every parameter to document units, resolving formulas by name
    /// and rejecting cyclic references. Literal values are converted from each
    /// parameter's declared unit to the document unit.
    pub fn evaluate_parameter_values(&self) -> Result<BTreeMap<ParameterId, f64>, ExpressionError> {
        let mut parameter_ids_by_name = BTreeMap::new();
        for parameter in self.parameters.values() {
            if !is_valid_parameter_name(&parameter.name) {
                return Err(ExpressionError::UnknownParameter(parameter.name.clone()));
            }
            if parameter_ids_by_name
                .insert(parameter.name.clone(), parameter.id)
                .is_some()
            {
                return Err(ExpressionError::DuplicateParameterName(
                    parameter.name.clone(),
                ));
            }
        }
        let mut values = BTreeMap::new();
        let mut resolving = Vec::new();
        for parameter_id in self.parameters.keys().copied() {
            resolve_parameter_value(
                self,
                parameter_id,
                &parameter_ids_by_name,
                &mut values,
                &mut resolving,
            )?;
        }
        Ok(values)
    }

    /// Evaluates a typed dimension expression against this document's
    /// parameter graph. This is used by constraint dimensions and remains local
    /// even when an agent proposed the expression source.
    pub fn evaluate_expression(
        &self,
        expression: &ParameterExpression,
    ) -> Result<f64, ExpressionError> {
        let values = self.evaluate_parameter_values()?;
        let parameter_ids_by_name = self
            .parameters
            .values()
            .map(|parameter| (parameter.name.as_str(), parameter.id))
            .collect::<BTreeMap<_, _>>();
        expression.evaluate(|name| {
            let id = parameter_ids_by_name
                .get(name)
                .ok_or_else(|| ExpressionError::UnknownParameter(name.into()))?;
            values
                .get(id)
                .copied()
                .ok_or_else(|| ExpressionError::UnknownParameter(name.into()))
        })
    }

    /// Upgrades a serialized document in place before it is admitted to a
    /// workspace. Earlier versions did not persist the complete current model
    /// shape or reliable ID cursors.
    pub fn migrate_to_current(&mut self) -> Result<(), CommandError> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(CommandError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.schema_version < CURRENT_SCHEMA_VERSION {
            self.schema_version = CURRENT_SCHEMA_VERSION;
            self.normalize_next_ids()?;
        }
        self.validate()
    }

    fn normalize_next_ids(&mut self) -> Result<(), CommandError> {
        self.next_layer_id = next_id_after("layer", self.layers.keys().copied())?;
        self.next_entity_id = next_id_after("entity", self.entities.keys().copied())?;
        self.next_parameter_id = next_id_after("parameter", self.parameters.keys().copied())?;
        self.next_constraint_id = next_id_after("constraint", self.constraints.keys().copied())?;
        Ok(())
    }
}

fn resolve_parameter_value(
    document: &CadDocument,
    parameter_id: ParameterId,
    parameter_ids_by_name: &BTreeMap<String, ParameterId>,
    values: &mut BTreeMap<ParameterId, f64>,
    resolving: &mut Vec<ParameterId>,
) -> Result<f64, ExpressionError> {
    if let Some(value) = values.get(&parameter_id) {
        return Ok(*value);
    }
    if let Some(start) = resolving.iter().position(|id| *id == parameter_id) {
        let mut cycle = resolving[start..]
            .iter()
            .filter_map(|id| document.parameters.get(id))
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        if let Some(parameter) = document.parameters.get(&parameter_id) {
            cycle.push(parameter.name.clone());
        }
        return Err(ExpressionError::DependencyCycle(cycle));
    }
    let parameter = document
        .parameters
        .get(&parameter_id)
        .ok_or_else(|| ExpressionError::UnknownParameter(parameter_id.to_string()))?;
    resolving.push(parameter_id);
    let value = match &parameter.expression {
        Some(expression) => expression.evaluate(|name| {
            let parameter_id = parameter_ids_by_name
                .get(name)
                .copied()
                .ok_or_else(|| ExpressionError::UnknownParameter(name.into()))?;
            resolve_parameter_value(
                document,
                parameter_id,
                parameter_ids_by_name,
                values,
                resolving,
            )
        }),
        None => Ok(convert_units(
            parameter.value,
            parameter.unit,
            document.units,
        )),
    };
    resolving.pop();
    let value = value?;
    if !value.is_finite() {
        return Err(ExpressionError::NonFiniteResult);
    }
    values.insert(parameter_id, value);
    Ok(value)
}

fn convert_units(value: f64, from: Units, to: Units) -> f64 {
    value * unit_to_millimeters(from) / unit_to_millimeters(to)
}

const fn unit_to_millimeters(unit: Units) -> f64 {
    match unit {
        Units::Millimeters => 1.0,
        Units::Meters => 1_000.0,
        Units::Inches => 25.4,
    }
}

pub(crate) fn next_id_after(
    kind: &str,
    ids: impl Iterator<Item = u64>,
) -> Result<u64, CommandError> {
    match ids.max() {
        Some(id) => id
            .checked_add(1)
            .ok_or_else(|| CommandError::InvalidDocument(format!("{kind} id space is exhausted"))),
        None => Ok(1),
    }
}

fn validate_next_id(
    kind: &str,
    next_id: u64,
    ids: impl Iterator<Item = u64>,
) -> Result<(), CommandError> {
    let required = next_id_after(kind, ids)?;
    if next_id < required {
        return Err(CommandError::InvalidDocument(format!(
            "next {kind} id {next_id} is behind existing ids"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub title: String,
    pub entity_count: usize,
    pub layer_count: usize,
    pub domains: Vec<Domain>,
}
pub(crate) fn validate_entity(entity: &Entity, document: &CadDocument) -> Result<(), CommandError> {
    if entity.id == EntityId::MAX || entity.name.trim().is_empty() {
        return Err(CommandError::InvalidGeometry(
            "entity id and name must be valid".into(),
        ));
    }
    if !document.layers.contains_key(&entity.layer) {
        return Err(CommandError::LayerMissing(entity.layer));
    }
    entity.kind.validate(document)
}

fn finite_point(point: Point2) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    LayerExists(LayerId),
    LayerMissing(LayerId),
    LayerLocked(LayerId),
    EntityExists(EntityId),
    EntityMissing(EntityId),
    ConstraintExists(ConstraintId),
    ConstraintMissing(ConstraintId),
    InvalidLayer(String),
    InvalidGeometry(String),
    InvalidParameter(String),
    InvalidConstraint(String),
    InvalidReference(String),
    InvalidDocument(String),
    UnsupportedSchemaVersion(u32),
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerExists(id) => write!(formatter, "layer {id} already exists"),
            Self::LayerMissing(id) => write!(formatter, "layer {id} does not exist"),
            Self::LayerLocked(id) => write!(formatter, "layer {id} is locked"),
            Self::EntityExists(id) => write!(formatter, "entity {id} already exists"),
            Self::EntityMissing(id) => write!(formatter, "entity {id} does not exist"),
            Self::ConstraintExists(id) => write!(formatter, "constraint {id} already exists"),
            Self::ConstraintMissing(id) => write!(formatter, "constraint {id} does not exist"),
            Self::InvalidLayer(message)
            | Self::InvalidGeometry(message)
            | Self::InvalidParameter(message)
            | Self::InvalidConstraint(message)
            | Self::InvalidReference(message)
            | Self::InvalidDocument(message) => formatter.write_str(message),
            Self::UnsupportedSchemaVersion(version) => write!(
                formatter,
                "document schema version {version} is not supported by this build"
            ),
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CadCommand, CommandTransaction};

    #[test]
    fn parameter_formulas_resolve_dependencies_in_document_units() {
        let mut document = CadDocument::new("Expressions");
        CommandTransaction::new(vec![
            CadCommand::SetParameter {
                parameter: Parameter::literal(1, "base_width", 2.0, Units::Inches),
            },
            CadCommand::SetParameter {
                parameter: Parameter::literal(2, "clearance", 10.0, Units::Millimeters),
            },
            CadCommand::SetParameter {
                parameter: Parameter::formula(
                    3,
                    "overall_width",
                    "base_width * 2 + clearance",
                    Units::Millimeters,
                )
                .unwrap(),
            },
        ])
        .apply(&mut document)
        .unwrap();

        let values = document.evaluate_parameter_values().unwrap();
        assert!((values[&1] - 50.8).abs() < 1e-10);
        assert!((values[&3] - 111.6).abs() < 1e-10);
    }

    #[test]
    fn cyclic_parameter_formulas_are_rejected_atomically() {
        let mut document = CadDocument::new("Cycles");
        let before = document.clone();
        let transaction = CommandTransaction::new(vec![
            CadCommand::SetParameter {
                parameter: Parameter::formula(1, "a", "b + 1", Units::Millimeters).unwrap(),
            },
            CadCommand::SetParameter {
                parameter: Parameter::formula(2, "b", "a + 1", Units::Millimeters).unwrap(),
            },
        ]);

        let error = transaction.apply(&mut document).unwrap_err();

        assert!(matches!(error, CommandError::InvalidParameter(_)));
        assert_eq!(document, before);
    }

    #[test]
    fn arcs_require_a_finite_partial_angular_span_atomically() {
        let mut document = CadDocument::new("Arcs");
        let before = document.clone();
        let entity = |id, sweep_angle| Entity {
            id,
            layer: 1,
            name: format!("Arc {id}"),
            visible: true,
            kind: EntityKind::Arc {
                center: Point2::new(2.0, 3.0),
                radius: 5.0,
                start_angle: 0.25,
                sweep_angle,
            },
            parameter_refs: BTreeSet::new(),
        };
        let transaction = CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: entity(1, std::f64::consts::PI),
            },
            CadCommand::CreateEntity {
                entity: entity(2, std::f64::consts::TAU),
            },
        ]);

        assert!(matches!(
            transaction.apply(&mut document),
            Err(CommandError::InvalidGeometry(_))
        ));
        assert_eq!(document, before);

        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: entity(1, std::f64::consts::PI),
        }])
        .apply(&mut document)
        .unwrap();
        assert_eq!(document.entities[&1].kind.domain(), Domain::Drafting);
        document.validate().unwrap();
    }

    #[test]
    fn aligned_dimensions_require_distinct_points_and_a_nonzero_offset_atomically() {
        let mut document = CadDocument::new("Dimensions");
        let before = document.clone();
        let entity = |id, end, offset| Entity {
            id,
            layer: 1,
            name: format!("Dimension {id}"),
            visible: true,
            kind: EntityKind::AlignedDimension {
                start: Point2::new(0.0, 0.0),
                end,
                offset,
                text_override: Some("REF <>".into()),
            },
            parameter_refs: BTreeSet::new(),
        };
        let transaction = CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: entity(1, Point2::new(20.0, 0.0), 8.0),
            },
            CadCommand::CreateEntity {
                entity: entity(2, Point2::new(20.0, 0.0), 0.0),
            },
        ]);

        assert!(matches!(
            transaction.apply(&mut document),
            Err(CommandError::InvalidGeometry(_))
        ));
        assert_eq!(document, before);

        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: entity(1, Point2::new(20.0, 0.0), -8.0),
        }])
        .apply(&mut document)
        .unwrap();
        assert_eq!(document.entities[&1].kind.domain(), Domain::Drafting);
        document.validate().unwrap();
    }
}
