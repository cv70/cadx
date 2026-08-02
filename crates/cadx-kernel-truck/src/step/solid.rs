use cadx_core::{
    domain::{Feature, StepShellBoundary},
    kernel::KernelError,
};
use truck_meshalgo::prelude::*;
use truck_modeling::{InnerSpace, Invertible, Point3, Shell, Solid};
use truck_stepio::r#in::Table;

use crate::convert_compressed_shell;

struct BoundaryAnalysis {
    shell: Shell,
    polygon: PolygonMesh,
    volume: f64,
    min: [f64; 3],
    max: [f64; 3],
}

pub(crate) fn import_step_solid(
    feature: &Feature,
    source: &str,
    data_section: usize,
    shell_id: u64,
    void_shells: &[StepShellBoundary],
) -> Result<Solid, KernelError> {
    let exchange = truck_stepio::r#in::ruststep::parser::parse(source).map_err(|error| {
        KernelError::Evaluation {
            feature_id: feature.id,
            message: format!("STEP source could not be parsed: {error}"),
        }
    })?;
    let data = exchange
        .data
        .get(data_section)
        .ok_or_else(|| KernelError::Evaluation {
            feature_id: feature.id,
            message: format!("STEP source contains no DATA section at index {data_section}"),
        })?;
    let table = Table::from_data_section(data);
    let mut boundaries = Vec::with_capacity(void_shells.len() + 1);
    boundaries.push(import_step_shell(feature, &table, shell_id, true)?);
    for boundary in void_shells {
        boundaries.push(import_step_shell(
            feature,
            &table,
            boundary.shell_id,
            boundary.orientation,
        )?);
    }
    Solid::try_new(boundaries).map_err(|error| KernelError::Evaluation {
        feature_id: feature.id,
        message: format!("STEP solid boundaries are not a valid closed B-Rep: {error}"),
    })
}

fn import_step_shell(
    feature: &Feature,
    table: &Table,
    shell_id: u64,
    orientation: bool,
) -> Result<Shell, KernelError> {
    let shell = table
        .shell
        .get(&shell_id)
        .ok_or_else(|| KernelError::Evaluation {
            feature_id: feature.id,
            message: format!("STEP shell entity #{shell_id} could not be resolved"),
        })?;
    let compressed = table
        .to_compressed_shell(shell)
        .map_err(|error| KernelError::Evaluation {
            feature_id: feature.id,
            message: format!("STEP shell #{shell_id} could not be converted: {error}"),
        })?;
    let compressed = convert_compressed_shell(feature, compressed)?;
    let mut shell = Shell::extract(compressed).map_err(|error| KernelError::Evaluation {
        feature_id: feature.id,
        message: format!("STEP shell #{shell_id} is not a valid closed B-Rep: {error}"),
    })?;
    if !orientation {
        for face in shell.face_iter_mut() {
            face.invert();
        }
    }
    Ok(shell)
}

