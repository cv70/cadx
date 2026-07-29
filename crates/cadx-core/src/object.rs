use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    CadCommand, CadDocument, CommandTransaction, CommitId, ConstraintId, ConstraintKind, Entity,
    EntityId, EntityKind, LayerId, ParameterExpression, ParameterId, SketchConstraint,
};

/// A typed identity in the current compatibility model.
///
/// IDs remain document-local integers for now. The enum prevents collisions
/// between object kinds while the project migrates toward globally stable IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectId {
    Layer(LayerId),
    Entity(EntityId),
    Parameter(ParameterId),
    Constraint(ConstraintId),
}

pub(crate) fn transaction_objects(
    transaction: &CommandTransaction,
    document: &CadDocument,
) -> BTreeSet<ObjectId> {
    let parameters_by_name = document
        .parameters
        .values()
        .map(|parameter| (parameter.name.as_str(), parameter.id))
        .collect::<BTreeMap<_, _>>();
    let mut objects = BTreeSet::new();
    for command in &transaction.commands {
        match command {
            CadCommand::CreateLayer { layer } | CadCommand::UpdateLayer { layer } => {
                objects.insert(ObjectId::Layer(layer.id));
            }
            CadCommand::DeleteLayer { id } => {
                objects.insert(ObjectId::Layer(*id));
            }
            CadCommand::CreateEntity { entity } => {
                collect_entity_objects(entity, &mut objects);
            }
            CadCommand::UpdateEntity { entity } => {
                if let Some(previous) = document.entities.get(&entity.id) {
                    collect_entity_objects(previous, &mut objects);
                }
                collect_entity_objects(entity, &mut objects);
            }
            CadCommand::DeleteEntity { id } => {
                objects.insert(ObjectId::Entity(*id));
                if let Some(entity) = document.entities.get(id) {
                    collect_entity_objects(entity, &mut objects);
                }
            }
            CadCommand::SetParameter { parameter } => {
                objects.insert(ObjectId::Parameter(parameter.id));
                if let Some(expression) = &parameter.expression {
                    collect_expression_parameters(expression, &parameters_by_name, &mut objects);
                }
            }
            CadCommand::DeleteParameter { id } => {
                objects.insert(ObjectId::Parameter(*id));
            }
            CadCommand::CreateConstraint { constraint }
            | CadCommand::UpdateConstraint { constraint } => {
                collect_constraint_objects(constraint, &parameters_by_name, &mut objects);
            }
            CadCommand::DeleteConstraint { id } => {
                objects.insert(ObjectId::Constraint(*id));
            }
        }
    }
    objects
}

pub(crate) fn transaction_writes(transaction: &CommandTransaction) -> BTreeSet<ObjectId> {
    transaction
        .commands
        .iter()
        .map(|command| match command {
            CadCommand::CreateLayer { layer } | CadCommand::UpdateLayer { layer } => {
                ObjectId::Layer(layer.id)
            }
            CadCommand::DeleteLayer { id } => ObjectId::Layer(*id),
            CadCommand::CreateEntity { entity } | CadCommand::UpdateEntity { entity } => {
                ObjectId::Entity(entity.id)
            }
            CadCommand::DeleteEntity { id } => ObjectId::Entity(*id),
            CadCommand::SetParameter { parameter } => ObjectId::Parameter(parameter.id),
            CadCommand::DeleteParameter { id } => ObjectId::Parameter(*id),
            CadCommand::CreateConstraint { constraint }
            | CadCommand::UpdateConstraint { constraint } => ObjectId::Constraint(constraint.id),
            CadCommand::DeleteConstraint { id } => ObjectId::Constraint(*id),
        })
        .collect()
}

fn collect_entity_objects(entity: &Entity, objects: &mut BTreeSet<ObjectId>) {
    objects.insert(ObjectId::Entity(entity.id));
    objects.insert(ObjectId::Layer(entity.layer));
    objects.extend(
        entity
            .parameter_refs
            .iter()
            .copied()
            .map(ObjectId::Parameter),
    );
    if let EntityKind::Extrude { profile, .. } = &entity.kind {
        objects.insert(ObjectId::Entity(*profile));
    }
}

