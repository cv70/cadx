use std::collections::BTreeSet;

use cadx_core::{
    CadCommand, CadDocument, Capability, CheckResult, CheckStatus, CommandTransaction,
    ConstraintKind, Entity, EntityId, EntityKind, Parameter, ParameterExpression, Point2,
    SketchConstraint, SketchPoint, SketchSegment, ValidationReport, is_valid_parameter_name,
};
use serde::Deserialize;

use crate::error::AgentError;
use crate::provider::PlannedAction;

const MAX_REMOTE_DECISION_BYTES: usize = 64 * 1024;
const MAX_NAME_BYTES: usize = 256;
const MAX_TEXT_BYTES: usize = 4 * 1024;

pub(crate) fn decode_decision(body: &str) -> Result<RemotePlanningDecision, AgentError> {
    RemotePlanningDecision::decode_json(body)
}

impl RemotePlanningDecision {
    pub fn decode_json(body: &str) -> Result<Self, AgentError> {
        if body.len() > MAX_REMOTE_DECISION_BYTES {
            return Err(AgentError::Provider(
                "remote model response exceeds the supported size".into(),
            ));
        }
        let decision = serde_json::from_str::<WireDecision>(body).map_err(|_| {
            AgentError::Provider("remote model returned an invalid planning decision".into())
        })?;
        if matches!(&decision, WireDecision::Complete { summary } if summary.trim().is_empty()) {
            return Err(AgentError::Provider(
                "remote model returned an invalid planning decision".into(),
            ));
        }
        Ok(Self(decision))
    }
}

pub(crate) fn materialize_decision(
    decision: RemotePlanningDecision,
    document: &CadDocument,
    allowed_capabilities: &BTreeSet<Capability>,
) -> Result<crate::provider::PlanningDecision, AgentError> {
    match decision.0 {
        WireDecision::Action { action } => {
            let layer = document
                .layers
                .values()
                .find(|layer| layer.visible && !layer.locked)
                .map(|layer| layer.id)
                .ok_or_else(|| AgentError::Provider("document has no editable layer".into()))?;
            let action = action.into_planned_action(document, layer, allowed_capabilities)?;
            action.transaction.preview(document).map_err(|_| {
                AgentError::Provider(
                    "remote action is not valid against the observed document".into(),
                )
            })?;
            Ok(crate::provider::PlanningDecision::Action(action))
        }
        WireDecision::Complete { summary } => Ok(crate::provider::PlanningDecision::Complete {
            summary: bounded_text(summary, "completion summary", MAX_TEXT_BYTES)?,
        }),
    }
}

#[derive(Debug)]
pub struct RemotePlanningDecision(WireDecision);