pub(crate) fn partition_step_export_solids(
    feature: &Feature,
    solid: &Solid,
    tolerance: f64,
) -> Result<Vec<Solid>, KernelError> {
    if solid.boundaries().len() <= 1 {
        return Ok(vec![solid.clone()]);
    }

    let mut boundaries = Vec::with_capacity(solid.boundaries().len());
    for shell in solid.boundaries() {
        let polygon = shell.triangulation(tolerance).to_polygon();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for point in polygon.positions() {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        if min.into_iter().chain(max).any(|value| !value.is_finite()) {
            return Err(KernelError::Exchange {
                format: "STEP",
                message: format!(
                    "feature {} has an empty or non-finite boundary tessellation",
                    feature.id
                ),
            });
        }
        let origin = Point3::new(
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        );
        let volume = stable_signed_volume(&polygon, origin);
        if !volume.is_finite() || volume.abs() <= tolerance.powi(3) {
            return Err(KernelError::Exchange {
                format: "STEP",
                message: format!(
                    "feature {} has a boundary shell with indeterminate orientation or volume",
                    feature.id
                ),
            });
        }
        boundaries.push(BoundaryAnalysis {
            shell: shell.clone(),
            polygon,
            volume,
            min,
            max,
        });
    }

    let outer_sign = boundaries
        .iter()
        .max_by(|left, right| left.volume.abs().total_cmp(&right.volume.abs()))
        .expect("multi-boundary solid has at least two boundaries")
        .volume
        .is_sign_positive();
    let outer_indices = boundaries
        .iter()
        .enumerate()
        .filter_map(|(index, boundary)| {
            (boundary.volume.is_sign_positive() == outer_sign).then_some(index)
        })
        .collect::<Vec<_>>();
    let mut grouped_voids = vec![Vec::new(); outer_indices.len()];

    for (void_index, void) in boundaries
        .iter()
        .enumerate()
        .filter(|(_, boundary)| boundary.volume.is_sign_positive() != outer_sign)
    {
        if void.polygon.positions().is_empty() {
            return Err(KernelError::Exchange {
                format: "STEP",
                message: format!("feature {} has an empty boundary tessellation", feature.id),
            });
        }
        let candidates = outer_indices
            .iter()
            .enumerate()
            .filter_map(|(group, outer_index)| {
                let outer = &boundaries[*outer_index];
                let bounds_contain = (0..3).all(|axis| {
                    void.min[axis] >= outer.min[axis] - tolerance
                        && void.max[axis] <= outer.max[axis] + tolerance
                });
                if !bounds_contain || void.volume.abs() >= outer.volume.abs() {
                    return None;
                }
                let mut polygon = outer.polygon.clone();
                if outer.volume.is_sign_negative() {
                    polygon.invert();
                }
                void.polygon
                    .positions()
                    .iter()
                    .all(|point| polygon.inside(*point))
                    .then_some((group, outer.volume.abs()))
            })
            .collect::<Vec<_>>();
        let group = match candidates.as_slice() {
            [(group, _)] => *group,
            [] => {
                return Err(KernelError::Exchange {
                    format: "STEP",
                    message: format!(
                        "feature {} has a void boundary that is not provably contained by an outer shell",
                        feature.id
                    ),
                });
            }
            _ => {
                return Err(KernelError::Exchange {
                    format: "STEP",
                    message: format!(
                        "feature {} has a void boundary with ambiguous outer-shell ownership",
                        feature.id
                    ),
                });
            }
        };
        grouped_voids[group].push(void_index);
    }

    outer_indices
        .into_iter()
        .zip(grouped_voids)
        .map(|(outer_index, void_indices)| {
            let mut shells = Vec::with_capacity(void_indices.len() + 1);
            shells.push(boundaries[outer_index].shell.clone());
            shells.extend(
                void_indices
                    .into_iter()
                    .map(|index| boundaries[index].shell.clone()),
            );
            Solid::try_new(shells).map_err(|error| KernelError::Exchange {
                format: "STEP",
                message: format!(
                    "feature {} could not be partitioned into STEP solids: {error}",
                    feature.id
                ),
            })
        })
        .collect()
}

fn stable_signed_volume(polygon: &PolygonMesh, origin: Point3) -> f64 {
    polygon
        .faces()
        .triangle_iter()
        .map(|triangle| {
            let a = polygon.positions()[triangle[0].pos] - origin;
            let b = polygon.positions()[triangle[1].pos] - origin;
            let c = polygon.positions()[triangle[2].pos] - origin;
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_core::domain::{CadDocument, ModelCommand};
    use truck_modeling::{Vector3, builder};

    fn box_solid(origin: Point3, size: f64) -> Solid {
        let vertex = builder::vertex(origin);
        let edge = builder::tsweep(&vertex, Vector3::new(size, 0.0, 0.0));
        let face = builder::tsweep(&edge, Vector3::new(0.0, size, 0.0));
        builder::tsweep(&face, Vector3::new(0.0, 0.0, size))
    }

    fn box_polygon(origin: Point3, size: f64) -> PolygonMesh {
        box_solid(origin, size).boundaries()[0]
            .triangulation(0.01)
            .to_polygon()
    }

    #[test]
    fn signed_boundary_volume_is_stable_far_from_the_origin() {
        for offset in [0.0, 1.0e9] {
            let polygon = box_polygon(Point3::new(offset, offset, offset), 10.0);
            let origin = Point3::new(offset + 5.0, offset + 5.0, offset + 5.0);
            assert!((stable_signed_volume(&polygon, origin) - 1_000.0).abs() < 1.0e-6);
        }
    }

    #[test]
    fn disjoint_opposite_oriented_shell_is_not_exported_as_a_void() {
        let outer = box_solid(Point3::origin(), 10.0)
            .into_boundaries()
            .pop()
            .unwrap();
        let mut disjoint = box_solid(Point3::new(30.0, 0.0, 0.0), 5.0)
            .into_boundaries()
            .pop()
            .unwrap();
        for face in disjoint.face_iter_mut() {
            face.invert();
        }
        let invalid = Solid::try_new(vec![outer, disjoint]).unwrap();
        let mut document = CadDocument::default();
        let feature_id = document
            .apply(ModelCommand::CreateBox {
                name: "export witness".into(),
                size: [1.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let error =
            partition_step_export_solids(document.feature(feature_id).unwrap(), &invalid, 0.01)
                .unwrap_err();
        assert!(matches!(
            error,
            KernelError::Exchange { message, .. }
                if message.contains("not provably contained")
        ));
    }

    #[test]
    fn void_contained_by_multiple_outer_shells_is_rejected_as_ambiguous() {
        let outer_a = box_solid(Point3::origin(), 10.0)
            .into_boundaries()
            .pop()
            .unwrap();
        let outer_b = box_solid(Point3::new(1.0, 0.0, 0.0), 10.0)
            .into_boundaries()
            .pop()
            .unwrap();
        let mut cavity = box_solid(Point3::new(3.0, 3.0, 3.0), 2.0)
            .into_boundaries()
            .pop()
            .unwrap();
        for face in cavity.face_iter_mut() {
            face.invert();
        }
        let ambiguous = Solid::try_new(vec![outer_a, outer_b, cavity]).unwrap();
        let mut document = CadDocument::default();
        let feature_id = document
            .apply(ModelCommand::CreateBox {
                name: "export witness".into(),
                size: [1.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let error =
            partition_step_export_solids(document.feature(feature_id).unwrap(), &ambiguous, 0.01)
                .unwrap_err();
        assert!(matches!(
            error,
            KernelError::Exchange { message, .. }
                if message.contains("ambiguous outer-shell ownership")
        ));
    }
}
