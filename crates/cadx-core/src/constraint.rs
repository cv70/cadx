//! Deterministic two-dimensional sketch constraints.
//!
//! The solver is intentionally small and explicit: it uses ordered projection
//! iterations for point coincidence, horizontal/vertical relationships,
//! point-to-point distance, and circle radius. It produces a normal command
//! transaction instead of mutating a workspace, so successful solves are
//! replayable and authorization remains at the existing command boundary.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::command::{CadCommand, CommandTransaction};
use crate::document::{CadDocument, Entity, EntityKind, Point2};
use crate::expression::{ExpressionError, ParameterExpression};
use crate::{ConstraintId, EntityId};

const MAX_CONSTRAINT_NAME_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointAnchor {
    Start,
    End,
    Center,
    Origin,
    Vertex { index: u32 },
    Insertion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchPoint {
    pub entity_id: EntityId,
    pub anchor: PointAnchor,
}

impl SketchPoint {
    pub const fn new(entity_id: EntityId, anchor: PointAnchor) -> Self {
        Self { entity_id, anchor }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchSegment {
    pub start: SketchPoint,
    pub end: SketchPoint,
}

impl SketchSegment {
    pub const fn new(start: SketchPoint, end: SketchPoint) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintKind {
    Coincident {
        first: SketchPoint,
        second: SketchPoint,
    },
    Horizontal {
        segment: SketchSegment,
    },
    Vertical {
        segment: SketchSegment,
    },
    Distance {
        first: SketchPoint,
        second: SketchPoint,
        value: ParameterExpression,
    },
    Radius {
        entity_id: EntityId,
        value: ParameterExpression,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchConstraint {
    pub id: ConstraintId,
    pub name: String,
    pub driving: bool,
    pub kind: ConstraintKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstraintSolverSettings {
    pub max_iterations: u32,
    pub tolerance: f64,
}

impl Default for ConstraintSolverSettings {
    fn default() -> Self {
        Self {
            max_iterations: 32,
            tolerance: 1e-7,
        }
    }
}

impl ConstraintSolverSettings {
    pub fn validate(self) -> Result<(), ConstraintError> {
        if self.max_iterations == 0 || self.max_iterations > 1_024 {
            return Err(ConstraintError::InvalidSettings(
                "constraint iterations must be between 1 and 1024".into(),
            ));
        }
        if !self.tolerance.is_finite() || self.tolerance <= 0.0 {
            return Err(ConstraintError::InvalidSettings(
                "constraint tolerance must be finite and positive".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstraintDiagnostic {
    pub constraint_id: ConstraintId,
    pub driving: bool,
    pub residual: f64,
    pub satisfied: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstraintSolution {
    pub iterations: u32,
    pub converged: bool,
    pub diagnostics: Vec<ConstraintDiagnostic>,
    pub updated_entities: Vec<Entity>,
}

impl ConstraintSolution {
    pub fn maximum_residual(&self) -> f64 {
        self.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.residual)
            .fold(0.0, f64::max)
    }

    pub fn maximum_driving_residual(&self) -> f64 {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.driving)
            .map(|diagnostic| diagnostic.residual)
            .fold(0.0, f64::max)
    }

    /// Converts a converged solution into replayable local mutations.
    pub fn transaction(&self) -> Result<CommandTransaction, ConstraintError> {
        if !self.converged {
            return Err(ConstraintError::NotConverged);
        }
        Ok(CommandTransaction::new(
            self.updated_entities
                .iter()
                .cloned()
                .map(|entity| CadCommand::UpdateEntity { entity })
                .collect(),
        ))
    }
}

/// Validates structural references and dimension expressions without changing
/// geometry. An unsatisfied constraint is valid data and is diagnosed by the
/// solver rather than rejected during document loading.
pub(crate) fn validate_constraint(
    constraint: &SketchConstraint,
    document: &CadDocument,
) -> Result<(), ConstraintError> {
    if constraint.id == ConstraintId::MAX || constraint.name.trim().is_empty() {
        return Err(ConstraintError::InvalidConstraint(
            "constraint id and name must be valid".into(),
        ));
    }
    if constraint.name.len() > MAX_CONSTRAINT_NAME_BYTES {
        return Err(ConstraintError::InvalidConstraint(format!(
            "constraint name exceeds the {MAX_CONSTRAINT_NAME_BYTES}-byte limit"
        )));
    }
    match &constraint.kind {
        ConstraintKind::Coincident { first, second } => {
            validate_distinct_points(*first, *second)?;
            validate_point(document, *first)?;
            validate_point(document, *second)?;
        }
        ConstraintKind::Horizontal { segment } | ConstraintKind::Vertical { segment } => {
            validate_segment(document, *segment)?;
        }
        ConstraintKind::Distance {
            first,
            second,
            value,
        } => {
            validate_distinct_points(*first, *second)?;
            validate_point(document, *first)?;
            validate_point(document, *second)?;
            let value = dimension_value(document, value)?;
            if value < 0.0 {
                return Err(ConstraintError::InvalidConstraint(
                    "distance constraint value must not be negative".into(),
                ));
            }
        }
        ConstraintKind::Radius { entity_id, value } => {
            let entity = document
                .entities
                .get(entity_id)
                .ok_or(ConstraintError::EntityMissing(*entity_id))?;
            if !matches!(
                entity.kind,
                EntityKind::Circle { .. } | EntityKind::Arc { .. }
            ) {
                return Err(ConstraintError::InvalidConstraint(
                    "radius constraints require a circle or arc entity".into(),
                ));
            }
            let value = dimension_value(document, value)?;
            if value <= 0.0 {
                return Err(ConstraintError::InvalidConstraint(
                    "radius constraint value must be positive".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Solves every document constraint in stable ID order. Non-convergence is
/// reported in the returned solution and can never be turned into a command
/// transaction through [`ConstraintSolution::transaction`].
pub fn solve_constraints(
    document: &CadDocument,
    settings: ConstraintSolverSettings,
) -> Result<ConstraintSolution, ConstraintError> {
    settings.validate()?;
    document
        .validate()
        .map_err(|error| ConstraintError::InvalidConstraint(error.to_string()))?;
    for constraint in document.constraints.values() {
        validate_constraint(constraint, document)?;
    }
    if document.constraints.is_empty() {
        return Ok(ConstraintSolution {
            iterations: 0,
            converged: true,
            diagnostics: Vec::new(),
            updated_entities: Vec::new(),
        });
    }

    let mut entities = document.entities.clone();
    let mut diagnostics = Vec::new();
    let mut iterations = 0;
    for iteration in 1..=settings.max_iterations {
        iterations = iteration;
        for constraint in document
            .constraints
            .values()
            .filter(|constraint| constraint.driving)
        {
            project_constraint(constraint, &mut entities, document)?;
        }
        diagnostics = diagnostics_for(document, &entities, settings.tolerance)?;
        if diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.driving || diagnostic.satisfied)
        {
            break;
        }
    }
    let converged = diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.driving || diagnostic.satisfied);
    let updated_entities = entities
        .into_iter()
        .filter_map(|(id, entity)| (document.entities.get(&id) != Some(&entity)).then_some(entity))
        .collect();
    Ok(ConstraintSolution {
        iterations,
        converged,
        diagnostics,
        updated_entities,
    })
}

fn diagnostics_for(
    document: &CadDocument,
    entities: &BTreeMap<EntityId, Entity>,
    tolerance: f64,
) -> Result<Vec<ConstraintDiagnostic>, ConstraintError> {
    document
        .constraints
        .values()
        .map(|constraint| {
            let residual = constraint_residual(constraint, entities, document)?;
            Ok(ConstraintDiagnostic {
                constraint_id: constraint.id,
                driving: constraint.driving,
                residual,
                satisfied: residual <= tolerance,
            })
        })
        .collect()
}

fn project_constraint(
    constraint: &SketchConstraint,
    entities: &mut BTreeMap<EntityId, Entity>,
    document: &CadDocument,
) -> Result<(), ConstraintError> {
    match &constraint.kind {
        ConstraintKind::Coincident { first, second } => {
            let first_value = point_value(entities, *first)?;
            let second_value = point_value(entities, *second)?;
            let midpoint = Point2::new(
                (first_value.x + second_value.x) * 0.5,
                (first_value.y + second_value.y) * 0.5,
            );
            set_point_value(entities, *first, midpoint)?;
            set_point_value(entities, *second, midpoint)
        }
        ConstraintKind::Horizontal { segment } => {
            let start = point_value(entities, segment.start)?;
            let end = point_value(entities, segment.end)?;
            let y = (start.y + end.y) * 0.5;
            set_point_value(entities, segment.start, Point2::new(start.x, y))?;
            set_point_value(entities, segment.end, Point2::new(end.x, y))
        }
        ConstraintKind::Vertical { segment } => {
            let start = point_value(entities, segment.start)?;
            let end = point_value(entities, segment.end)?;
            let x = (start.x + end.x) * 0.5;
            set_point_value(entities, segment.start, Point2::new(x, start.y))?;
            set_point_value(entities, segment.end, Point2::new(x, end.y))
        }
        ConstraintKind::Distance {
            first,
            second,
            value,
        } => {
            let target = dimension_value(document, value)?;
            let first_value = point_value(entities, *first)?;
            let second_value = point_value(entities, *second)?;
            let dx = second_value.x - first_value.x;
            let dy = second_value.y - first_value.y;
            let current = dx.hypot(dy);
            let (direction_x, direction_y) = if current > f64::EPSILON {
                (dx / current, dy / current)
            } else {
                (1.0, 0.0)
            };
            let midpoint = Point2::new(
                (first_value.x + second_value.x) * 0.5,
                (first_value.y + second_value.y) * 0.5,
            );
            let half = target * 0.5;
            set_point_value(
                entities,
                *first,
                Point2::new(
                    midpoint.x - direction_x * half,
                    midpoint.y - direction_y * half,
                ),
            )?;
            set_point_value(
                entities,
                *second,
                Point2::new(
                    midpoint.x + direction_x * half,
                    midpoint.y + direction_y * half,
                ),
            )
        }
        ConstraintKind::Radius { entity_id, value } => {
            let value = dimension_value(document, value)?;
            let entity = entities
                .get_mut(entity_id)
                .ok_or(ConstraintError::EntityMissing(*entity_id))?;
            match &mut entity.kind {
                EntityKind::Circle { radius, .. } | EntityKind::Arc { radius, .. } => {
                    *radius = value;
                    Ok(())
                }
                _ => Err(ConstraintError::InvalidConstraint(
                    "radius constraints require a circle entity".into(),
                )),
            }
        }
    }
}

fn constraint_residual(
    constraint: &SketchConstraint,
    entities: &BTreeMap<EntityId, Entity>,
    document: &CadDocument,
) -> Result<f64, ConstraintError> {
    let residual = match &constraint.kind {
        ConstraintKind::Coincident { first, second } => distance(
            point_value(entities, *first)?,
            point_value(entities, *second)?,
        ),
        ConstraintKind::Horizontal { segment } => {
            (point_value(entities, segment.start)?.y - point_value(entities, segment.end)?.y).abs()
        }
        ConstraintKind::Vertical { segment } => {
            (point_value(entities, segment.start)?.x - point_value(entities, segment.end)?.x).abs()
        }
        ConstraintKind::Distance {
            first,
            second,
            value,
        } => {
            let expected = dimension_value(document, value)?;
            (distance(
                point_value(entities, *first)?,
                point_value(entities, *second)?,
            ) - expected)
                .abs()
        }
        ConstraintKind::Radius { entity_id, value } => {
            let expected = dimension_value(document, value)?;
            let entity = entities
                .get(entity_id)
                .ok_or(ConstraintError::EntityMissing(*entity_id))?;
            match entity.kind {
                EntityKind::Circle { radius, .. } | EntityKind::Arc { radius, .. } => {
                    (radius - expected).abs()
                }
                _ => {
                    return Err(ConstraintError::InvalidConstraint(
                        "radius constraints require a circle entity".into(),
                    ));
                }
            }
        }
    };
    if residual.is_finite() {
        Ok(residual)
    } else {
        Err(ConstraintError::NonFiniteGeometry)
    }
}

fn validate_segment(document: &CadDocument, segment: SketchSegment) -> Result<(), ConstraintError> {
    validate_distinct_points(segment.start, segment.end)?;
    validate_point(document, segment.start)?;
    validate_point(document, segment.end)
}

fn validate_distinct_points(
    first: SketchPoint,
    second: SketchPoint,
) -> Result<(), ConstraintError> {
    if first == second {
        return Err(ConstraintError::InvalidConstraint(
            "a constraint must reference two distinct sketch points".into(),
        ));
    }
    Ok(())
}

fn validate_point(document: &CadDocument, point: SketchPoint) -> Result<(), ConstraintError> {
    point_value(&document.entities, point).map(|_| ())
}

fn dimension_value(
    document: &CadDocument,
    expression: &ParameterExpression,
) -> Result<f64, ConstraintError> {
    document
        .evaluate_expression(expression)
        .map_err(ConstraintError::Expression)
}

fn point_value(
    entities: &BTreeMap<EntityId, Entity>,
    point: SketchPoint,
) -> Result<Point2, ConstraintError> {
    let entity = entities
        .get(&point.entity_id)
        .ok_or(ConstraintError::EntityMissing(point.entity_id))?;
    match (&entity.kind, point.anchor) {
        (EntityKind::Line { start, .. } | EntityKind::Wall { start, .. }, PointAnchor::Start) => {
            Ok(*start)
        }
        (EntityKind::Line { end, .. } | EntityKind::Wall { end, .. }, PointAnchor::End) => Ok(*end),
        (
            EntityKind::Circle { center, .. } | EntityKind::Arc { center, .. },
            PointAnchor::Center,
        ) => Ok(*center),
        (EntityKind::Rectangle { origin, .. }, PointAnchor::Origin) => Ok(*origin),
        (EntityKind::SketchProfile { points, .. }, PointAnchor::Vertex { index }) => points
            .get(index as usize)
            .copied()
            .ok_or(ConstraintError::AnchorMissing {
                entity_id: point.entity_id,
                anchor: point.anchor,
            }),
        (EntityKind::Text { position, .. }, PointAnchor::Insertion) => Ok(*position),
        _ => Err(ConstraintError::AnchorUnsupported {
            entity_id: point.entity_id,
            anchor: point.anchor,
        }),
    }
}

fn set_point_value(
    entities: &mut BTreeMap<EntityId, Entity>,
    point: SketchPoint,
    value: Point2,
) -> Result<(), ConstraintError> {
    if !value.x.is_finite() || !value.y.is_finite() {
        return Err(ConstraintError::NonFiniteGeometry);
    }
    let entity = entities
        .get_mut(&point.entity_id)
        .ok_or(ConstraintError::EntityMissing(point.entity_id))?;
    match (&mut entity.kind, point.anchor) {
        (EntityKind::Line { start, .. } | EntityKind::Wall { start, .. }, PointAnchor::Start) => {
            *start = value;
            Ok(())
        }
        (EntityKind::Line { end, .. } | EntityKind::Wall { end, .. }, PointAnchor::End) => {
            *end = value;
            Ok(())
        }
        (
            EntityKind::Circle { center, .. } | EntityKind::Arc { center, .. },
            PointAnchor::Center,
        ) => {
            *center = value;
            Ok(())
        }
        (EntityKind::Rectangle { origin, .. }, PointAnchor::Origin) => {
            *origin = value;
            Ok(())
        }
        (EntityKind::SketchProfile { points, .. }, PointAnchor::Vertex { index }) => {
            let point = points
                .get_mut(index as usize)
                .ok_or(ConstraintError::AnchorMissing {
                    entity_id: point.entity_id,
                    anchor: point.anchor,
                })?;
            *point = value;
            Ok(())
        }
        (EntityKind::Text { position, .. }, PointAnchor::Insertion) => {
            *position = value;
            Ok(())
        }
        _ => Err(ConstraintError::AnchorUnsupported {
            entity_id: point.entity_id,
            anchor: point.anchor,
        }),
    }
}

fn distance(first: Point2, second: Point2) -> f64 {
    (first.x - second.x).hypot(first.y - second.y)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConstraintError {
    EntityMissing(EntityId),
    AnchorMissing {
        entity_id: EntityId,
        anchor: PointAnchor,
    },
    AnchorUnsupported {
        entity_id: EntityId,
        anchor: PointAnchor,
    },
    Expression(ExpressionError),
    InvalidConstraint(String),
    InvalidSettings(String),
    NonFiniteGeometry,
    NotConverged,
}

impl fmt::Display for ConstraintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityMissing(id) => write!(formatter, "constraint entity {id} does not exist"),
            Self::AnchorMissing { entity_id, anchor } => {
                write!(
                    formatter,
                    "constraint anchor {anchor:?} is missing on entity {entity_id}"
                )
            }
            Self::AnchorUnsupported { entity_id, anchor } => write!(
                formatter,
                "constraint anchor {anchor:?} is not supported by entity {entity_id}"
            ),
            Self::Expression(error) => error.fmt(formatter),
            Self::InvalidConstraint(message) | Self::InvalidSettings(message) => {
                formatter.write_str(message)
            }
            Self::NonFiniteGeometry => {
                formatter.write_str("constraint solve produced invalid geometry")
            }
            Self::NotConverged => formatter.write_str("constraints did not converge"),
        }
    }
}

impl std::error::Error for ConstraintError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{
        CadCommand, Capability, CommandTransaction, History, Parameter, TaskAuthority, Units,
        ValidationReport,
    };

    fn line(id: EntityId, start: Point2, end: Point2) -> Entity {
        Entity {
            id,
            layer: 1,
            name: format!("Line {id}"),
            visible: true,
            kind: EntityKind::Line { start, end },
            parameter_refs: BTreeSet::new(),
        }
    }

    fn line_endpoints() -> (SketchPoint, SketchPoint) {
        (
            SketchPoint::new(1, PointAnchor::Start),
            SketchPoint::new(1, PointAnchor::End),
        )
    }

    #[test]
    fn solver_creates_replayable_updates_for_parameterized_constraints() {
        let mut document = CadDocument::new("Constrained line");
        let (start, end) = line_endpoints();
        CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: line(1, Point2::new(0.0, 3.0), Point2::new(8.0, 11.0)),
            },
            CadCommand::SetParameter {
                parameter: Parameter::literal(1, "target_length", 40.0, Units::Millimeters),
            },
            CadCommand::CreateConstraint {
                constraint: SketchConstraint {
                    id: 1,
                    name: "Level".into(),
                    driving: true,
                    kind: ConstraintKind::Horizontal {
                        segment: SketchSegment::new(start, end),
                    },
                },
            },
            CadCommand::CreateConstraint {
                constraint: SketchConstraint {
                    id: 2,
                    name: "Length".into(),
                    driving: true,
                    kind: ConstraintKind::Distance {
                        first: start,
                        second: end,
                        value: ParameterExpression::new("target_length").unwrap(),
                    },
                },
            },
        ])
        .apply(&mut document)
        .unwrap();

        let solution = solve_constraints(&document, ConstraintSolverSettings::default()).unwrap();
        assert!(solution.converged);
        assert_eq!(solution.updated_entities.len(), 1);
        assert!(
            solution
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.satisfied)
        );

        let transaction = solution.transaction().unwrap();
        let mut history = History::new(document.clone());
        let (solved, commit_id) = history
            .commit(
                &document,
                None,
                "Solve constrained line",
                transaction,
                ValidationReport::default(),
            )
            .unwrap();
        let EntityKind::Line { start, end } = &solved.entities[&1].kind else {
            panic!("expected solved line");
        };
        assert!((start.y - end.y).abs() < 1e-7);
        assert!((distance(*start, *end) - 40.0).abs() < 1e-7);
        assert_eq!(history.restore(commit_id).unwrap(), solved);
    }

    #[test]
    fn radius_constraints_update_arcs_without_changing_their_angles() {
        let mut document = CadDocument::new("Constrained arc");
        CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: Entity {
                    id: 1,
                    layer: 1,
                    name: "Arc".into(),
                    visible: true,
                    kind: EntityKind::Arc {
                        center: Point2::new(2.0, 3.0),
                        radius: 1.0,
                        start_angle: 0.5,
                        sweep_angle: 2.0,
                    },
                    parameter_refs: BTreeSet::new(),
                },
            },
            CadCommand::CreateConstraint {
                constraint: SketchConstraint {
                    id: 1,
                    name: "Arc radius".into(),
                    driving: true,
                    kind: ConstraintKind::Radius {
                        entity_id: 1,
                        value: ParameterExpression::new("8").unwrap(),
                    },
                },
            },
        ])
        .apply(&mut document)
        .unwrap();

        let solution = solve_constraints(&document, ConstraintSolverSettings::default()).unwrap();
        assert!(solution.converged);
        let mut solved = document.clone();
        solution.transaction().unwrap().apply(&mut solved).unwrap();
        let EntityKind::Arc {
            radius,
            start_angle,
            sweep_angle,
            ..
        } = solved.entities[&1].kind
        else {
            panic!("expected solved arc");
        };
        assert!((radius - 8.0).abs() < 1.0e-9);
        assert_eq!(start_angle, 0.5);
        assert_eq!(sweep_angle, 2.0);
    }

    #[test]
    fn conflicting_dimensions_cannot_be_committed() {
        let mut document = CadDocument::new("Conflicting radii");
        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: Entity {
                id: 1,
                layer: 1,
                name: "Circle".into(),
                visible: true,
                kind: EntityKind::Circle {
                    center: Point2::new(0.0, 0.0),
                    radius: 1.0,
                },
                parameter_refs: BTreeSet::new(),
            },
        }])
        .apply(&mut document)
        .unwrap();
        for (id, name, value) in [(1, "Small", "5"), (2, "Large", "10")] {
            CommandTransaction::new(vec![CadCommand::CreateConstraint {
                constraint: SketchConstraint {
                    id,
                    name: name.into(),
                    driving: true,
                    kind: ConstraintKind::Radius {
                        entity_id: 1,
                        value: ParameterExpression::new(value).unwrap(),
                    },
                },
            }])
            .apply(&mut document)
            .unwrap();
        }
        let before = document.clone();

        let solution = solve_constraints(
            &document,
            ConstraintSolverSettings {
                max_iterations: 4,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(!solution.converged);
        assert_eq!(solution.transaction(), Err(ConstraintError::NotConverged));
        assert_eq!(document, before);
    }

    #[test]
    fn constraint_edits_require_mechanical_authority() {
        let mut document = CadDocument::new("Authority");
        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: line(1, Point2::new(0.0, 0.0), Point2::new(10.0, 1.0)),
        }])
        .apply(&mut document)
        .unwrap();
        let (start, end) = line_endpoints();
        let transaction = CommandTransaction::new(vec![CadCommand::CreateConstraint {
            constraint: SketchConstraint {
                id: 1,
                name: "Horizontal".into(),
                driving: true,
                kind: ConstraintKind::Horizontal {
                    segment: SketchSegment::new(start, end),
                },
            },
        }]);
        let authority = TaskAuthority::DirectWrite {
            capabilities: BTreeSet::from([Capability::Drafting]),
        };

        assert!(!authority.permits(&transaction, &document));
        assert!(TaskAuthority::all_direct().permits(&transaction, &document));
    }

    #[test]
    fn reference_constraints_are_diagnostic_only() {
        let mut document = CadDocument::new("Reference constraint");
        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: line(1, Point2::new(0.0, 0.0), Point2::new(10.0, 4.0)),
        }])
        .apply(&mut document)
        .unwrap();
        let (start, end) = line_endpoints();
        CommandTransaction::new(vec![CadCommand::CreateConstraint {
            constraint: SketchConstraint {
                id: 1,
                name: "Measured level".into(),
                driving: false,
                kind: ConstraintKind::Horizontal {
                    segment: SketchSegment::new(start, end),
                },
            },
        }])
        .apply(&mut document)
        .unwrap();

        let solution = solve_constraints(&document, ConstraintSolverSettings::default()).unwrap();

        assert!(solution.converged);
        assert!(solution.updated_entities.is_empty());
        assert!(!solution.diagnostics[0].satisfied);
        assert!(!solution.diagnostics[0].driving);
        assert_eq!(solution.maximum_driving_residual(), 0.0);
    }
}