#[derive(Debug, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
enum WireDecision {
    Action { action: WireAction },
    Complete { summary: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAction {
    intent: String,
    detail: String,
    operation: WireOperation,
    #[serde(default)]
    validation: Vec<WireCheck>,
}

impl WireAction {
    fn into_planned_action(
        self,
        document: &CadDocument,
        layer: u64,
        allowed_capabilities: &BTreeSet<Capability>,
    ) -> Result<PlannedAction, AgentError> {
        if !self
            .operation
            .required_capabilities()
            .iter()
            .all(|capability| allowed_capabilities.contains(capability))
        {
            return Err(AgentError::Provider(
                "remote plan contains an operation outside the approved task capabilities".into(),
            ));
        }
        let (tool_name, transaction) = self.operation.into_transaction(document, layer)?;
        let validation = ValidationReport {
            checks: self
                .validation
                .into_iter()
                .map(WireCheck::into_check)
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(PlannedAction {
            intent: bounded_text(self.intent, "intent", MAX_TEXT_BYTES)?,
            tool_name: format!("remote.{tool_name}"),
            detail: bounded_text(self.detail, "detail", MAX_TEXT_BYTES)?,
            transaction,
            validation,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WireOperation {
    #[serde(rename = "create_line")]
    Line {
        name: String,
        start: [f64; 2],
        end: [f64; 2],
    },
    #[serde(rename = "create_circle")]
    Circle {
        name: String,
        center: [f64; 2],
        radius: f64,
    },
    #[serde(rename = "create_rectangle")]
    Rectangle {
        name: String,
        origin: [f64; 2],
        width: f64,
        height: f64,
    },
    #[serde(rename = "create_sketch_profile")]
    SketchProfile {
        name: String,
        points: Vec<[f64; 2]>,
        closed: bool,
    },
    #[serde(rename = "create_wall")]
    Wall {
        name: String,
        start: [f64; 2],
        end: [f64; 2],
        thickness: f64,
    },
    #[serde(rename = "create_room")]
    Room {
        name: String,
        boundary: Vec<[f64; 2]>,
    },
    #[serde(rename = "create_text")]
    Text {
        name: String,
        position: [f64; 2],
        content: String,
    },
    #[serde(rename = "create_parameter")]
    Parameter {
        name: String,
        #[serde(default)]
        value: Option<f64>,
        #[serde(default)]
        formula: Option<String>,
    },
    #[serde(rename = "create_constrained_line")]
    ConstrainedLine {
        name: String,
        start: [f64; 2],
        end: [f64; 2],
        #[serde(default)]
        horizontal: bool,
        #[serde(default)]
        vertical: bool,
        #[serde(default)]
        length: Option<String>,
    },
}

impl WireOperation {
    fn required_capabilities(&self) -> &'static [Capability] {
        match self {
            Self::Line { .. }
            | Self::Circle { .. }
            | Self::Rectangle { .. }
            | Self::Text { .. } => &[Capability::Drafting],
            Self::SketchProfile { .. } => &[Capability::Mechanical],
            Self::Wall { .. } | Self::Room { .. } => &[Capability::Architecture],
            Self::Parameter { .. } => &[Capability::Parameters],
            Self::ConstrainedLine { .. } => &[Capability::Drafting, Capability::Mechanical],
        }
    }

    fn entity_name(&self) -> Result<String, AgentError> {
        match self {
            Self::Line { name, .. }
            | Self::Circle { name, .. }
            | Self::Rectangle { name, .. }
            | Self::SketchProfile { name, .. }
            | Self::Wall { name, .. }
            | Self::Room { name, .. }
            | Self::Text { name, .. }
            | Self::ConstrainedLine { name, .. } => {
                bounded_text(name.clone(), "entity name", MAX_NAME_BYTES)
            }
            Self::Parameter { .. } => Err(AgentError::Provider(
                "parameter operations do not have an entity name".into(),
            )),
        }
    }

    fn into_entity_kind(self) -> Result<EntityKind, AgentError> {
        match self {
            Self::Line { start, end, .. } => Ok(EntityKind::Line {
                start: point(start)?,
                end: point(end)?,
            }),
            Self::Circle { center, radius, .. } => Ok(EntityKind::Circle {
                center: point(center)?,
                radius: positive_number(radius, "circle radius")?,
            }),
            Self::Rectangle {
                origin,
                width,
                height,
                ..
            } => Ok(EntityKind::Rectangle {
                origin: point(origin)?,
                width: positive_number(width, "rectangle width")?,
                height: positive_number(height, "rectangle height")?,
            }),
            Self::SketchProfile { points, closed, .. } => Ok(EntityKind::SketchProfile {
                points: points
                    .into_iter()
                    .map(point)
                    .collect::<Result<Vec<_>, _>>()?,
                closed,
            }),
            Self::Wall {
                start,
                end,
                thickness,
                ..
            } => Ok(EntityKind::Wall {
                start: point(start)?,
                end: point(end)?,
                thickness: positive_number(thickness, "wall thickness")?,
            }),
            Self::Room { boundary, .. } => {
                let boundary = boundary
                    .into_iter()
                    .map(point)
                    .collect::<Result<Vec<_>, _>>()?;
                let area = polygon_area(&boundary);
                if !area.is_finite() || area <= 0.0 {
                    return Err(AgentError::Provider(
                        "remote room boundary must enclose a positive area".into(),
                    ));
                }
                Ok(EntityKind::Room { boundary, area })
            }
            Self::Text {
                position, content, ..
            } => Ok(EntityKind::Text {
                position: point(position)?,
                content: bounded_text(content, "text content", MAX_TEXT_BYTES)?,
            }),
            Self::Parameter { .. } | Self::ConstrainedLine { .. } => Err(AgentError::Provider(
                "remote operation cannot be converted into a standalone entity".into(),
            )),
        }
    }

    fn into_transaction(
        self,
        document: &CadDocument,
        layer: u64,
    ) -> Result<(&'static str, CommandTransaction), AgentError> {
        match self {
            Self::Parameter {
                name,
                value,
                formula,
            } => parameter_transaction(document, name, value, formula),
            Self::ConstrainedLine {
                name,
                start,
                end,
                horizontal,
                vertical,
                length,
            } => constrained_line_transaction(
                document, layer, name, start, end, horizontal, vertical, length,
            ),
            operation => {
                let id = next_entity_id(document)?;
                let name = operation.entity_name()?;
                let kind = operation.into_entity_kind()?;
                let tool_name = entity_tool_name(&kind);
                Ok((
                    tool_name,
                    CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: Entity {
                            id,
                            layer,
                            name,
                            visible: true,
                            kind,
                            parameter_refs: BTreeSet::new(),
                        },
                    }]),
                ))
            }
        }
    }
}

fn parameter_transaction(
    document: &CadDocument,
    name: String,
    value: Option<f64>,
    formula: Option<String>,
) -> Result<(&'static str, CommandTransaction), AgentError> {
    let name = bounded_text(name, "parameter name", MAX_NAME_BYTES)?;
    if !is_valid_parameter_name(&name) {
        return Err(AgentError::Provider(
            "remote parameter names must use the local identifier syntax".into(),
        ));
    }
    if document
        .parameters
        .values()
        .any(|parameter| parameter.name == name)
    {
        return Err(AgentError::Provider(
            "remote parameter creation cannot replace an existing parameter".into(),
        ));
    }
    let id = document.next_parameter_id();
    if id == u64::MAX {
        return Err(AgentError::Provider(
            "document parameter ID space is exhausted".into(),
        ));
    }
    let parameter = match (value, formula) {
        (Some(value), None) if value.is_finite() => {
            Parameter::literal(id, name, value, document.units)
        }
        (None, Some(formula)) => Parameter::formula(id, name, formula, document.units)
            .map_err(|_| AgentError::Provider("remote parameter formula is invalid".into()))?,
        _ => {
            return Err(AgentError::Provider(
                "remote parameter requires exactly one finite value or formula".into(),
            ));
        }
    };
    Ok((
        "set_parameter",
        CommandTransaction::new(vec![CadCommand::SetParameter { parameter }]),
    ))
}

#[allow(clippy::too_many_arguments)]
fn constrained_line_transaction(
    document: &CadDocument,
    layer: u64,
    name: String,
    start: [f64; 2],
    end: [f64; 2],
    horizontal: bool,
    vertical: bool,
    length: Option<String>,
) -> Result<(&'static str, CommandTransaction), AgentError> {
    if horizontal && vertical {
        return Err(AgentError::Provider(
            "a remote constrained line cannot be both horizontal and vertical".into(),
        ));
    }
    if !horizontal && !vertical && length.is_none() {
        return Err(AgentError::Provider(
            "a constrained line requires at least one supported constraint".into(),
        ));
    }
    let id = next_entity_id(document)?;
    let start = point(start)?;
    let end = point(end)?;
    let name = bounded_text(name, "entity name", MAX_NAME_BYTES)?;
    let start_reference = SketchPoint::new(id, cadx_core::PointAnchor::Start);
    let end_reference = SketchPoint::new(id, cadx_core::PointAnchor::End);
    let segment = SketchSegment::new(start_reference, end_reference);
    let mut commands = vec![CadCommand::CreateEntity {
        entity: Entity {
            id,
            layer,
            name,
            visible: true,
            kind: EntityKind::Line { start, end },
            parameter_refs: BTreeSet::new(),
        },
    }];
    let mut next_constraint_id = document.next_constraint_id();
    if horizontal {
        commands.push(CadCommand::CreateConstraint {
            constraint: next_constraint(
                &mut next_constraint_id,
                "Horizontal",
                ConstraintKind::Horizontal { segment },
            )?,
        });
    }
    if vertical {
        commands.push(CadCommand::CreateConstraint {
            constraint: next_constraint(
                &mut next_constraint_id,
                "Vertical",
                ConstraintKind::Vertical { segment },
            )?,
        });
    }
    if let Some(length) = length {
        let expression = ParameterExpression::new(length)
            .map_err(|_| AgentError::Provider("remote line length expression is invalid".into()))?;
        let parameter_refs = parameter_references(document, &expression)?;
        let CadCommand::CreateEntity { entity } = &mut commands[0] else {
            unreachable!("the first constrained-line command creates its entity");
        };
        entity.parameter_refs = parameter_refs;
        commands.push(CadCommand::CreateConstraint {
            constraint: next_constraint(
                &mut next_constraint_id,
                "Length",
                ConstraintKind::Distance {
                    first: start_reference,
                    second: end_reference,
                    value: expression,
                },
            )?,
        });
    }
    Ok(("create_constrained_line", CommandTransaction::new(commands)))
}

fn next_entity_id(document: &CadDocument) -> Result<EntityId, AgentError> {
    let id = document.next_entity_id();
    if id == EntityId::MAX {
        return Err(AgentError::Provider(
            "document entity ID space is exhausted".into(),
        ));
    }
    Ok(id)
}

fn next_constraint(
    next_id: &mut u64,
    name: &str,
    kind: ConstraintKind,
) -> Result<SketchConstraint, AgentError> {
    let id = *next_id;
    *next_id = next_id
        .checked_add(1)
        .filter(|id| *id != u64::MAX)
        .ok_or_else(|| AgentError::Provider("document constraint ID space is exhausted".into()))?;
    Ok(SketchConstraint {
        id,
        name: name.into(),
        driving: true,
        kind,
    })
}

fn parameter_references(
    document: &CadDocument,
    expression: &ParameterExpression,
) -> Result<BTreeSet<u64>, AgentError> {
    expression
        .dependencies()
        .map_err(|_| AgentError::Provider("remote line length expression is invalid".into()))?
        .into_iter()
        .map(|name| {
            document
                .parameters
                .values()
                .find(|parameter| parameter.name == name)
                .map(|parameter| parameter.id)
                .ok_or_else(|| {
                    AgentError::Provider(
                        "remote line length expression references an unavailable parameter".into(),
                    )
                })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCheck {
    name: String,
    detail: String,
    status: WireCheckStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireCheckStatus {
    Passed,
    Warning,
}

impl WireCheck {
    fn into_check(self) -> Result<CheckResult, AgentError> {
        Ok(CheckResult {
            name: bounded_text(self.name, "validation name", MAX_NAME_BYTES)?,
            status: match self.status {
                WireCheckStatus::Passed => CheckStatus::Passed,
                WireCheckStatus::Warning => CheckStatus::Warning,
            },
            detail: bounded_text(self.detail, "validation detail", MAX_TEXT_BYTES)?,
        })
    }
}

fn bounded_text(value: String, field: &str, max_bytes: usize) -> Result<String, AgentError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > max_bytes {
        return Err(AgentError::Provider(format!(
            "remote plan {field} must be non-empty and within the supported size"
        )));
    }
    Ok(value)
}

fn point(value: [f64; 2]) -> Result<Point2, AgentError> {
    if !value[0].is_finite() || !value[1].is_finite() {
        return Err(AgentError::Provider(
            "remote plan coordinates must be finite".into(),
        ));
    }
    Ok(Point2::new(value[0], value[1]))
}

fn positive_number(value: f64, field: &str) -> Result<f64, AgentError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(AgentError::Provider(format!(
            "remote plan {field} must be finite and positive"
        )));
    }
    Ok(value)
}

fn polygon_area(points: &[Point2]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let twice_area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(left, right)| left.x.mul_add(right.y, -left.y * right.x))
        .sum::<f64>();
    twice_area.abs() * 0.5
}

fn entity_tool_name(kind: &EntityKind) -> &'static str {
    match kind {
        EntityKind::Line { .. } => "create_line",
        EntityKind::Circle { .. } => "create_circle",
        EntityKind::Arc { .. } => "create_arc",
        EntityKind::AlignedDimension { .. } => "create_aligned_dimension",
        EntityKind::Rectangle { .. } => "create_rectangle",
        EntityKind::SketchProfile { .. } => "create_sketch_profile",
        EntityKind::Extrude { .. } => "create_extrude",
        EntityKind::Wall { .. } => "create_wall",
        EntityKind::Room { .. } => "create_room",
        EntityKind::Text { .. } => "create_text",
    }
}