fn collect_constraint_objects(
    constraint: &SketchConstraint,
    parameters_by_name: &BTreeMap<&str, ParameterId>,
    objects: &mut BTreeSet<ObjectId>,
) {
    objects.insert(ObjectId::Constraint(constraint.id));
    match &constraint.kind {
        ConstraintKind::Coincident { first, second }
        | ConstraintKind::Distance {
            first,
            second,
            value: _,
        } => {
            objects.insert(ObjectId::Entity(first.entity_id));
            objects.insert(ObjectId::Entity(second.entity_id));
        }
        ConstraintKind::Horizontal { segment } | ConstraintKind::Vertical { segment } => {
            objects.insert(ObjectId::Entity(segment.start.entity_id));
            objects.insert(ObjectId::Entity(segment.end.entity_id));
        }
        ConstraintKind::Radius {
            entity_id,
            value: _,
        } => {
            objects.insert(ObjectId::Entity(*entity_id));
        }
    }
    match &constraint.kind {
        ConstraintKind::Distance { value, .. } | ConstraintKind::Radius { value, .. } => {
            collect_expression_parameters(value, parameters_by_name, objects);
        }
        ConstraintKind::Coincident { .. }
        | ConstraintKind::Horizontal { .. }
        | ConstraintKind::Vertical { .. } => {}
    }
}

fn collect_expression_parameters(
    expression: &ParameterExpression,
    parameters_by_name: &BTreeMap<&str, ParameterId>,
    objects: &mut BTreeSet<ObjectId>,
) {
    if let Ok(dependencies) = expression.dependencies() {
        objects.extend(
            dependencies
                .iter()
                .filter_map(|name| parameters_by_name.get(name.as_str()))
                .copied()
                .map(ObjectId::Parameter),
        );
    }
}

/// The state a prepared operation observed for one object.
///
/// `last_modified_revision` remains populated for deleted objects so an ABA
/// delete-and-recreate sequence cannot satisfy an older precondition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectPrecondition {
    pub object: ObjectId,
    pub exists: bool,
    pub last_modified_revision: Option<CommitId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectVersionIndex {
    states: BTreeMap<ObjectId, ObjectPrecondition>,
}

impl ObjectVersionIndex {
    pub(crate) fn from_root(document: &CadDocument) -> Self {
        let mut index = Self::default();
        for id in document.layers.keys().copied() {
            index.mark(ObjectId::Layer(id), true, 0);
        }
        for id in document.entities.keys().copied() {
            index.mark(ObjectId::Entity(id), true, 0);
        }
        for id in document.parameters.keys().copied() {
            index.mark(ObjectId::Parameter(id), true, 0);
        }
        for id in document.constraints.keys().copied() {
            index.mark(ObjectId::Constraint(id), true, 0);
        }
        index
    }

    pub(crate) fn apply_transaction(
        &mut self,
        revision: CommitId,
        transaction: &CommandTransaction,
    ) {
        for command in &transaction.commands {
            let (object, exists) = match command {
                CadCommand::CreateLayer { layer } | CadCommand::UpdateLayer { layer } => {
                    (ObjectId::Layer(layer.id), true)
                }
                CadCommand::DeleteLayer { id } => (ObjectId::Layer(*id), false),
                CadCommand::CreateEntity { entity } | CadCommand::UpdateEntity { entity } => {
                    (ObjectId::Entity(entity.id), true)
                }
                CadCommand::DeleteEntity { id } => (ObjectId::Entity(*id), false),
                CadCommand::SetParameter { parameter } => (ObjectId::Parameter(parameter.id), true),
                CadCommand::DeleteParameter { id } => (ObjectId::Parameter(*id), false),
                CadCommand::CreateConstraint { constraint }
                | CadCommand::UpdateConstraint { constraint } => {
                    (ObjectId::Constraint(constraint.id), true)
                }
                CadCommand::DeleteConstraint { id } => (ObjectId::Constraint(*id), false),
            };
            self.mark(object, exists, revision);
        }
    }

    pub(crate) fn precondition(&self, object: ObjectId) -> ObjectPrecondition {
        self.states
            .get(&object)
            .copied()
            .unwrap_or(ObjectPrecondition {
                object,
                exists: false,
                last_modified_revision: None,
            })
    }

    fn mark(&mut self, object: ObjectId, exists: bool, revision: CommitId) {
        self.states.insert(
            object,
            ObjectPrecondition {
                object,
                exists,
                last_modified_revision: Some(revision),
            },
        );
    }
}
