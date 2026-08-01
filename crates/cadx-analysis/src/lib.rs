//! Read-only engineering analysis over the kernel-neutral CADX scene.
//!
//! The analysis layer deliberately consumes [`cadx_core::kernel::EvaluatedScene`]
//! instead of a concrete B-Rep. This keeps mass properties, AI context, and
//! future manufacturing checks independent from Truck or any other kernel.

mod measurement;

pub use measurement::{
    LengthPrecision, MeasurementEntity, MeasurementEntityKind, MeasurementError, MeasurementResult,
    measure,
};

use cadx_core::{
    domain::{FeatureId, MAX_MATERIAL_DENSITY_KG_M3, Material},
    kernel::{EvaluatedPart, EvaluatedScene},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MM3_PER_M3: f64 = 1.0e9;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AnalysisError {
    #[error("material density must be finite and non-negative")]
    InvalidDensity,
    #[error("part {feature_id} has no triangles")]
    EmptyPart { feature_id: FeatureId },
    #[error("part {feature_id} contains an out-of-bounds triangle index")]
    InvalidMesh { feature_id: FeatureId },
    #[error("part {feature_id} has non-finite mesh coordinates")]
    NonFiniteGeometry { feature_id: FeatureId },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl BoundingBox {
    #[must_use]
    pub fn size(self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartAnalysis {
    pub feature_id: FeatureId,
    pub name: String,
    pub triangle_count: usize,
    pub surface_area_mm2: f64,
    /// Signed mesh volume is normalized to a positive value for closed solids.
    pub volume_mm3: f64,
    pub centroid_mm: [f64; 3],
    pub bounds: BoundingBox,
    /// Persisted material metadata, independent of any density override.
    pub material: Option<Material>,
    /// Effective density, using the explicit override before part material.
    pub density_kg_m3: Option<f64>,
    pub mass_kg: Option<f64>,
    /// Inertia tensor about this part's centroid, in kg mm^2.
    pub inertia_centroid_kg_mm2: Option<[[f64; 3]; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneAnalysis {
    pub parts: Vec<PartAnalysis>,
    pub total_surface_area_mm2: f64,
    pub total_volume_mm3: f64,
    pub total_mass_kg: Option<f64>,
    pub center_of_mass_mm: Option<[f64; 3]>,
    /// Inertia tensor about the complete scene's center of mass, in kg mm^2.
    pub inertia_centroid_kg_mm2: Option<[[f64; 3]; 3]>,
}

impl Default for SceneAnalysis {
    fn default() -> Self {
        Self {
            parts: Vec::new(),
            total_surface_area_mm2: 0.0,
            total_volume_mm3: 0.0,
            total_mass_kg: None,
            center_of_mass_mm: None,
            inertia_centroid_kg_mm2: None,
        }
    }
}

impl SceneAnalysis {
    #[must_use]
    pub fn part(&self, feature_id: FeatureId) -> Option<&PartAnalysis> {
        self.parts.iter().find(|part| part.feature_id == feature_id)
    }
}

/// Computes deterministic engineering metrics from every visible scene part.
///
/// `density_kg_m3` overrides all part materials when present. Without an
/// override, each part uses its persisted material density. Per-part geometry
/// is always returned, but aggregate mass properties are absent when any part
/// lacks material. Meshes must be closed and consistently wound for volume and
/// inertia to be physically meaningful.
///
/// # Errors
///
/// Returns [`AnalysisError::InvalidDensity`] for an invalid material density,
/// or a mesh error when a part contains no triangles, invalid indices, or
/// non-finite coordinates.
pub fn analyze_scene(
    scene: &EvaluatedScene,
    density_override_kg_m3: Option<f64>,
) -> Result<SceneAnalysis, AnalysisError> {
    if density_override_kg_m3.is_some_and(|density| !density.is_finite() || density < 0.0) {
        return Err(AnalysisError::InvalidDensity);
    }

    let mut parts = Vec::with_capacity(scene.parts.len());
    for part in &scene.parts {
        parts.push(analyze_part(part, density_override_kg_m3)?);
    }
    let total_surface_area_mm2 = parts.iter().map(|part| part.surface_area_mm2).sum();
    let total_volume_mm3 = parts.iter().map(|part| part.volume_mm3).sum();
    let mass_properties_complete = density_override_kg_m3.is_some()
        || (!parts.is_empty() && parts.iter().all(|part| part.mass_kg.is_some()));
    let total_mass_kg =
        mass_properties_complete.then(|| parts.iter().filter_map(|part| part.mass_kg).sum::<f64>());
    let center_of_mass_mm = total_mass_kg
        .filter(|mass| *mass > f64::EPSILON)
        .map(|total_mass| {
            let mut numerator = [0.0; 3];
            for part in &parts {
                let mass = part.mass_kg.unwrap_or_default();
                for (axis, value) in numerator.iter_mut().enumerate() {
                    *value += part.centroid_mm[axis] * mass;
                }
            }
            numerator.map(|value| value / total_mass)
        });
    let inertia_centroid_kg_mm2 = center_of_mass_mm.and_then(|center| {
        let mut total = [[0.0; 3]; 3];
        for part in &parts {
            let inertia = part.inertia_centroid_kg_mm2?;
            let mass = part.mass_kg?;
            add_matrix(&mut total, inertia);
            add_matrix(
                &mut total,
                parallel_axis(mass, sub(part.centroid_mm, center)),
            );
        }
        Some(total)
    });
    Ok(SceneAnalysis {
        parts,
        total_surface_area_mm2,
        total_volume_mm3,
        total_mass_kg,
        center_of_mass_mm,
        inertia_centroid_kg_mm2,
    })
}

fn analyze_part(
    part: &EvaluatedPart,
    density_override_kg_m3: Option<f64>,
) -> Result<PartAnalysis, AnalysisError> {
    if part.mesh.indices.is_empty() {
        return Err(AnalysisError::EmptyPart {
            feature_id: part.feature_id,
        });
    }
    if !part.mesh.indices.len().is_multiple_of(3) {
        return Err(AnalysisError::InvalidMesh {
            feature_id: part.feature_id,
        });
    }
    let integration_origin = point(
        part.mesh
            .positions
            .first()
            .copied()
            .ok_or(AnalysisError::InvalidMesh {
                feature_id: part.feature_id,
            })?,
        part.feature_id,
    )?;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut area = 0.0;
    let mut signed_volume = 0.0;
    let mut centroid_numerator = [0.0; 3];
    let mut second_moments = [[0.0; 3]; 3];

    for triangle in part.mesh.indices.chunks_exact(3) {
        let a = point(
            part.mesh
                .positions
                .get(triangle[0] as usize)
                .copied()
                .ok_or(AnalysisError::InvalidMesh {
                    feature_id: part.feature_id,
                })?,
            part.feature_id,
        )?;
        let b = point(
            part.mesh
                .positions
                .get(triangle[1] as usize)
                .copied()
                .ok_or(AnalysisError::InvalidMesh {
                    feature_id: part.feature_id,
                })?,
            part.feature_id,
        )?;
        let c = point(
            part.mesh
                .positions
                .get(triangle[2] as usize)
                .copied()
                .ok_or(AnalysisError::InvalidMesh {
                    feature_id: part.feature_id,
                })?,
            part.feature_id,
        )?;
        for point in [a, b, c] {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        let ab = sub(b, a);
        let ac = sub(c, a);
        let area_cross = cross(ab, ac);
        area += 0.5 * length(area_cross);
        let a = sub(a, integration_origin);
        let b = sub(b, integration_origin);
        let c = sub(c, integration_origin);
        let tetra_volume = dot(a, cross(b, c)) / 6.0;
        signed_volume += tetra_volume;
        for axis in 0..3 {
            centroid_numerator[axis] += (a[axis] + b[axis] + c[axis]) * tetra_volume / 4.0;
            second_moments[axis][axis] += tetra_volume / 10.0
                * (a[axis] * a[axis]
                    + b[axis] * b[axis]
                    + c[axis] * c[axis]
                    + a[axis] * b[axis]
                    + a[axis] * c[axis]
                    + b[axis] * c[axis]);
        }
        for (first, second) in [(0, 1), (0, 2), (1, 2)] {
            let integral = tetra_volume / 20.0
                * ((a[first] + b[first] + c[first]) * (a[second] + b[second] + c[second])
                    + a[first] * a[second]
                    + b[first] * b[second]
                    + c[first] * c[second]);
            second_moments[first][second] += integral;
            second_moments[second][first] += integral;
        }
    }

    let volume_mm3 = signed_volume.abs();
    let centroid_local = if signed_volume.abs() > f64::EPSILON {
        centroid_numerator.map(|value| value / signed_volume)
    } else {
        // Open meshes have no meaningful solid centroid. The average of their
        // vertices is still a useful, deterministic inspection point.
        let mut sum = [0.0; 3];
        for position in &part.mesh.positions {
            let position = point(*position, part.feature_id)?;
            for axis in 0..3 {
                sum[axis] += position[axis];
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let count = part.mesh.positions.len() as f64;
        sub(sum.map(|value| value / count), integration_origin)
    };
    let centroid_mm = add(centroid_local, integration_origin);
    let material = part.material.clone();
    if material.as_ref().is_some_and(|material| {
        !material.density_kg_m3.is_finite()
            || material.density_kg_m3 <= 0.0
            || material.density_kg_m3 > MAX_MATERIAL_DENSITY_KG_M3
    }) {
        return Err(AnalysisError::InvalidDensity);
    }
    let density_kg_m3 =
        density_override_kg_m3.or_else(|| material.as_ref().map(|material| material.density_kg_m3));
    let mass_kg = density_kg_m3.map(|density| volume_mm3 / MM3_PER_M3 * density);
    let inertia_centroid_kg_mm2 = density_kg_m3.and_then(|density| {
        (volume_mm3 > f64::EPSILON).then(|| {
            let orientation = signed_volume.signum();
            for row in &mut second_moments {
                for value in row {
                    *value *= orientation;
                }
            }
            let density_in_model_units = density / MM3_PER_M3;
            let mut inertia_origin = [[0.0; 3]; 3];
            inertia_origin[0][0] =
                density_in_model_units * (second_moments[1][1] + second_moments[2][2]);
            inertia_origin[1][1] =
                density_in_model_units * (second_moments[0][0] + second_moments[2][2]);
            inertia_origin[2][2] =
                density_in_model_units * (second_moments[0][0] + second_moments[1][1]);
            for (first, second) in [(0, 1), (0, 2), (1, 2)] {
                let product = -density_in_model_units * second_moments[first][second];
                inertia_origin[first][second] = product;
                inertia_origin[second][first] = product;
            }
            let mut inertia_centroid = inertia_origin;
            subtract_matrix(
                &mut inertia_centroid,
                parallel_axis(volume_mm3 / MM3_PER_M3 * density, centroid_local),
            );
            inertia_centroid
        })
    });
    Ok(PartAnalysis {
        feature_id: part.feature_id,
        name: part.name.clone(),
        triangle_count: part.mesh.triangle_count(),
        surface_area_mm2: area,
        volume_mm3,
        centroid_mm,
        bounds: BoundingBox { min, max },
        material,
        density_kg_m3,
        mass_kg,
        inertia_centroid_kg_mm2,
    })
}

fn parallel_axis(mass: f64, offset: [f64; 3]) -> [[f64; 3]; 3] {
    let squared_length = dot(offset, offset);
    let mut matrix = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            let identity = f64::from(row == column);
            matrix[row][column] = mass * (squared_length * identity - offset[row] * offset[column]);
        }
    }
    matrix
}

fn add_matrix(target: &mut [[f64; 3]; 3], value: [[f64; 3]; 3]) {
    for row in 0..3 {
        for column in 0..3 {
            target[row][column] += value[row][column];
        }
    }
}

fn subtract_matrix(target: &mut [[f64; 3]; 3], value: [[f64; 3]; 3]) {
    for row in 0..3 {
        for column in 0..3 {
            target[row][column] -= value[row][column];
        }
    }
}

fn point(position: [f32; 3], feature_id: FeatureId) -> Result<[f64; 3], AnalysisError> {
    let point = position.map(f64::from);
    if point.iter().all(|value| value.is_finite()) {
        Ok(point)
    } else {
        Err(AnalysisError::NonFiniteGeometry { feature_id })
    }
}

fn sub(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn add(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn length(value: [f64; 3]) -> f64 {
    dot(value, value).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_core::kernel::{EvaluatedPart, TriangleMesh};
    use cadx_core::topology::{EvaluatedFace, FaceGeometry, FaceRef, PrimitiveFace, SurfaceKind};

    fn cube_scene() -> EvaluatedScene {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let mesh = TriangleMesh {
            positions,
            normals: Vec::new(),
            indices: vec![
                0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1, 6, 5, 0, 1, 5, 0,
                5, 4, 3, 7, 6, 3, 6, 2,
            ],
        };
        let face = EvaluatedFace {
            reference: FaceRef::primitive(1, PrimitiveFace::BoxXMin),
            geometry: FaceGeometry {
                surface: SurfaceKind::Plane,
                plane: None,
                area: 1.0,
                centroid: [0.0; 3],
                mean_normal: [0.0, 0.0, 1.0],
            },
            triangles: 0..12,
        };
        EvaluatedScene {
            parts: vec![EvaluatedPart {
                feature_id: 1,
                name: "cube".into(),
                color: [1.0; 4],
                material: None,
                mesh,
                faces: vec![face],
                edges: Vec::new(),
                vertices: Vec::new(),
            }],
            ..EvaluatedScene::default()
        }
    }

    #[test]
    fn computes_cube_metrics_and_density() {
        let analysis = analyze_scene(&cube_scene(), Some(1_000.0)).unwrap();
        let part = &analysis.parts[0];
        assert!((part.volume_mm3 - 1.0).abs() < 1.0e-6);
        assert!((part.surface_area_mm2 - 6.0).abs() < 1.0e-6);
        assert!(
            part.bounds
                .size()
                .iter()
                .all(|value| (*value - 1.0).abs() < 1.0e-6)
        );
        assert!(
            part.centroid_mm
                .iter()
                .all(|value| (*value - 0.5).abs() < 1.0e-6)
        );
        assert_eq!(part.density_kg_m3, Some(1_000.0));
        assert!((part.mass_kg.unwrap() - 1.0e-6).abs() < 1.0e-12);
        assert!(
            analysis
                .center_of_mass_mm
                .unwrap()
                .iter()
                .all(|value| (*value - 0.5).abs() < 1.0e-12)
        );
        let inertia = part.inertia_centroid_kg_mm2.unwrap();
        for (axis, row) in inertia.iter().enumerate() {
            assert!((row[axis] - 1.0e-6 / 6.0).abs() < 1.0e-12);
        }
    }

    #[test]
    fn uses_part_materials_and_requires_complete_assignment_for_scene_mass() {
        let mut scene = cube_scene();
        scene.parts[0].material = Some(Material {
            name: "Water".into(),
            density_kg_m3: 1_000.0,
        });
        let assigned = analyze_scene(&scene, None).unwrap();
        assert_eq!(assigned.parts[0].material, scene.parts[0].material);
        assert_eq!(assigned.total_mass_kg, Some(1.0e-6));
        assert!(assigned.inertia_centroid_kg_mm2.is_some());

        let mut unassigned = scene.parts[0].clone();
        unassigned.feature_id = 2;
        unassigned.material = None;
        scene.parts.push(unassigned);
        let incomplete = analyze_scene(&scene, None).unwrap();
        assert!(incomplete.parts[0].mass_kg.is_some());
        assert!(incomplete.parts[1].mass_kg.is_none());
        assert!(incomplete.total_mass_kg.is_none());
        assert!(incomplete.center_of_mass_mm.is_none());
        assert!(incomplete.inertia_centroid_kg_mm2.is_none());
    }

    #[test]
    fn inertia_is_orientation_independent_and_uses_parallel_axis_theorem() {
        let mut scene = cube_scene();
        for triangle in scene.parts[0].mesh.indices.chunks_exact_mut(3) {
            triangle.swap(1, 2);
        }
        let reversed = analyze_scene(&scene, Some(1_000.0)).unwrap();
        let inertia = reversed.parts[0].inertia_centroid_kg_mm2.unwrap();
        for (axis, row) in inertia.iter().enumerate() {
            assert!((row[axis] - 1.0e-6 / 6.0).abs() < 1.0e-12);
        }

        let mut second = scene.parts[0].clone();
        second.feature_id = 2;
        for position in &mut second.mesh.positions {
            position[0] += 2.0;
        }
        scene.parts.push(second);
        let combined = analyze_scene(&scene, Some(1_000.0)).unwrap();
        let center = combined.center_of_mass_mm.unwrap();
        assert!((center[0] - 1.5).abs() < 1.0e-12);
        assert!((center[1] - 0.5).abs() < 1.0e-12);
        assert!((center[2] - 0.5).abs() < 1.0e-12);
        let inertia = combined.inertia_centroid_kg_mm2.unwrap();
        assert!((inertia[0][0] - 2.0e-6 / 6.0).abs() < 1.0e-12);
        for axis in [1, 2] {
            assert!((inertia[axis][axis] - 2.0 * (1.0e-6 / 6.0 + 1.0e-6)).abs() < 1.0e-12);
        }

        let mut translated = cube_scene();
        for position in &mut translated.parts[0].mesh.positions {
            position[0] += 50_000.0;
            position[1] -= 40_000.0;
            position[2] += 30_000.0;
        }
        let translated = analyze_scene(&translated, Some(1_000.0)).unwrap();
        let translated_inertia = translated.parts[0].inertia_centroid_kg_mm2.unwrap();
        for (axis, row) in translated_inertia.iter().enumerate() {
            assert!((row[axis] - 1.0e-6 / 6.0).abs() < 1.0e-12);
        }
    }

    #[test]
    fn rejects_invalid_indices_and_non_finite_density() {
        let mut scene = cube_scene();
        scene.parts[0].mesh.indices[0] = 99;
        assert!(matches!(
            analyze_scene(&scene, None),
            Err(AnalysisError::InvalidMesh { .. })
        ));
        assert!(matches!(
            analyze_scene(&cube_scene(), Some(f64::NAN)),
            Err(AnalysisError::InvalidDensity)
        ));

        let mut scene = cube_scene();
        scene.parts[0].mesh.indices.push(0);
        assert!(matches!(
            analyze_scene(&scene, None),
            Err(AnalysisError::InvalidMesh { .. })
        ));
    }
}
