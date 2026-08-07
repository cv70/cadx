//! Occurrence placement math: `ITEM_DEFINED_TRANSFORMATION` and
//! `AXIS2_PLACEMENT_3D` reduced to a CADX [`AssemblyTransform`].

use cadx_core::assembly::AssemblyTransform;
use ruststep::ast::{EntityInstance, Parameter};

use crate::ExportError;

use super::{
    super::ast::{entity_by_id, parameter_list, parameter_ref},
    invalid, unique_record,
};

pub(super) fn item_defined_transform(
    entities: &[EntityInstance],
    transform_id: u64,
    millimeters_per_unit: f64,
) -> Result<AssemblyTransform, ExportError> {
    let entity = entity_by_id(entities, transform_id).ok_or_else(|| {
        invalid(format!(
            "assembly transformation references missing entity #{transform_id}"
        ))
    })?;
    let record = unique_record(entity, "ITEM_DEFINED_TRANSFORMATION")?.ok_or_else(|| {
        invalid(format!(
            "assembly transformation #{transform_id} is not ITEM_DEFINED_TRANSFORMATION"
        ))
    })?;
    let values = parameter_list(&record.parameter).ok_or_else(|| {
        invalid(format!(
            "ITEM_DEFINED_TRANSFORMATION #{transform_id} has invalid parameters"
        ))
    })?;
    let placements = values.iter().filter_map(parameter_ref).collect::<Vec<_>>();
    let [parent_id, child_id] = placements.as_slice() else {
        return Err(invalid(format!(
            "ITEM_DEFINED_TRANSFORMATION #{transform_id} requires two placement items"
        )));
    };
    let parent = axis_placement(entities, *parent_id, millimeters_per_unit)?;
    let child = axis_placement(entities, *child_id, millimeters_per_unit)?;
    Ok(parent.compose(child.inverse()))
}

fn axis_placement(
    entities: &[EntityInstance],
    placement_id: u64,
    millimeters_per_unit: f64,
) -> Result<AssemblyTransform, ExportError> {
    let entity = entity_by_id(entities, placement_id).ok_or_else(|| {
        invalid(format!(
            "assembly placement references missing entity #{placement_id}"
        ))
    })?;
    let record = unique_record(entity, "AXIS2_PLACEMENT_3D")?.ok_or_else(|| {
        invalid(format!(
            "assembly placement #{placement_id} is not AXIS2_PLACEMENT_3D"
        ))
    })?;
    let values = parameter_list(&record.parameter).ok_or_else(|| {
        invalid(format!(
            "AXIS2_PLACEMENT_3D #{placement_id} has invalid parameters"
        ))
    })?;
    let location_id = values.get(1).and_then(parameter_ref).ok_or_else(|| {
        invalid(format!(
            "AXIS2_PLACEMENT_3D #{placement_id} has no location"
        ))
    })?;
    let translation =
        point_coordinates(entities, location_id)?.map(|value| value * millimeters_per_unit);
    let z = match values.get(2).and_then(parameter_ref) {
        Some(direction) => direction_ratios(entities, direction)?,
        None => [0.0, 0.0, 1.0],
    };
    let reference_x = match values.get(3).and_then(parameter_ref) {
        Some(direction) => direction_ratios(entities, direction)?,
        None => [1.0, 0.0, 0.0],
    };
    let x_projection = dot(reference_x, z);
    let x = normalize(std::array::from_fn(|axis| {
        reference_x[axis] - x_projection * z[axis]
    }))
    .ok_or_else(|| {
        invalid(format!(
            "AXIS2_PLACEMENT_3D #{placement_id} has parallel axis directions"
        ))
    })?;
    let y = cross(z, x);
    Ok(AssemblyTransform {
        translation,
        rotation: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
    })
}

fn point_coordinates(entities: &[EntityInstance], point_id: u64) -> Result<[f64; 3], ExportError> {
    vector_values(entities, point_id, "CARTESIAN_POINT")
}

fn direction_ratios(
    entities: &[EntityInstance],
    direction_id: u64,
) -> Result<[f64; 3], ExportError> {
    normalize(vector_values(entities, direction_id, "DIRECTION")?).ok_or_else(|| {
        invalid(format!(
            "assembly direction #{direction_id} has zero magnitude"
        ))
    })
}

fn vector_values(
    entities: &[EntityInstance],
    id: u64,
    record_name: &str,
) -> Result<[f64; 3], ExportError> {
    let entity = entity_by_id(entities, id)
        .ok_or_else(|| invalid(format!("assembly references missing {record_name} #{id}")))?;
    let record = unique_record(entity, record_name)?
        .ok_or_else(|| invalid(format!("assembly entity #{id} is not {record_name}")))?;
    let values = parameter_list(&record.parameter)
        .and_then(|values| values.get(1))
        .and_then(parameter_list)
        .ok_or_else(|| {
            invalid(format!(
                "assembly {record_name} #{id} has invalid coordinates"
            ))
        })?;
    if values.len() != 3 {
        return Err(invalid(format!(
            "assembly {record_name} #{id} requires three coordinates"
        )));
    }
    let result = std::array::from_fn(|axis| parameter_number(&values[axis]).unwrap_or(f64::NAN));
    if result.into_iter().any(|value| !value.is_finite()) {
        return Err(invalid(format!(
            "assembly {record_name} #{id} contains non-finite coordinates"
        )));
    }
    Ok(result)
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left.into_iter().zip(right).map(|(a, b)| a * b).sum()
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn normalize(vector: [f64; 3]) -> Option<[f64; 3]> {
    let magnitude = dot(vector, vector).sqrt();
    (magnitude.is_finite() && magnitude > 1.0e-12).then(|| vector.map(|value| value / magnitude))
}

fn parameter_number(parameter: &Parameter) -> Option<f64> {
    match parameter {
        Parameter::Real(value) if value.is_finite() => Some(*value),
        Parameter::Integer(value) => i32::try_from(*value).ok().map(f64::from),
        Parameter::Typed { parameter, .. } => parameter_number(parameter),
        _ => None,
    }
}
