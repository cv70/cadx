use serde::{Deserialize, Serialize};

use crate::constraint::{SketchConstraint, validate_constraint};
use crate::document::{CadDocument, CommandError, Entity, Layer, Parameter, validate_entity};
use crate::expression::is_valid_parameter_name;
use crate::{ConstraintId, EntityId, LayerId, ParameterId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CadCommand {
    CreateLayer { layer: Layer },
    UpdateLayer { layer: Layer },
    DeleteLayer { id: LayerId },
    CreateEntity { entity: Entity },
    UpdateEntity { entity: Entity },
    DeleteEntity { id: EntityId },
    SetParameter { parameter: Parameter },
    DeleteParameter { id: ParameterId },
    CreateConstraint { constraint: SketchConstraint },
    UpdateConstraint { constraint: SketchConstraint },
    DeleteConstraint { id: ConstraintId },
}

impl CadCommand {
    pub fn validate(&self, document: &CadDocument) -> Result<(), CommandError> {
        match self {
            Self::CreateLayer { layer } => {
                validate_layer(layer, document, None)?;
                if document.layers.contains_key(&layer.id) {
                    return Err(CommandError::LayerExists(layer.id));
                }
                Ok(())
            }
            Self::UpdateLayer { layer } => {
                if !document.layers.contains_key(&layer.id) {
                    return Err(CommandError::LayerMissing(layer.id));
                }
                validate_layer(layer, document, Some(layer.id))
            }
            Self::DeleteLayer { id } => {
                if !document.layers.contains_key(id) {
                    return Err(CommandError::LayerMissing(*id));
                }
                if document.layers.len() == 1 {
                    return Err(CommandError::InvalidLayer(
                        "a document must retain at least one layer".into(),
                    ));
                }
                if document.entities.values().any(|entity| entity.layer == *id) {
                    return Err(CommandError::InvalidLayer(format!(
                        "layer {id} must be empty before it can be deleted"
                    )));
                }
                Ok(())
            }
            Self::CreateEntity { entity } => {
                if document.entities.contains_key(&entity.id) {
                    return Err(CommandError::EntityExists(entity.id));
                }
                validate_entity(entity, document)?;
                require_unlocked_layer(document, entity.layer)
            }
            Self::UpdateEntity { entity } => {
                let existing = document
                    .entities
                    .get(&entity.id)
                    .ok_or(CommandError::EntityMissing(entity.id))?;
                require_unlocked_layer(document, existing.layer)?;
                validate_entity(entity, document)?;
                require_unlocked_layer(document, entity.layer)
            }
            Self::DeleteEntity { id } => {
                let entity = document
                    .entities
                    .get(id)
                    .ok_or(CommandError::EntityMissing(*id))?;
                require_unlocked_layer(document, entity.layer)
            }
            Self::SetParameter { parameter } => {
                if parameter.id == ParameterId::MAX
                    || !is_valid_parameter_name(&parameter.name)
                    || !parameter.value.is_finite()
                {
                    return Err(CommandError::InvalidParameter(
                        "parameter name and value must be valid".into(),
                    ));
                }
                if let Some(expression) = &parameter.expression {
                    expression
                        .parse()
                        .map_err(|error| CommandError::InvalidParameter(error.to_string()))?;
                }
                Ok(())
            }
            Self::DeleteParameter { id } => {
                if document.parameters.contains_key(id) {
                    Ok(())
                } else {
                    Err(CommandError::InvalidParameter(format!(
                        "parameter {id} does not exist"
                    )))
                }
            }
            Self::CreateConstraint { constraint } => {
                if document.constraints.contains_key(&constraint.id) {
                    return Err(CommandError::ConstraintExists(constraint.id));
                }
                validate_constraint(constraint, document)
                    .map_err(|error| CommandError::InvalidConstraint(error.to_string()))
            }
            Self::UpdateConstraint { constraint } => {
                if !document.constraints.contains_key(&constraint.id) {
                    return Err(CommandError::ConstraintMissing(constraint.id));
                }
                validate_constraint(constraint, document)
                    .map_err(|error| CommandError::InvalidConstraint(error.to_string()))
            }
            Self::DeleteConstraint { id } => {
                if document.constraints.contains_key(id) {
                    Ok(())
                } else {
                    Err(CommandError::ConstraintMissing(*id))
                }
            }
        }
    }

    pub(crate) fn apply(&self, document: &mut CadDocument) {
        match self {
            Self::CreateLayer { layer } | Self::UpdateLayer { layer } => {
                document.next_layer_id = document
                    .next_layer_id
                    .max(layer.id.checked_add(1).expect("validated layer id"));
                document.layers.insert(layer.id, layer.clone());
            }
            Self::DeleteLayer { id } => {
                document.layers.remove(id);
            }
            Self::CreateEntity { entity } | Self::UpdateEntity { entity } => {
                document.next_entity_id = document
                    .next_entity_id
                    .max(entity.id.checked_add(1).expect("validated entity id"));
                document.entities.insert(entity.id, entity.clone());
            }
            Self::DeleteEntity { id } => {
                document.entities.remove(id);
            }
            Self::SetParameter { parameter } => {
                document.next_parameter_id = document
                    .next_parameter_id
                    .max(parameter.id.checked_add(1).expect("validated parameter id"));
                document.parameters.insert(parameter.id, parameter.clone());
            }
            Self::DeleteParameter { id } => {
                document.parameters.remove(id);
            }
            Self::CreateConstraint { constraint } | Self::UpdateConstraint { constraint } => {
                document.next_constraint_id = document.next_constraint_id.max(
                    constraint
                        .id
                        .checked_add(1)
                        .expect("validated constraint id"),
                );
                document
                    .constraints
                    .insert(constraint.id, constraint.clone());
            }
            Self::DeleteConstraint { id } => {
                document.constraints.remove(id);
            }
        }
    }

    fn label(&self) -> String {
        match self {
            Self::CreateLayer { layer } => format!("Create layer {}", layer.name),
            Self::UpdateLayer { layer } => format!("Update layer {}", layer.name),
            Self::DeleteLayer { id } => format!("Delete layer {id}"),
            Self::CreateEntity { entity } => format!("Create {}", entity.name),
            Self::UpdateEntity { entity } => format!("Update {}", entity.name),
            Self::DeleteEntity { id } => format!("Delete entity {id}"),
            Self::SetParameter { parameter } => format!("Set parameter {}", parameter.name),
            Self::DeleteParameter { id } => format!("Delete parameter {id}"),
            Self::CreateConstraint { constraint } => {
                format!("Create constraint {}", constraint.name)
            }
            Self::UpdateConstraint { constraint } => {
                format!("Update constraint {}", constraint.name)
            }
            Self::DeleteConstraint { id } => format!("Delete constraint {id}"),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentDiff {
    pub created_entities: Vec<EntityId>,
    pub updated_entities: Vec<EntityId>,
    pub deleted_entities: Vec<EntityId>,
    pub created_layers: Vec<LayerId>,
    pub updated_layers: Vec<LayerId>,
    pub deleted_layers: Vec<LayerId>,
    pub updated_parameters: Vec<ParameterId>,
    pub deleted_parameters: Vec<ParameterId>,
    pub created_constraints: Vec<ConstraintId>,
    pub updated_constraints: Vec<ConstraintId>,
    pub deleted_constraints: Vec<ConstraintId>,
}

impl DocumentDiff {
    pub fn is_empty(&self) -> bool {
        self.created_entities.is_empty()
            && self.updated_entities.is_empty()
            && self.deleted_entities.is_empty()
            && self.created_layers.is_empty()
            && self.updated_layers.is_empty()
            && self.deleted_layers.is_empty()
            && self.updated_parameters.is_empty()
            && self.deleted_parameters.is_empty()
            && self.created_constraints.is_empty()
            && self.updated_constraints.is_empty()
            && self.deleted_constraints.is_empty()
    }

    pub fn summary(&self) -> String {
        let changes = self.created_entities.len()
            + self.updated_entities.len()
            + self.deleted_entities.len()
            + self.created_layers.len()
            + self.updated_layers.len()
            + self.deleted_layers.len()
            + self.updated_parameters.len()
            + self.deleted_parameters.len()
            + self.created_constraints.len()
            + self.updated_constraints.len()
            + self.deleted_constraints.len();
        format!("{changes} model changes")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandTransaction {
    pub commands: Vec<CadCommand>,
}

impl CommandTransaction {
    pub fn new(commands: Vec<CadCommand>) -> Self {
        Self { commands }
    }

    pub fn preview(&self, document: &CadDocument) -> Result<DocumentDiff, CommandError> {
        let mut temporary = document.clone();
        let mut diff = DocumentDiff::default();
        for command in &self.commands {
            command.validate(&temporary)?;
            collect_diff(&mut diff, command, &temporary);
            command.apply(&mut temporary);
        }
        temporary.validate()?;
        Ok(diff)
    }

    pub fn apply(&self, document: &mut CadDocument) -> Result<DocumentDiff, CommandError> {
        let mut temporary = document.clone();
        let diff = self.preview(&temporary)?;
        for command in &self.commands {
            command.apply(&mut temporary);
        }
        *document = temporary;
        Ok(diff)
    }

    pub fn label(&self) -> String {
        match self.commands.as_slice() {
            [] => "No changes".into(),
            [command] => command.label(),
            commands => format!("{} actions", commands.len()),
        }
    }
}

fn collect_diff(diff: &mut DocumentDiff, command: &CadCommand, document: &CadDocument) {
    match command {
        CadCommand::CreateLayer { layer } => diff.created_layers.push(layer.id),
        CadCommand::UpdateLayer { layer } => diff.updated_layers.push(layer.id),
        CadCommand::DeleteLayer { id } => diff.deleted_layers.push(*id),
        CadCommand::CreateEntity { entity } => diff.created_entities.push(entity.id),
        CadCommand::UpdateEntity { entity } => {
            if document.entities.contains_key(&entity.id) {
                diff.updated_entities.push(entity.id);
            }
        }
        CadCommand::DeleteEntity { id } => diff.deleted_entities.push(*id),
        CadCommand::SetParameter { parameter } => diff.updated_parameters.push(parameter.id),
        CadCommand::DeleteParameter { id } => diff.deleted_parameters.push(*id),
        CadCommand::CreateConstraint { constraint } => diff.created_constraints.push(constraint.id),
        CadCommand::UpdateConstraint { constraint } => {
            if document.constraints.contains_key(&constraint.id) {
                diff.updated_constraints.push(constraint.id);
            }
        }
        CadCommand::DeleteConstraint { id } => diff.deleted_constraints.push(*id),
    }
}

fn validate_layer(
    layer: &Layer,
    document: &CadDocument,
    replacing: Option<LayerId>,
) -> Result<(), CommandError> {
    if layer.id == LayerId::MAX || layer.name.trim().is_empty() {
        return Err(CommandError::InvalidLayer(
            "layer id and name must be valid".into(),
        ));
    }
    if document.layers.values().any(|candidate| {
        Some(candidate.id) != replacing
            && candidate
                .name
                .trim()
                .eq_ignore_ascii_case(layer.name.trim())
    }) {
        return Err(CommandError::InvalidLayer(format!(
            "layer name {} is not unique",
            layer.name
        )));
    }
    Ok(())
}

fn require_unlocked_layer(document: &CadDocument, id: LayerId) -> Result<(), CommandError> {
    let layer = document
        .layers
        .get(&id)
        .ok_or(CommandError::LayerMissing(id))?;
    if layer.locked {
        Err(CommandError::LayerLocked(id))
    } else {
        Ok(())
    }
}
