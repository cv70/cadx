//! Low-level STEP physical-file syntax access shared by the STEP adapter's
//! discovery passes.

use ruststep::ast::{EntityInstance, Exchange, Name, Parameter, Record};

use crate::ExportError;

pub(super) fn parse_exchange(source: &str) -> Result<Exchange, ExportError> {
    let exchange = ruststep::parser::parse(source)
        .map_err(|error| ExportError::InvalidStep(error.to_string()))?;
    if exchange
        .data
        .iter()
        .all(|section| section.entities.is_empty())
    {
        return Err(ExportError::InvalidStep(
            "STEP document contains no data entities".into(),
        ));
    }
    Ok(exchange)
}

pub(super) fn entity_id(entity: &EntityInstance) -> u64 {
    match entity {
        EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
    }
}

pub(super) fn entity_records(entity: &EntityInstance) -> Vec<&Record> {
    match entity {
        EntityInstance::Simple { record, .. } => vec![record],
        EntityInstance::Complex { subsuper, .. } => subsuper.0.iter().collect(),
    }
}

pub(super) fn entity_by_id(entities: &[EntityInstance], id: u64) -> Option<&EntityInstance> {
    entities.iter().find(|entity| entity_id(entity) == id)
}

pub(super) fn parameter_list(parameter: &Parameter) -> Option<&[Parameter]> {
    match parameter {
        Parameter::List(values) => Some(values),
        _ => None,
    }
}

pub(super) fn parameter_ref(parameter: &Parameter) -> Option<u64> {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => Some(*id),
        _ => None,
    }
}

pub(super) fn collect_entity_refs(parameter: &Parameter, refs: &mut Vec<u64>) {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => refs.push(*id),
        Parameter::List(values) => {
            for value in values {
                collect_entity_refs(value, refs);
            }
        }
        Parameter::Typed { parameter, .. } => collect_entity_refs(parameter, refs),
        _ => {}
    }
}
