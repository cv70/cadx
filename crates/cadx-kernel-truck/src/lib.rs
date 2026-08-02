use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    f64::consts::{PI, TAU},
    fmt::Write as _,
    panic::{AssertUnwindSafe, catch_unwind},
};

use truck_meshalgo::prelude::*;
use truck_modeling::{
    BSplineCurve, Curve, Edge, Face, Invertible, KnotVec, Line, Matrix4, NurbsCurve, Plane, Point3,
    Processor, Rad, Shell, Solid, Surface, Transformed, Vector3, Vector4, Vertex, Wire, builder,
};
use truck_stepio::{
    r#in::{
        Table,
        alias::{Curve3D, ElementarySurface, Surface as StepSurface, SweptCurve},
    },
    out::{StepHeaderDescriptor, StepModels},
};
use truck_topology::compress::{
    CompressedEdge, CompressedEdgeIndex, CompressedFace, CompressedShell,
};

use cadx_core::{
    assembly::{AssemblyDefinitionBody, AssemblyTransform},
    diagnostics::{
        AxisAlignedBounds, BooleanDiagnostic, BooleanFailureReason, BooleanFailureStage,
        EdgeModifierDiagnostic, EdgeModifierFailureReason, EdgeModifierFailureStage,
        EdgeModifierOperation, EdgeModifierParameter,
    },
    domain::{
        BooleanOperation, CadDocument, Feature, FeatureId, Primitive, SketchLoop2D, SketchPlane,
        SketchRegion2D, SketchSegment2D, Vec3,
    },
    kernel::{
        CadKernel, CadKernelCapabilities, EdgeConvexitySupport, EdgeCountSupport, EdgeCurveSupport,
        EdgeModifierCapability, EvaluatedDatumPlane, EvaluatedDatumPoint, EvaluatedMeshDefinition,
        EvaluatedMeshInstance, EvaluatedPart, EvaluatedScene, EvaluatedSketch,
        EvaluatedSketchDiagnostic, ExchangeKernel, InterferenceAnalysis, InterferenceFailureReason,
        InterferenceFailureStage, InterferenceIntersectionMethod, InterferencePairAnalysis,
        InterferencePairOutcome, InterferenceVolumePrecision, KernelError, SharedVertexSupport,
        SourceFeatureScope, SupportSurfaceSupport, TriangleMesh,
    },
    tolerance::{BooleanTolerancePolicy, BooleanTolerancePolicyError},
    topology::{FaceRef, TopologyResolution},
};

mod boolean;
mod step;
mod topology;

use step::{
    AssemblyStepExportPlan, StepExportBodyOwner, append_ap242_product_structure, import_step_solid,
    partition_step_export_solids,
};
use topology::NamedSolid;

#[derive(Debug, Clone, Copy)]
pub struct TruckKernel {
    tolerance: f64,
    boolean_tolerance_policy: BooleanTolerancePolicy,
}

#[derive(Default)]
struct ImportedStepSolidCache {
    local_solids: HashMap<AssemblyDefinitionBody, Solid>,
}

impl ImportedStepSolidCache {
    fn instantiate(
        &mut self,
        key: Option<AssemblyDefinitionBody>,
        reconstruct: impl FnOnce() -> Result<Solid, KernelError>,
    ) -> Result<Solid, KernelError> {
        if let Some(solid) = key.and_then(|key| self.local_solids.get(&key)) {
            return Ok(builder::clone(solid));
        }

        let solid = reconstruct()?;
        #[cfg(test)]
        IMPORTED_STEP_RECONSTRUCTION_COUNT.with(|count| count.set(count.get() + 1));
        if let Some(key) = key {
            self.local_solids.insert(key, builder::clone(&solid));
        }
        Ok(solid)
    }
}

#[cfg(test)]
thread_local! {
    static IMPORTED_STEP_RECONSTRUCTION_COUNT: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn reset_imported_step_reconstruction_count() {
    IMPORTED_STEP_RECONSTRUCTION_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn imported_step_reconstruction_count() -> usize {
    IMPORTED_STEP_RECONSTRUCTION_COUNT.with(std::cell::Cell::get)
}

fn shared_imported_step_bodies(
    document: &CadDocument,
) -> HashMap<FeatureId, AssemblyDefinitionBody> {
    reusable_definition_bodies(document, |primitive| {
        matches!(primitive, Primitive::ImportedStep { .. })
    })
}

fn reusable_render_bodies(document: &CadDocument) -> HashMap<FeatureId, AssemblyDefinitionBody> {
    reusable_definition_bodies(document, |primitive| {
        matches!(
            primitive,
            Primitive::Box { .. }
                | Primitive::Cylinder { .. }
                | Primitive::Sphere { .. }
                | Primitive::Cone { .. }
                | Primitive::Torus { .. }
                | Primitive::Extrusion { .. }
                | Primitive::ImportedStep { .. }
        )
    })
}

fn reusable_definition_bodies(
    document: &CadDocument,
    eligible: impl Fn(&Primitive) -> bool,
) -> HashMap<FeatureId, AssemblyDefinitionBody> {
    let instances = document.assembly_feature_instances();
    let mut occurrences_by_definition = BTreeMap::new();
    for assembly in &document.assemblies {
        for occurrence in &assembly.occurrences {
            occurrences_by_definition
                .entry((assembly.id, occurrence.definition_id))
                .or_insert_with(Vec::new)
                .push(occurrence);
        }
    }

    let mut reusable = HashMap::new();
    for ((assembly_id, definition_id), occurrences) in occurrences_by_definition {
        let Some(first) = occurrences.first() else {
            continue;
        };
        let body_count = first.feature_ids.len();
        if occurrences.len() < 2
            || body_count == 0
            || occurrences
                .iter()
                .any(|occurrence| occurrence.feature_ids.len() != body_count)
        {
            continue;
        }
        let compatible = (0..body_count).all(|body_slot| {
            let Some(reference) = document
                .feature(first.feature_ids[body_slot])
                .map(|feature| &feature.primitive)
                .filter(|primitive| eligible(primitive))
            else {
                return false;
            };
            occurrences.iter().skip(1).all(|occurrence| {
                document
                    .feature(occurrence.feature_ids[body_slot])
                    .is_some_and(|feature| feature.primitive == *reference)
            })
        });
        if !compatible {
            continue;
        }

        for occurrence in occurrences {
            for feature_id in &occurrence.feature_ids {
                let Some(instance) = instances.get(feature_id) else {
                    continue;
                };
                debug_assert_eq!(instance.assembly_id, assembly_id);
                debug_assert_eq!(instance.definition_id, definition_id);
                reusable.insert(*feature_id, instance.definition_body());
            }
        }
    }
    reusable
}

fn instanced_render_geometry(
    document: &CadDocument,
    parts: &[EvaluatedPart],
    reusable_bodies: &HashMap<FeatureId, AssemblyDefinitionBody>,
) -> (Vec<EvaluatedMeshDefinition>, Vec<EvaluatedMeshInstance>) {
    let mut parts_by_definition = BTreeMap::<AssemblyDefinitionBody, Vec<&EvaluatedPart>>::new();
    for part in parts {
        if let Some(key) = reusable_bodies.get(&part.feature_id) {
            parts_by_definition.entry(*key).or_default().push(part);
        }
    }

    let mut definitions = Vec::new();
    let mut instances = Vec::new();
    for (key, grouped_parts) in parts_by_definition {
        if grouped_parts.len() < 2 {
            continue;
        }
        let Some(first_feature) = document.feature(grouped_parts[0].feature_id) else {
            continue;
        };
        let first_transform = AssemblyTransform::from_euler_xyz_degrees(
            first_feature.translation.as_array(),
            first_feature.rotation.as_array(),
        );
        definitions.push(EvaluatedMeshDefinition {
            key,
            mesh: transform_triangle_mesh(&grouped_parts[0].mesh, first_transform.inverse()),
        });
        instances.extend(grouped_parts.into_iter().filter_map(|part| {
            let feature = document.feature(part.feature_id)?;
            Some(EvaluatedMeshInstance {
                feature_id: part.feature_id,
                definition: key,
                transform: AssemblyTransform::from_euler_xyz_degrees(
                    feature.translation.as_array(),
                    feature.rotation.as_array(),
                ),
            })
        }));
    }
    (definitions, instances)
}

#[allow(clippy::cast_possible_truncation)]
fn transform_triangle_mesh(mesh: &TriangleMesh, transform: AssemblyTransform) -> TriangleMesh {
    TriangleMesh {
        positions: mesh
            .positions
            .iter()
            .map(|position| {
                transform
                    .transform_point(position.map(f64::from))
                    .map(|value| value as f32)
            })
            .collect(),
        normals: mesh
            .normals
            .iter()
            .map(|normal| {
                transform
                    .transform_vector(normal.map(f64::from))
                    .map(|value| value as f32)
            })
            .collect(),
        indices: mesh.indices.clone(),
    }
}

#[derive(Debug, Clone, Copy)]
struct SketchFrame {
    origin: [f64; 3],
    x_dir: [f64; 3],
    y_dir: [f64; 3],
    normal: [f64; 3],
}

impl SketchFrame {
    const WORLD_XY: Self = Self {
        origin: [0.0; 3],
        x_dir: [1.0, 0.0, 0.0],
        y_dir: [0.0, 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    };
    const WORLD_XZ: Self = Self {
        origin: [0.0; 3],
        x_dir: [1.0, 0.0, 0.0],
        y_dir: [0.0, 0.0, 1.0],
        normal: [0.0, -1.0, 0.0],
    };
    const WORLD_YZ: Self = Self {
        origin: [0.0; 3],
        x_dir: [0.0, 1.0, 0.0],
        y_dir: [0.0, 0.0, 1.0],
        normal: [1.0, 0.0, 0.0],
    };

    fn point(self, point: [f64; 2]) -> Point3 {
        Point3::new(
            point[0].mul_add(
                self.x_dir[0],
                point[1].mul_add(self.y_dir[0], self.origin[0]),
            ),
            point[0].mul_add(
                self.x_dir[1],
                point[1].mul_add(self.y_dir[1], self.origin[1]),
            ),
            point[0].mul_add(
                self.x_dir[2],
                point[1].mul_add(self.y_dir[2], self.origin[2]),
            ),
        )
    }

    fn direction(self, direction: [f64; 2]) -> Vector3 {
        Vector3::new(
            direction[0].mul_add(self.x_dir[0], direction[1] * self.y_dir[0]),
            direction[0].mul_add(self.x_dir[1], direction[1] * self.y_dir[1]),
            direction[0].mul_add(self.x_dir[2], direction[1] * self.y_dir[2]),
        )
    }

    fn point_array(self, point: [f64; 2]) -> [f64; 3] {
        let point = self.point(point);
        [point.x, point.y, point.z]
    }

    fn with_model_offset(mut self, offset: Vec3) -> Self {
        for axis in 0..3 {
            self.origin[axis] += offset.as_array()[axis];
        }
        self
    }

    fn with_local_transform(mut self, translation: Vec3, angle_degrees: f64) -> Self {
        let translation = translation.as_array();
        for axis in 0..3 {
            self.origin[axis] = translation[0].mul_add(
                self.x_dir[axis],
                translation[1].mul_add(
                    self.y_dir[axis],
                    translation[2].mul_add(self.normal[axis], self.origin[axis]),
                ),
            );
        }
        let (sin, cos) = angle_degrees.to_radians().sin_cos();
        let old_x = self.x_dir;
        let old_y = self.y_dir;
        self.x_dir = std::array::from_fn(|axis| cos.mul_add(old_x[axis], sin * old_y[axis]));
        self.y_dir = std::array::from_fn(|axis| (-sin).mul_add(old_x[axis], cos * old_y[axis]));
        self
    }
}

fn sketch_edge(
    frame: SketchFrame,
    segment: &SketchSegment2D,
    front: &Vertex,
    back: &Vertex,
) -> Edge {
    match segment {
        SketchSegment2D::Line { .. } => builder::line(front, back),
        SketchSegment2D::Arc { .. } => {
            builder::circle_arc(front, back, frame.point(segment.midpoint()))
        }
        SketchSegment2D::RationalQuadratic {
            start,
            control,
            end,
            weight,
        } => {
            let homogeneous = |point: [f64; 2], weight: f64| {
                let point = frame.point(point);
                Vector4::new(point.x * weight, point.y * weight, point.z * weight, weight)
            };
            let curve = BSplineCurve::new(
                KnotVec::bezier_knot(2),
                vec![
                    homogeneous(*start, 1.0),
                    homogeneous(*control, *weight),
                    homogeneous(*end, 1.0),
                ],
            );
            Edge::new(front, back, Curve::NurbsCurve(NurbsCurve::new(curve)))
        }
        SketchSegment2D::CubicBezier {
            control1, control2, ..
        } => builder::bezier(
            front,
            back,
            vec![frame.point(*control1), frame.point(*control2)],
        ),
    }
}

fn sketch_wire(frame: SketchFrame, profile: &SketchLoop2D) -> Wire {
    let vertices = profile
        .segments
        .iter()
        .map(SketchSegment2D::start)
        .map(|point| builder::vertex(frame.point(point)))
        .collect::<Vec<_>>();
    let edges = profile
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            sketch_edge(
                frame,
                segment,
                &vertices[index],
                &vertices[(index + 1) % vertices.len()],
            )
        })
        .collect::<Vec<_>>();
    Wire::from(edges)
}

fn loft_profile_centroid(frame: SketchFrame, profile: &SketchLoop2D) -> Option<Point3> {
    let count = u32::try_from(profile.segments.len()).ok()?;
    let sum = profile
        .segments
        .iter()
        .map(SketchSegment2D::start)
        .map(|point| frame.point(point) - Point3::origin())
        .fold(Vector3::zero(), |sum, point| sum + point);
    Some(Point3::origin() + sum / f64::from(count))
}

fn build_loft_solid(
    feature: &Feature,
    frames: &[SketchFrame],
    profiles: &[SketchLoop2D],
    offset: Vec3,
    tolerance: f64,
) -> Result<Solid, KernelError> {
    let error = |message: String| KernelError::Evaluation {
        feature_id: feature.id,
        message,
    };
    if frames.len() != profiles.len() || profiles.len() < 2 {
        return Err(error(
            "loft requires matching work planes for at least two profiles".into(),
        ));
    }
    let frames = frames
        .iter()
        .copied()
        .map(|frame| frame.with_model_offset(offset))
        .collect::<Vec<_>>();
    let centers = frames
        .iter()
        .zip(profiles)
        .map(|(frame, profile)| {
            loft_profile_centroid(*frame, profile)
                .ok_or_else(|| error("loft profile has too many segments".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let axis = centers[centers.len() - 1] - centers[0];
    let length = axis.magnitude();
    if !length.is_finite() || length <= tolerance {
        return Err(error(
            "loft first and last section centroids must be separated".into(),
        ));
    }
    let direction = axis / length;
    let mut previous = f64::NEG_INFINITY;
    let mut orientation = None;
    for (index, ((frame, profile), center)) in frames.iter().zip(profiles).zip(&centers).enumerate()
    {
        let projection = (*center - centers[0]).dot(direction);
        if index > 0 && projection <= previous + tolerance {
            return Err(error(format!(
                "loft section {index} does not advance monotonically along the section axis"
            )));
        }
        previous = projection;
        let area_direction = if profile.signed_area().is_sign_positive() {
            Vector3::new(frame.normal[0], frame.normal[1], frame.normal[2])
        } else {
            -Vector3::new(frame.normal[0], frame.normal[1], frame.normal[2])
        };
        let agreement = area_direction.dot(direction);
        if !agreement.is_finite() || agreement.abs() <= 1.0e-6 {
            return Err(error(format!(
                "loft section {index} plane is tangent to the section axis"
            )));
        }
        let section_orientation = agreement.is_sign_positive();
        if orientation.is_some_and(|expected| expected != section_orientation) {
            return Err(error(format!(
                "loft section {index} has an inconsistent model-space winding"
            )));
        }
        orientation = Some(section_orientation);
    }
    let forward = orientation.unwrap_or(true);
    let wires = frames
        .iter()
        .zip(profiles)
        .map(|(frame, profile)| sketch_wire(*frame, profile))
        .collect::<Vec<_>>();
    let first_cap = builder::try_attach_plane(&[wires[0].clone()])
        .map_err(|cause| error(format!("could not construct loft start cap: {cause}")))?;
    let mut faces = Vec::with_capacity(
        profiles[0]
            .segments
            .len()
            .saturating_mul(profiles.len() - 1)
            .saturating_add(2),
    );
    faces.push(if forward {
        first_cap.inverse()
    } else {
        first_cap
    });
    for (transition, pair) in wires.windows(2).enumerate() {
        let side_faces = builder::try_wire_homotopy(&pair[0], &pair[1]).map_err(|cause| {
            error(format!(
                "could not construct loft transition {transition}: {cause}"
            ))
        })?;
        faces.extend(
            side_faces
                .into_iter()
                .map(|face| if forward { face } else { face.inverse() }),
        );
    }
    let end_cap = builder::try_attach_plane(&[wires[wires.len() - 1].clone()])
        .map_err(|cause| error(format!("could not construct loft end cap: {cause}")))?;
    faces.push(if forward { end_cap } else { end_cap.inverse() });
    let shell: Shell = faces.into_iter().collect();
    let solid = Solid::try_new(vec![shell])
        .map_err(|cause| error(format!("loft did not form one closed shell: {cause}")))?;
    if !solid.is_geometric_consistent() {
        return Err(error(
            "loft closed shell is not geometrically consistent".into(),
        ));
    }
    Ok(solid)
}

impl Default for TruckKernel {
    fn default() -> Self {
        Self {
            tolerance: 0.05,
            boolean_tolerance_policy: BooleanTolerancePolicy::default(),
        }
    }
}

impl TruckKernel {
    #[must_use]
    pub fn new(tolerance: f64) -> Self {
        let tolerance = if tolerance.is_finite() && tolerance > 0.0 {
            tolerance.max(1.0e-6)
        } else {
            1.0e-6
        };
        let boolean_tolerance_policy = BooleanTolerancePolicy {
            absolute_mm: tolerance,
            relative: 0.0,
            maximum_mm: tolerance,
            retry_multiplier: 1.0,
            max_attempts: 1,
            healing: cadx_core::tolerance::BooleanHealingPolicy::Disabled,
        };
        Self {
            tolerance,
            boolean_tolerance_policy,
        }
    }

    /// Replaces the bounded boolean tolerance and healing policy.
    ///
    /// # Errors
    ///
    /// Returns a field-level policy error before the adapter can evaluate a
    /// document with invalid numeric configuration.
    pub fn with_boolean_tolerance_policy(
        mut self,
        policy: BooleanTolerancePolicy,
    ) -> Result<Self, BooleanTolerancePolicyError> {
        policy.validate()?;
        self.boolean_tolerance_policy = policy;
        Ok(self)
    }

    #[must_use]
    pub const fn boolean_tolerance_policy(self) -> BooleanTolerancePolicy {
        self.boolean_tolerance_policy
    }

    fn evaluate_feature(
        self,
        feature: &Feature,
        document: &CadDocument,
        upstream: &HashMap<FeatureId, NamedSolid>,
        sketch_frames: &HashMap<FeatureId, SketchFrame>,
        shared_imports: &HashMap<FeatureId, AssemblyDefinitionBody>,
        imported_step_cache: &mut ImportedStepSolidCache,
    ) -> Result<(NamedSolid, EvaluatedPart), KernelError> {
        let solid = self
            .build_solid(
                feature,
                document,
                upstream,
                sketch_frames,
                shared_imports,
                imported_step_cache,
            )
            .map_err(|error| self.enrich_feature_error(feature, upstream, error))?;
        let (mesh, faces, edges, vertices) =
            topology::evaluated_topology(feature, &solid, self.tolerance)
                .map_err(|error| self.enrich_feature_error(feature, upstream, error))?;

        Ok((
            solid,
            EvaluatedPart {
                feature_id: feature.id,
                name: feature.name.clone(),
                color: feature.color,
                material: feature.material.clone(),
                mesh,
                faces,
                edges,
                vertices,
            },
        ))
    }

    fn build_solid(
        self,
        feature: &Feature,
        document: &CadDocument,
        upstream: &HashMap<FeatureId, NamedSolid>,
        sketch_frames: &HashMap<FeatureId, SketchFrame>,
        shared_imports: &HashMap<FeatureId, AssemblyDefinitionBody>,
        imported_step_cache: &mut ImportedStepSolidCache,
    ) -> Result<NamedSolid, KernelError> {
        let origin = feature.translation;
        let build_extrusion = |frame: SketchFrame,
                               region: &SketchRegion2D,
                               height: f64|
         -> Result<Solid, KernelError> {
            let build_wire = |loop_: &SketchLoop2D| {
                let vertices = loop_
                    .segments
                    .iter()
                    .map(SketchSegment2D::start)
                    .map(|point| builder::vertex(frame.point(point)))
                    .collect::<Vec<_>>();
                let mut edges = Vec::with_capacity(vertices.len());
                for (index, segment) in loop_.segments.iter().enumerate() {
                    let next = (index + 1) % vertices.len();
                    edges.push(sketch_edge(
                        frame,
                        segment,
                        &vertices[index],
                        &vertices[next],
                    ));
                }
                Wire::from(edges)
            };
            let mut wires = Vec::with_capacity(region.holes.len() + 1);
            wires.push(build_wire(&region.profile));
            let outer_area = region.profile.signed_area();
            wires.extend(region.holes.iter().map(|hole| {
                let wire = build_wire(hole);
                if hole.signed_area().is_sign_positive() == outer_area.is_sign_positive() {
                    wire.inverse()
                } else {
                    wire
                }
            }));
            let face =
                builder::try_attach_plane(&wires).map_err(|error| KernelError::Evaluation {
                    feature_id: feature.id,
                    message: format!(
                        "could not construct extrusion profile with inner loops: {error}"
                    ),
                })?;
            Ok(builder::tsweep(
                &face,
                Vector3::new(
                    frame.normal[0] * height,
                    frame.normal[1] * height,
                    frame.normal[2] * height,
                ),
            ))
        };
        let build_revolve = |frame: SketchFrame,
                             profile: &SketchLoop2D,
                             axis_origin: [f64; 2],
                             axis_direction: [f64; 2],
                             angle: f64|
         -> Result<Solid, KernelError> {
            let vertices = profile
                .segments
                .iter()
                .map(SketchSegment2D::start)
                .map(|point| builder::vertex(frame.point(point)))
                .collect::<Vec<_>>();
            let mut edges = Vec::with_capacity(vertices.len());
            for (index, segment) in profile.segments.iter().enumerate() {
                let next = (index + 1) % vertices.len();
                edges.push(sketch_edge(
                    frame,
                    segment,
                    &vertices[index],
                    &vertices[next],
                ));
            }
            let face = builder::try_attach_plane(&[Wire::from(edges)]).map_err(|error| {
                KernelError::Evaluation {
                    feature_id: feature.id,
                    message: format!("could not construct revolve profile: {error}"),
                }
            })?;
            let length = axis_direction[0].hypot(axis_direction[1]);
            let axis = frame.direction([axis_direction[0] / length, axis_direction[1] / length]);
            Ok(builder::rsweep(
                &face,
                frame.point(axis_origin),
                axis,
                Rad(angle.to_radians()),
            ))
        };
        let (mut solid, faces) = if let Primitive::ImportedStep {
            source,
            data_section,
            shell_id,
            void_shells,
            length_unit,
        } = &feature.primitive
        {
            let key = shared_imports.get(&feature.id).copied();
            let solid = imported_step_cache.instantiate(key, || {
                let mut solid =
                    import_step_solid(feature, source, *data_section, *shell_id, void_shells)?;
                let scale = length_unit.millimeters_per_unit;
                if (scale - 1.0).abs() > f64::EPSILON {
                    solid = builder::scaled(
                        &solid,
                        Point3::origin(),
                        Vector3::new(scale, scale, scale),
                    );
                }
                Ok(solid)
            })?;
            let faces = topology::name_primitive_faces(
                feature,
                &feature.primitive,
                &solid,
                self.tolerance,
            )?;
            (solid, faces)
        } else {
            match feature.primitive.clone() {
                Primitive::Box { size } => {
                    let vertex = builder::vertex(Point3::new(origin.x, origin.y, origin.z));
                    let edge = builder::tsweep(&vertex, Vector3::new(size.x, 0.0, 0.0));
                    let face = builder::tsweep(&edge, Vector3::new(0.0, size.y, 0.0));
                    let solid = builder::tsweep(&face, Vector3::new(0.0, 0.0, size.z));
                    let faces = topology::name_primitive_faces(
                        feature,
                        &feature.primitive,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::Cylinder { radius, height } => {
                    let center = Point3::new(origin.x, origin.y, origin.z);
                    let start = Point3::new(origin.x + radius, origin.y, origin.z);
                    let vertex = builder::vertex(start);
                    let circle: Wire =
                        builder::rsweep(&vertex, center, Vector3::unit_z(), Rad(TAU));
                    let disk: Face = builder::try_attach_plane(&[circle]).map_err(|error| {
                        KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!("could not construct cylinder profile: {error}"),
                        }
                    })?;
                    let solid = builder::tsweep(&disk, Vector3::new(0.0, 0.0, height));
                    let faces = topology::name_primitive_faces(
                        feature,
                        &feature.primitive,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::Sphere { radius } => {
                    let center = Point3::new(origin.x, origin.y, origin.z);
                    let vertex =
                        builder::vertex(Point3::new(origin.x, origin.y + radius, origin.z));
                    let meridian: Wire =
                        builder::rsweep(&vertex, center, Vector3::unit_x(), Rad(TAU / 2.0));
                    let shell = builder::cone(&meridian, Vector3::unit_y(), Rad(TAU));
                    let solid = Solid::new(vec![shell]);
                    let faces = topology::name_primitive_faces(
                        feature,
                        &feature.primitive,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::Cone {
                    bottom_radius,
                    top_radius,
                    height,
                } => {
                    let bottom_center = builder::vertex(Point3::new(origin.x, origin.y, origin.z));
                    let bottom_outer =
                        builder::vertex(Point3::new(origin.x + bottom_radius, origin.y, origin.z));
                    let top_center =
                        builder::vertex(Point3::new(origin.x, origin.y, origin.z + height));
                    let mut profile = vec![builder::line(&bottom_center, &bottom_outer)];
                    if top_radius > 0.0 {
                        let top_outer = builder::vertex(Point3::new(
                            origin.x + top_radius,
                            origin.y,
                            origin.z + height,
                        ));
                        profile.push(builder::line(&bottom_outer, &top_outer));
                        profile.push(builder::line(&top_outer, &top_center));
                    } else {
                        profile.push(builder::line(&bottom_outer, &top_center));
                    }
                    let profile: Wire = profile.into();
                    let shell = builder::cone(&profile, Vector3::unit_z(), Rad(TAU));
                    let solid = Solid::new(vec![shell]);
                    let faces = topology::name_primitive_faces(
                        feature,
                        &feature.primitive,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::Torus {
                    major_radius,
                    minor_radius,
                } => {
                    let section_start = builder::vertex(Point3::new(
                        origin.x + major_radius,
                        origin.y,
                        origin.z + minor_radius,
                    ));
                    let section = builder::rsweep(
                        &section_start,
                        Point3::new(origin.x + major_radius, origin.y, origin.z),
                        Vector3::unit_y(),
                        Rad(TAU),
                    );
                    let shell = builder::rsweep(
                        &section,
                        Point3::new(origin.x, origin.y, origin.z),
                        Vector3::unit_z(),
                        Rad(TAU),
                    );
                    let solid = Solid::new(vec![shell]);
                    let faces = topology::name_primitive_faces(
                        feature,
                        &feature.primitive,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::Extrusion { profile, height } => {
                    let region = SketchRegion2D::from_polygons(profile.clone(), Vec::new());
                    let solid = build_extrusion(
                        SketchFrame::WORLD_XY.with_model_offset(origin),
                        &region,
                        height,
                    )?;
                    let faces = topology::name_primitive_faces(
                        feature,
                        &feature.primitive,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::ExtrusionFromSketch {
                    sketch_id, height, ..
                } => {
                    let sketch =
                        document
                            .feature(sketch_id)
                            .ok_or_else(|| KernelError::Evaluation {
                                feature_id: feature.id,
                                message: format!("source sketch {sketch_id} does not exist"),
                            })?;
                    let Primitive::Sketch {
                        region,
                        construction,
                        constraints,
                        ..
                    } = &sketch.primitive
                    else {
                        return Err(KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!("source feature {sketch_id} is not a sketch"),
                        });
                    };
                    let region = cadx_sketch::solve_sketch(
                        region,
                        construction,
                        constraints,
                        cadx_sketch::SolverConfig::default(),
                    )
                    .map_err(|error| KernelError::Evaluation {
                        feature_id: feature.id,
                        message: format!("source sketch {sketch_id} constraints failed: {error}"),
                    })?
                    .region;
                    let frame = sketch_frames.get(&sketch_id).copied().ok_or_else(|| {
                        KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!(
                                "source sketch {sketch_id} has no resolved work plane"
                            ),
                        }
                    })?;
                    let frame = frame.with_model_offset(origin);
                    let solid = build_extrusion(frame, &region, height)?;
                    let faces = topology::name_extrusion_faces(
                        feature,
                        &region,
                        height,
                        &solid,
                        frame.origin,
                        frame.x_dir,
                        frame.y_dir,
                        frame.normal,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::RevolveFromSketch {
                    sketch_id,
                    profile: _,
                    axis_origin,
                    axis_direction,
                    angle,
                } => {
                    let sketch =
                        document
                            .feature(sketch_id)
                            .ok_or_else(|| KernelError::Evaluation {
                                feature_id: feature.id,
                                message: format!("source sketch {sketch_id} does not exist"),
                            })?;
                    let Primitive::Sketch {
                        region,
                        construction,
                        constraints,
                        ..
                    } = &sketch.primitive
                    else {
                        return Err(KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!("source feature {sketch_id} is not a sketch"),
                        });
                    };
                    if !region.holes.is_empty() {
                        return Err(KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!(
                                "source sketch {sketch_id} has hole loops, which revolve does not support"
                            ),
                        });
                    }
                    let region = cadx_sketch::solve_sketch(
                        region,
                        construction,
                        constraints,
                        cadx_sketch::SolverConfig::default(),
                    )
                    .map_err(|error| KernelError::Evaluation {
                        feature_id: feature.id,
                        message: format!("source sketch {sketch_id} constraints failed: {error}"),
                    })?
                    .region;
                    let profile = region.profile;
                    let frame = sketch_frames.get(&sketch_id).copied().ok_or_else(|| {
                        KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!(
                                "source sketch {sketch_id} has no resolved work plane"
                            ),
                        }
                    })?;
                    let solid = build_revolve(
                        frame.with_model_offset(origin),
                        &profile,
                        axis_origin,
                        axis_direction,
                        angle,
                    )?;
                    let effective = Primitive::RevolveFromSketch {
                        sketch_id,
                        profile,
                        axis_origin,
                        axis_direction,
                        angle,
                    };
                    let faces = topology::name_primitive_faces(
                        feature,
                        &effective,
                        &solid,
                        self.tolerance,
                    )?;
                    (solid, faces)
                }
                Primitive::LoftFromSketches { sketch_ids, .. } => {
                    let mut profiles = Vec::with_capacity(sketch_ids.len());
                    let mut frames = Vec::with_capacity(sketch_ids.len());
                    for sketch_id in &sketch_ids {
                        let sketch = document.feature(*sketch_id).ok_or_else(|| {
                            KernelError::Evaluation {
                                feature_id: feature.id,
                                message: format!("source sketch {sketch_id} does not exist"),
                            }
                        })?;
                        let Primitive::Sketch {
                            region,
                            construction,
                            constraints,
                            ..
                        } = &sketch.primitive
                        else {
                            return Err(KernelError::Evaluation {
                                feature_id: feature.id,
                                message: format!("source feature {sketch_id} is not a sketch"),
                            });
                        };
                        if !region.holes.is_empty() {
                            return Err(KernelError::Evaluation {
                                feature_id: feature.id,
                                message: format!(
                                    "source sketch {sketch_id} has hole loops, which ruled loft does not support"
                                ),
                            });
                        }
                        let solved = cadx_sketch::solve_sketch(
                            region,
                            construction,
                            constraints,
                            cadx_sketch::SolverConfig::default(),
                        )
                        .map_err(|error| KernelError::Evaluation {
                            feature_id: feature.id,
                            message: format!(
                                "source sketch {sketch_id} constraints failed: {error}"
                            ),
                        })?;
                        profiles.push(solved.region.profile);
                        frames.push(*sketch_frames.get(sketch_id).ok_or_else(|| {
                            KernelError::Evaluation {
                                feature_id: feature.id,
                                message: format!(
                                    "source sketch {sketch_id} has no resolved work plane"
                                ),
                            }
                        })?);
                    }
                    let solid =
                        build_loft_solid(feature, &frames, &profiles, origin, self.tolerance)?;
                    let effective = Primitive::LoftFromSketches {
                        sketch_ids,
                        profiles,
                    };
                    let faces = topology::name_loft_faces(feature, &effective, &solid)?;
                    (solid, faces)
                }
                Primitive::ImportedStep { .. } => unreachable!("handled before primitive cloning"),
                Primitive::Chamfer { edges, distance } => {
                    let Some(source_id) = edges.first().map(|edge| edge.feature_id) else {
                        return Err(edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::EmptyEdgeSet,
                            None,
                            "chamfer requires at least one edge",
                        ));
                    };
                    if edges.iter().any(|edge| edge.feature_id != source_id) {
                        return Err(edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::MixedSourceFeatures,
                            None,
                            "chamfer edges must belong to one source feature",
                        ));
                    }
                    let source = upstream.get(&source_id).ok_or_else(|| {
                        edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::LostReference,
                            None,
                            format!("chamfer source feature {source_id} was not evaluated"),
                        )
                    })?;
                    let source_feature = document.feature(source_id).ok_or_else(|| {
                        edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::LostReference,
                            None,
                            format!(
                                "chamfer source feature {source_id} is missing from the document"
                            ),
                        )
                    })?;
                    let (solid, generated) = build_chamfer(
                        feature,
                        source_feature,
                        source,
                        &edges,
                        distance,
                        self.tolerance,
                    )?;
                    let faces = topology::name_edge_modifier_faces(
                        feature,
                        &solid,
                        source,
                        &generated,
                        "chamfer",
                        self.tolerance,
                    )
                    .map_err(|error| {
                        edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::TopologyNaming,
                            EdgeModifierFailureReason::TopologyNamingFailed,
                            None,
                            error.to_string(),
                        )
                    })?;
                    (solid, faces)
                }
                Primitive::Fillet { edges, radius } => {
                    let Some(source_id) = edges.first().map(|edge| edge.feature_id) else {
                        return Err(edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::EmptyEdgeSet,
                            None,
                            "fillet requires at least one edge",
                        ));
                    };
                    if edges.iter().any(|edge| edge.feature_id != source_id) {
                        return Err(edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::MixedSourceFeatures,
                            None,
                            "fillet edges must belong to one source feature",
                        ));
                    }
                    let source = upstream.get(&source_id).ok_or_else(|| {
                        edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::LostReference,
                            None,
                            format!("fillet source feature {source_id} was not evaluated"),
                        )
                    })?;
                    let source_feature = document.feature(source_id).ok_or_else(|| {
                        edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ReferenceResolution,
                            EdgeModifierFailureReason::LostReference,
                            None,
                            format!(
                                "fillet source feature {source_id} is missing from the document"
                            ),
                        )
                    })?;
                    let (solid, generated) = build_fillet(
                        feature,
                        source_feature,
                        source,
                        &edges,
                        radius,
                        self.tolerance,
                    )?;
                    let faces = topology::name_edge_modifier_faces(
                        feature,
                        &solid,
                        source,
                        &generated,
                        "fillet",
                        self.tolerance,
                    )
                    .map_err(|error| {
                        edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::TopologyNaming,
                            EdgeModifierFailureReason::TopologyNamingFailed,
                            None,
                            error.to_string(),
                        )
                    })?;
                    (solid, faces)
                }
                Primitive::Boolean {
                    operation,
                    left,
                    right,
                } => {
                    let left_solid = upstream.get(&left).ok_or_else(|| {
                        KernelError::from(boolean_diagnostic(
                            feature,
                            operation,
                            [left, right],
                            self.boolean_tolerance_policy.absolute_mm,
                            None,
                            None,
                            BooleanFailureStage::OperandResolution,
                            BooleanFailureReason::MissingOperand,
                            format!("upstream solid {left} was not evaluated"),
                        ))
                    })?;
                    let right_solid = upstream.get(&right).ok_or_else(|| {
                        KernelError::from(boolean_diagnostic(
                            feature,
                            operation,
                            [left, right],
                            self.boolean_tolerance_policy.absolute_mm,
                            solid_bounds(&left_solid.solid),
                            None,
                            BooleanFailureStage::OperandResolution,
                            BooleanFailureReason::MissingOperand,
                            format!("upstream solid {right} was not evaluated"),
                        ))
                    })?;
                    boolean::evaluate(
                        feature,
                        operation,
                        [left, right],
                        left_solid,
                        right_solid,
                        self.boolean_tolerance_policy,
                    )?
                }
                Primitive::Sketch { .. } => {
                    return Err(KernelError::Evaluation {
                        feature_id: feature.id,
                        message: "sketches are reference geometry and do not produce a solid mesh"
                            .into(),
                    });
                }
                Primitive::DatumPlane { .. } => {
                    return Err(KernelError::Evaluation {
                        feature_id: feature.id,
                        message:
                            "datum planes are reference geometry and do not produce a solid mesh"
                                .into(),
                    });
                }
                Primitive::DatumPoint { .. } => {
                    return Err(KernelError::Evaluation {
                        feature_id: feature.id,
                        message:
                            "datum points are reference geometry and do not produce a solid mesh"
                                .into(),
                    });
                }
            }
        };
        let is_derived = matches!(
            feature.primitive,
            Primitive::Boolean { .. } | Primitive::Chamfer { .. } | Primitive::Fillet { .. }
        );
        let is_imported = matches!(feature.primitive, Primitive::ImportedStep { .. });
        let center =
            if is_derived || is_imported {
                Point3::origin()
            } else if let Primitive::LoftFromSketches { sketch_ids, .. } = &feature.primitive {
                let sketch_id = *sketch_ids.first().ok_or_else(|| KernelError::Evaluation {
                    feature_id: feature.id,
                    message: "loft has no source sketch".into(),
                })?;
                let frame = sketch_frames.get(&sketch_id).copied().ok_or_else(|| {
                    KernelError::Evaluation {
                        feature_id: feature.id,
                        message: format!("source sketch {sketch_id} has no resolved work plane"),
                    }
                })?;
                let frame = frame.with_model_offset(origin);
                Point3::new(frame.origin[0], frame.origin[1], frame.origin[2])
            } else if let Some(sketch_id) = feature.primitive.source_sketch() {
                let frame = sketch_frames.get(&sketch_id).copied().ok_or_else(|| {
                    KernelError::Evaluation {
                        feature_id: feature.id,
                        message: format!("source sketch {sketch_id} has no resolved work plane"),
                    }
                })?;
                let frame = frame.with_model_offset(origin);
                Point3::new(frame.origin[0], frame.origin[1], frame.origin[2])
            } else {
                Point3::new(origin.x, origin.y, origin.z)
            };
        for (axis, angle) in [
            (Vector3::unit_x(), feature.rotation.x),
            (Vector3::unit_y(), feature.rotation.y),
            (Vector3::unit_z(), feature.rotation.z),
        ] {
            if angle.abs() > f64::EPSILON {
                solid = builder::rotated(&solid, center, axis, Rad(angle.to_radians()));
            }
        }
        if (is_derived || is_imported) && origin != cadx_core::domain::Vec3::ZERO {
            solid = builder::translated(&solid, Vector3::new(origin.x, origin.y, origin.z));
        }
        NamedSolid::new(feature, solid, faces)
    }

    fn enrich_boolean_result_error(
        self,
        feature: &Feature,
        upstream: &HashMap<FeatureId, NamedSolid>,
        error: KernelError,
    ) -> KernelError {
        let Primitive::Boolean {
            operation,
            left,
            right,
        } = &feature.primitive
        else {
            return error;
        };
        if matches!(error, KernelError::Boolean(_)) {
            return error;
        }
        let (stage, reason) = if matches!(error, KernelError::TopologyNaming { .. }) {
            (
                BooleanFailureStage::TopologyNaming,
                BooleanFailureReason::TopologyNamingFailed,
            )
        } else {
            (
                BooleanFailureStage::ResultValidation,
                BooleanFailureReason::ResultEvaluationFailed,
            )
        };
        boolean_diagnostic(
            feature,
            *operation,
            [*left, *right],
            self.boolean_tolerance_policy.absolute_mm,
            upstream
                .get(left)
                .and_then(|named| solid_bounds(&named.solid)),
            upstream
                .get(right)
                .and_then(|named| solid_bounds(&named.solid)),
            stage,
            reason,
            error.to_string(),
        )
        .into()
    }

    fn enrich_edge_modifier_error(self, feature: &Feature, error: KernelError) -> KernelError {
        if matches!(error, KernelError::EdgeModifier(_)) {
            return error;
        }
        let (stage, reason) = if matches!(error, KernelError::TopologyNaming { .. }) {
            (
                EdgeModifierFailureStage::TopologyNaming,
                EdgeModifierFailureReason::TopologyNamingFailed,
            )
        } else {
            (
                EdgeModifierFailureStage::ResultValidation,
                EdgeModifierFailureReason::InvalidResultTopology,
            )
        };
        edge_modifier_error(
            feature,
            self.tolerance,
            stage,
            reason,
            None,
            error.to_string(),
        )
    }

    fn enrich_feature_error(
        self,
        feature: &Feature,
        upstream: &HashMap<FeatureId, NamedSolid>,
        error: KernelError,
    ) -> KernelError {
        match feature.primitive {
            Primitive::Boolean { .. } => self.enrich_boolean_result_error(feature, upstream, error),
            Primitive::Chamfer { .. } | Primitive::Fillet { .. } => {
                self.enrich_edge_modifier_error(feature, error)
            }
            _ => error,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PlanarEdgeFrame {
    start: Point3,
    axis: Vector3,
    length: f64,
    normals: [Vector3; 2],
    inward: [Vector3; 2],
    exterior: Vector3,
}

#[derive(Debug, Clone, Copy)]
struct EdgeModifierSpec {
    operation: &'static str,
    parameter: &'static str,
    size: f64,
}

fn edge_modifier_error(
    feature: &Feature,
    tolerance: f64,
    stage: EdgeModifierFailureStage,
    reason: EdgeModifierFailureReason,
    offending_edge_indices: Option<Vec<usize>>,
    detail: impl Into<String>,
) -> KernelError {
    let (operation, edges, parameter, parameter_value_mm) = match &feature.primitive {
        Primitive::Chamfer { edges, distance } => (
            EdgeModifierOperation::Chamfer,
            edges.clone(),
            EdgeModifierParameter::Distance,
            *distance,
        ),
        Primitive::Fillet { edges, radius } => (
            EdgeModifierOperation::Fillet,
            edges.clone(),
            EdgeModifierParameter::Radius,
            *radius,
        ),
        _ => {
            return KernelError::Evaluation {
                feature_id: feature.id,
                message: detail.into(),
            };
        }
    };
    EdgeModifierDiagnostic {
        feature_id: feature.id,
        operation,
        source_feature_id: edges.first().map(|edge| edge.feature_id),
        edges,
        stage,
        reason,
        parameter,
        parameter_value_mm,
        tolerance_mm: tolerance,
        offending_edge_indices,
        detail: detail.into(),
    }
    .into()
}

fn resolve_planar_convex_edge(
    feature: &Feature,
    source_feature: &Feature,
    source: &NamedSolid,
    reference: &cadx_core::topology::EdgeRef,
    edge_index: usize,
    spec: EdgeModifierSpec,
    tolerance: f64,
) -> Result<PlanarEdgeFrame, KernelError> {
    if spec.size <= tolerance {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::ParameterBelowTolerance,
            None,
            format!(
                "{} {} {} mm must exceed the modeling tolerance {tolerance} mm",
                spec.operation, spec.parameter, spec.size
            ),
        ));
    }
    let resolved =
        topology::resolve_edge(source_feature, source, reference, tolerance).map_err(|error| {
            let reason = match error.failure {
                topology::EdgeResolutionFailure::Lost => EdgeModifierFailureReason::LostReference,
                topology::EdgeResolutionFailure::Ambiguous => {
                    EdgeModifierFailureReason::AmbiguousReference
                }
                topology::EdgeResolutionFailure::InvalidTopology => {
                    EdgeModifierFailureReason::TopologyNamingFailed
                }
            };
            edge_modifier_error(
                feature,
                tolerance,
                EdgeModifierFailureStage::ReferenceResolution,
                reason,
                Some(vec![edge_index]),
                error.detail,
            )
        })?;
    if resolved.geometry.curve != cadx_core::topology::CurveKind::Line {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::NonLinearEdge,
            Some(vec![edge_index]),
            format!("{} currently requires a linear edge", spec.operation),
        ));
    }
    if resolved
        .adjacent_faces
        .iter()
        .any(|face| face.surface != cadx_core::topology::SurfaceKind::Plane)
    {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::NonPlanarSupport,
            Some(vec![edge_index]),
            format!(
                "{} currently requires two planar adjacent faces",
                spec.operation
            ),
        ));
    }

    let normals = resolved.adjacent_faces.each_ref().map(|face| {
        Vector3::new(
            face.mean_normal[0],
            face.mean_normal[1],
            face.mean_normal[2],
        )
    });
    let edge_directions = resolved
        .adjacent_edge_directions
        .map(|direction| Vector3::new(direction[0], direction[1], direction[2]));
    let inward = [
        normals[0].cross(edge_directions[0]),
        normals[1].cross(edge_directions[1]),
    ]
    .map(|direction| {
        let length = direction.magnitude();
        (length.is_finite() && length > f64::EPSILON).then(|| direction / length)
    });
    let [Some(inward_first), Some(inward_second)] = inward else {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::KernelRejected,
            Some(vec![edge_index]),
            format!(
                "{} edge has degenerate adjacent face orientation",
                spec.operation
            ),
        ));
    };
    let projected = [
        -normals[1] + normals[0] * normals[0].dot(normals[1]),
        -normals[0] + normals[1] * normals[1].dot(normals[0]),
    ]
    .map(|direction| {
        let length = direction.magnitude();
        (length.is_finite() && length > tolerance).then(|| direction / length)
    });
    let [Some(projected_first), Some(projected_second)] = projected else {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::NonPlanarSupport,
            Some(vec![edge_index]),
            format!(
                "{} adjacent planes are parallel or degenerate",
                spec.operation
            ),
        ));
    };
    let convexity = [
        inward_first.dot(projected_first),
        inward_second.dot(projected_second),
    ];
    if convexity[0] < 0.9 || convexity[1] < 0.9 {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::NonConvexEdge,
            Some(vec![edge_index]),
            format!(
                "{} currently supports convex manifold edges only (orientation agreement {:.3}, {:.3})",
                spec.operation, convexity[0], convexity[1]
            ),
        ));
    }

    let start = resolved.edge.front().point();
    let edge_vector = resolved.edge.back().point() - start;
    let length = edge_vector.magnitude();
    if !length.is_finite() || length <= tolerance {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::InvalidResultTopology,
            Some(vec![edge_index]),
            format!(
                "{} edge is shorter than the modeling tolerance",
                spec.operation
            ),
        ));
    }
    let exterior = normals[0] + normals[1];
    let exterior_length = exterior.magnitude();
    if !exterior_length.is_finite() || exterior_length <= tolerance {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::KernelRejected,
            Some(vec![edge_index]),
            format!(
                "{} adjacent face normals do not define an exterior wedge",
                spec.operation
            ),
        ));
    }
    Ok(PlanarEdgeFrame {
        start,
        axis: edge_vector / length,
        length,
        normals,
        inward: [inward_first, inward_second],
        exterior: exterior / exterior_length,
    })
}

fn extended_wedge_prism(
    feature: &Feature,
    frame: PlanarEdgeFrame,
    offsets: [Vector3; 2],
    size: f64,
    tolerance: f64,
    operation: &str,
) -> Result<(Solid, f64), KernelError> {
    let extension = size.max(tolerance * 4.0);
    let edge_point = frame.start - frame.axis * extension;
    let first = edge_point + offsets[0];
    let second = edge_point + offsets[1];
    let cut_span = second - first;
    let first_extended = first - cut_span * 0.5;
    let second_extended = second + cut_span * 0.5;
    let outside_offset = frame.exterior * size.mul_add(8.0, tolerance * 4.0);
    let mut profile = vec![
        first_extended,
        second_extended,
        second_extended + outside_offset,
        first_extended + outside_offset,
    ];
    if (second_extended - first_extended)
        .cross(outside_offset)
        .dot(frame.axis)
        < 0.0
    {
        profile.reverse();
    }
    let vertices = profile.into_iter().map(builder::vertex).collect::<Vec<_>>();
    let edges = (0..vertices.len())
        .map(|index| builder::line(&vertices[index], &vertices[(index + 1) % vertices.len()]))
        .collect::<Vec<_>>();
    let face = builder::try_attach_plane(&[Wire::from(edges)]).map_err(|error| {
        edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::Construction,
            EdgeModifierFailureReason::KernelRejected,
            None,
            format!("could not construct {operation} cutter profile: {error}"),
        )
    })?;
    Ok((
        builder::tsweep(&face, frame.axis * (frame.length + extension * 2.0)),
        extension,
    ))
}

fn validate_edge_modifier_result(
    feature: &Feature,
    result: Solid,
    operation: &str,
    tolerance: f64,
) -> Result<Solid, KernelError> {
    if result.boundaries().is_empty() {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::ResultValidation,
            EdgeModifierFailureReason::InvalidResultTopology,
            None,
            format!("{operation} produced an empty solid"),
        ));
    }
    Solid::try_new(result.boundaries().clone()).map_err(|error| {
        edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::ResultValidation,
            EdgeModifierFailureReason::InvalidResultTopology,
            None,
            format!("{operation} produced invalid solid topology: {error}"),
        )
    })?;
    Ok(result)
}

fn build_chamfer(
    feature: &Feature,
    source_feature: &Feature,
    source: &NamedSolid,
    references: &[cadx_core::topology::EdgeRef],
    distance: f64,
    tolerance: f64,
) -> Result<(Solid, Vec<topology::GeneratedFace>), KernelError> {
    let frames = references
        .iter()
        .enumerate()
        .map(|(edge_index, reference)| {
            resolve_planar_convex_edge(
                feature,
                source_feature,
                source,
                reference,
                edge_index,
                EdgeModifierSpec {
                    operation: "chamfer",
                    parameter: "distance",
                    size: distance,
                },
                tolerance,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if modifier_frames_share_vertex(&frames, tolerance) {
        return build_convex_miter_chamfer(
            feature, source, references, &frames, distance, tolerance,
        );
    }

    let mut result = source.solid.clone();
    let mut generated = Vec::with_capacity(references.len());
    for (edge_index, (reference, frame)) in references.iter().zip(frames).enumerate() {
        let offsets = frame.inward.map(|direction| direction * distance);
        let (cutter, _) =
            extended_wedge_prism(feature, frame, offsets, distance, tolerance, "chamfer")?;
        let next =
            subtract_edge_cutter(feature, &result, cutter, tolerance, "chamfer", edge_index)?;
        let bevel = topology::unique_generated_face(feature, &next, &result, "chamfer").map_err(
            |error| {
                edge_modifier_error(
                    feature,
                    tolerance,
                    EdgeModifierFailureStage::TopologyNaming,
                    EdgeModifierFailureReason::TopologyNamingFailed,
                    None,
                    error.to_string(),
                )
            },
        )?;
        generated.push(topology::generated_face(
            &bevel,
            reference,
            cadx_core::topology::SurfaceKind::Plane,
        ));
        result = validate_edge_modifier_result(feature, next, "chamfer", tolerance)?;
    }
    Ok((result, generated))
}

fn build_fillet(
    feature: &Feature,
    source_feature: &Feature,
    source: &NamedSolid,
    references: &[cadx_core::topology::EdgeRef],
    radius: f64,
    tolerance: f64,
) -> Result<(Solid, Vec<topology::GeneratedFace>), KernelError> {
    let frames = references
        .iter()
        .enumerate()
        .map(|(edge_index, reference)| {
            resolve_planar_convex_edge(
                feature,
                source_feature,
                source,
                reference,
                edge_index,
                EdgeModifierSpec {
                    operation: "fillet",
                    parameter: "radius",
                    size: radius,
                },
                tolerance,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    reject_shared_modifier_vertices(feature, &frames, tolerance, "fillet")?;

    let mut result = source.solid.clone();
    let mut generated = Vec::with_capacity(references.len());
    for (edge_index, (reference, frame)) in references.iter().zip(frames).enumerate() {
        let (next, face) = apply_fillet(
            feature, &result, reference, edge_index, frame, radius, tolerance,
        )?;
        result = next;
        generated.push(face);
    }
    Ok((result, generated))
}

fn apply_fillet(
    feature: &Feature,
    source: &Solid,
    reference: &cadx_core::topology::EdgeRef,
    edge_index: usize,
    frame: PlanarEdgeFrame,
    radius: f64,
    tolerance: f64,
) -> Result<(Solid, topology::GeneratedFace), KernelError> {
    let denominator = 1.0 + frame.normals[0].dot(frame.normals[1]);
    if !denominator.is_finite() || denominator <= tolerance {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::NonConvexEdge,
            Some(vec![edge_index]),
            "fillet adjacent planes do not define a finite radius center",
        ));
    }
    let center_offset = -(frame.normals[0] + frame.normals[1]) * (radius / denominator);
    let tangent_offsets = [
        center_offset + frame.normals[0] * radius,
        center_offset + frame.normals[1] * radius,
    ];
    let (wedge, _) =
        extended_wedge_prism(feature, frame, tangent_offsets, radius, tolerance, "fillet")?;

    let center = frame.start + center_offset;
    let circle_vertex = builder::vertex(center + frame.normals[0] * radius);
    let circle: Wire = builder::rsweep(&circle_vertex, center, frame.axis, Rad(TAU));
    let disk: Face = builder::try_attach_plane(&[circle]).map_err(|error| {
        edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::Construction,
            EdgeModifierFailureReason::KernelRejected,
            Some(vec![edge_index]),
            format!("could not construct fillet cylinder profile: {error}"),
        )
    })?;
    let cylinder = builder::tsweep(&disk, frame.axis * frame.length);
    let cylinder_face = cylinder
        .face_iter()
        .find(|face| !matches!(face.surface(), Surface::Plane(_)))
        .ok_or_else(|| {
            edge_modifier_error(
                feature,
                tolerance,
                EdgeModifierFailureStage::Construction,
                EdgeModifierFailureReason::KernelRejected,
                Some(vec![edge_index]),
                "fillet cylinder did not contain an exact curved lateral surface",
            )
        })?;
    let chamfered = subtract_edge_cutter(feature, source, wedge, tolerance, "fillet", edge_index)?;
    let blend_face = topology::unique_generated_face(feature, &chamfered, source, "fillet")
        .map_err(|error| {
            edge_modifier_error(
                feature,
                tolerance,
                EdgeModifierFailureStage::TopologyNaming,
                EdgeModifierFailureReason::TopologyNamingFailed,
                Some(vec![edge_index]),
                error.to_string(),
            )
        })?;
    let mut arc_count = 0;
    for edge in blend_face.edge_iter() {
        let absolute = edge.absolute_clone();
        let start = absolute.front().point();
        let end = absolute.back().point();
        let span = end - start;
        let span_length = span.magnitude();
        if !span_length.is_finite() || span_length <= tolerance {
            return Err(edge_modifier_error(
                feature,
                tolerance,
                EdgeModifierFailureStage::ResultValidation,
                EdgeModifierFailureReason::InvalidResultTopology,
                Some(vec![edge_index]),
                "fillet scaffold contains a degenerate edge",
            ));
        }
        if (span / span_length).dot(frame.axis).abs() > 0.9 {
            continue;
        }
        let midpoint = start + span * 0.5;
        let axial = (midpoint - frame.start).dot(frame.axis);
        let arc_center = frame.start + frame.axis * axial + center_offset;
        let transit = arc_center + frame.exterior * radius;
        let arc = builder::circle_arc(absolute.front(), absolute.back(), transit);
        edge.set_curve(arc.curve());
        arc_count += 1;
    }
    if arc_count != 2 {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::ResultValidation,
            EdgeModifierFailureReason::InvalidResultTopology,
            Some(vec![edge_index]),
            format!("fillet scaffold exposed {arc_count} end chords instead of two"),
        ));
    }
    let mut cylindrical_surface = cylinder_face.oriented_surface();
    if !blend_face.orientation() {
        cylindrical_surface.invert();
    }
    blend_face.set_surface(cylindrical_surface);
    if supports_geometric_consistency_check(&chamfered) && !chamfered.is_geometric_consistent() {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::ResultValidation,
            EdgeModifierFailureReason::InvalidResultTopology,
            Some(vec![edge_index]),
            "fillet edges are not geometrically consistent with the cylindrical surface",
        ));
    }
    let generated = topology::generated_face(
        &blend_face,
        reference,
        cadx_core::topology::SurfaceKind::Cylinder,
    );
    Ok((
        validate_edge_modifier_result(feature, chamfered, "fillet", tolerance)?,
        generated,
    ))
}

fn subtract_edge_cutter(
    feature: &Feature,
    source: &Solid,
    cutter: Solid,
    tolerance: f64,
    operation: &str,
    edge_index: usize,
) -> Result<Solid, KernelError> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut complement = cutter;
        complement.not();
        truck_shapeops::and(source, &complement, tolerance)
    }));
    match result {
        Ok(Some(result)) => Ok(result),
        Ok(None) => Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::Construction,
            EdgeModifierFailureReason::KernelRejected,
            Some(vec![edge_index]),
            format!("Truck rejected the {operation} subtraction"),
        )),
        Err(payload) => Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::Construction,
            EdgeModifierFailureReason::KernelPanic,
            Some(vec![edge_index]),
            format!(
                "Truck panicked while constructing the {operation}: {}",
                panic_message(payload.as_ref())
            ),
        )),
    }
}

#[derive(Clone)]
struct ConvexMiterPlane {
    origin: Point3,
    normal: Vector3,
    source_surface: Option<(Surface, bool)>,
    edge_index: Option<usize>,
}

impl ConvexMiterPlane {
    fn signed_distance(&self, point: Point3) -> f64 {
        self.normal.dot(point - self.origin)
    }
}

fn build_convex_miter_chamfer(
    feature: &Feature,
    source: &NamedSolid,
    references: &[cadx_core::topology::EdgeRef],
    frames: &[PlanarEdgeFrame],
    distance: f64,
    tolerance: f64,
) -> Result<(Solid, Vec<topology::GeneratedFace>), KernelError> {
    let [shell] = source.solid.boundaries().as_slice() else {
        return Err(miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::NonConvexSource,
            "shared-vertex chamfer requires a source solid with exactly one closed shell",
        ));
    };
    if source
        .solid
        .edge_iter()
        .any(|edge| !matches!(edge.curve(), Curve::Line(_)))
    {
        return Err(miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::NonLinearEdge,
            "shared-vertex chamfer requires a polyhedral source with linear edges",
        ));
    }

    let mut planes = Vec::new();
    for face in shell.face_iter() {
        if face.boundaries().len() != 1 {
            return Err(miter_error(
                feature,
                tolerance,
                EdgeModifierFailureReason::NonConvexSource,
                "shared-vertex chamfer requires source faces with one simple boundary",
            ));
        }
        let Surface::Plane(oriented) = face.oriented_surface() else {
            return Err(miter_error(
                feature,
                tolerance,
                EdgeModifierFailureReason::NonPlanarSupport,
                "shared-vertex chamfer requires an all-planar convex source solid",
            ));
        };
        let normal = unit_vector(oriented.normal()).ok_or_else(|| {
            miter_error(
                feature,
                tolerance,
                EdgeModifierFailureReason::NonPlanarSupport,
                "shared-vertex chamfer found a degenerate source plane",
            )
        })?;
        planes.push(ConvexMiterPlane {
            origin: oriented.origin(),
            normal,
            source_surface: Some((face.surface(), face.orientation())),
            edge_index: None,
        });
    }

    let source_vertices = source
        .solid
        .vertex_iter()
        .map(|vertex| vertex.point())
        .collect::<Vec<_>>();
    if source_vertices.is_empty()
        || planes.iter().any(|plane| {
            source_vertices
                .iter()
                .any(|point| plane.signed_distance(*point) > tolerance)
        })
    {
        return Err(miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::NonConvexSource,
            "shared-vertex chamfer requires a convex source solid with outward face orientation",
        ));
    }

    for (edge_index, frame) in frames.iter().enumerate() {
        let first = frame.start + frame.inward[0] * distance;
        let second = frame.start + frame.inward[1] * distance;
        if frame.exterior.dot(second - first).abs() > tolerance {
            return Err(miter_error(
                feature,
                tolerance,
                EdgeModifierFailureReason::KernelRejected,
                "shared-vertex chamfer setback points do not define one miter plane",
            ));
        }
        planes.push(ConvexMiterPlane {
            origin: Point3::new(
                (first.x + second.x) * 0.5,
                (first.y + second.y) * 0.5,
                (first.z + second.z) * 0.5,
            ),
            normal: frame.exterior,
            source_surface: None,
            edge_index: Some(edge_index),
        });
    }

    let vertex_tolerance = (tolerance * 1.0e-4).max(1.0e-9);
    let mut vertices = convex_polyhedron_vertices(&planes, tolerance, vertex_tolerance);
    if vertices.len() < 4 {
        return Err(miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::ParameterExceedsTopology,
            "shared-vertex chamfer removed the complete source solid",
        ));
    }
    vertices.sort_by(compare_point3);

    let mut compressed_edges = Vec::new();
    let mut edge_map = BTreeMap::<(usize, usize), usize>::new();
    let mut compressed_faces = Vec::with_capacity(planes.len());
    let mut generated = Vec::with_capacity(references.len());
    for plane in &planes {
        let mut polygon = vertices
            .iter()
            .enumerate()
            .filter_map(|(index, point)| {
                (plane.signed_distance(*point).abs() <= tolerance * 2.0).then_some(index)
            })
            .collect::<Vec<_>>();
        if polygon.len() < 3 {
            return Err(miter_error(
                feature,
                tolerance,
                EdgeModifierFailureReason::ParameterExceedsTopology,
                "shared-vertex chamfer collapsed a source or bevel face",
            ));
        }
        sort_polygon(&vertices, &mut polygon, plane.normal).ok_or_else(|| {
            miter_error(
                feature,
                tolerance,
                EdgeModifierFailureReason::InvalidResultTopology,
                "shared-vertex chamfer produced a degenerate face polygon",
            )
        })?;

        let (surface, orientation) = if let Some((surface, orientation)) = &plane.source_surface {
            (surface.clone(), *orientation)
        } else {
            let surface: Surface = Plane::new(
                vertices[polygon[0]],
                vertices[polygon[1]],
                vertices[polygon[2]],
            )
            .into();
            let edge_index = plane
                .edge_index
                .expect("generated miter planes retain their edge index");
            generated.push(topology::generated_face_from_surface(
                &surface,
                &references[edge_index],
                cadx_core::topology::SurfaceKind::Plane,
            ));
            (surface, true)
        };
        if !orientation {
            polygon.reverse();
        }
        let mut boundary = Vec::with_capacity(polygon.len());
        for index in 0..polygon.len() {
            let front = polygon[index];
            let back = polygon[(index + 1) % polygon.len()];
            let key = if front < back {
                (front, back)
            } else {
                (back, front)
            };
            let edge_index = *edge_map.entry(key).or_insert_with(|| {
                let index = compressed_edges.len();
                compressed_edges.push(CompressedEdge {
                    vertices: key,
                    curve: Curve::Line(Line(vertices[key.0], vertices[key.1])),
                });
                index
            });
            boundary.push(CompressedEdgeIndex {
                index: edge_index,
                orientation: (front, back) == key,
            });
        }
        compressed_faces.push(CompressedFace {
            boundaries: vec![boundary],
            orientation,
            surface,
        });
    }

    let shell = Shell::extract(CompressedShell {
        vertices,
        edges: compressed_edges,
        faces: compressed_faces,
    })
    .map_err(|error| {
        miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::InvalidResultTopology,
            format!("shared-vertex chamfer could not rebuild a connected shell: {error}"),
        )
    })?;
    let result = Solid::try_new(vec![shell]).map_err(|error| {
        miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::InvalidResultTopology,
            format!("shared-vertex chamfer did not produce a closed solid: {error}"),
        )
    })?;
    let result = validate_edge_modifier_result(feature, result, "chamfer", tolerance)?;
    if !result.is_geometric_consistent() {
        return Err(miter_error(
            feature,
            tolerance,
            EdgeModifierFailureReason::InvalidResultTopology,
            "shared-vertex chamfer result is not geometrically consistent",
        ));
    }
    Ok((result, generated))
}

fn convex_polyhedron_vertices(
    planes: &[ConvexMiterPlane],
    tolerance: f64,
    vertex_tolerance: f64,
) -> Vec<Point3> {
    let mut vertices = Vec::new();
    for first in 0..planes.len() {
        for second in first + 1..planes.len() {
            for third in second + 1..planes.len() {
                let Some(point) = intersect_three_planes(
                    &planes[first],
                    &planes[second],
                    &planes[third],
                    vertex_tolerance,
                ) else {
                    continue;
                };
                if planes
                    .iter()
                    .any(|plane| plane.signed_distance(point) > tolerance)
                    || vertices.iter().any(|existing| {
                        let delta: Vector3 = point - *existing;
                        delta.magnitude() <= vertex_tolerance
                    })
                {
                    continue;
                }
                vertices.push(point);
            }
        }
    }
    vertices
}

fn intersect_three_planes(
    first: &ConvexMiterPlane,
    second: &ConvexMiterPlane,
    third: &ConvexMiterPlane,
    tolerance: f64,
) -> Option<Point3> {
    let second_cross_third = second.normal.cross(third.normal);
    let determinant = first.normal.dot(second_cross_third);
    if !determinant.is_finite() || determinant.abs() <= tolerance {
        return None;
    }
    let first_offset = first.normal.dot(first.origin - Point3::origin());
    let second_offset = second.normal.dot(second.origin - Point3::origin());
    let third_offset = third.normal.dot(third.origin - Point3::origin());
    let point = (second_cross_third * first_offset
        + third.normal.cross(first.normal) * second_offset
        + first.normal.cross(second.normal) * third_offset)
        / determinant;
    [point.x, point.y, point.z]
        .into_iter()
        .all(f64::is_finite)
        .then(|| Point3::new(point.x, point.y, point.z))
}

fn sort_polygon(points: &[Point3], polygon: &mut [usize], normal: Vector3) -> Option<()> {
    let count = u32::try_from(polygon.len()).ok()?;
    let centroid = polygon
        .iter()
        .fold(Vector3::new(0.0, 0.0, 0.0), |sum, index| {
            sum + (points[*index] - Point3::origin())
        })
        / f64::from(count);
    let reference = if normal.x.abs() < 0.8 {
        Vector3::unit_x()
    } else {
        Vector3::unit_y()
    };
    let first_axis = unit_vector(reference.cross(normal))?;
    let second_axis = normal.cross(first_axis);
    polygon.sort_by(|left, right| {
        let left = points[*left] - Point3::origin() - centroid;
        let right = points[*right] - Point3::origin() - centroid;
        left.dot(second_axis)
            .atan2(left.dot(first_axis))
            .total_cmp(&right.dot(second_axis).atan2(right.dot(first_axis)))
    });
    let first = points[polygon[1]] - points[polygon[0]];
    let second = points[polygon[2]] - points[polygon[1]];
    (first.cross(second).dot(normal) > 0.0).then_some(())
}

fn compare_point3(left: &Point3, right: &Point3) -> std::cmp::Ordering {
    left.x
        .total_cmp(&right.x)
        .then_with(|| left.y.total_cmp(&right.y))
        .then_with(|| left.z.total_cmp(&right.z))
}

fn unit_vector(vector: Vector3) -> Option<Vector3> {
    let length = vector.magnitude();
    (length.is_finite() && length > f64::EPSILON).then(|| vector / length)
}

fn miter_error(
    feature: &Feature,
    tolerance: f64,
    reason: EdgeModifierFailureReason,
    message: impl Into<String>,
) -> KernelError {
    let stage = match reason {
        EdgeModifierFailureReason::NonLinearEdge
        | EdgeModifierFailureReason::NonPlanarSupport
        | EdgeModifierFailureReason::NonConvexSource
        | EdgeModifierFailureReason::ParameterExceedsTopology => {
            EdgeModifierFailureStage::GeometryValidation
        }
        EdgeModifierFailureReason::InvalidResultTopology => {
            EdgeModifierFailureStage::ResultValidation
        }
        _ => EdgeModifierFailureStage::Construction,
    };
    edge_modifier_error(feature, tolerance, stage, reason, None, message)
}

fn reject_shared_modifier_vertices(
    feature: &Feature,
    frames: &[PlanarEdgeFrame],
    tolerance: f64,
    operation: &str,
) -> Result<(), KernelError> {
    if let Some(indices) = shared_modifier_vertex_pair(frames, tolerance) {
        return Err(edge_modifier_error(
            feature,
            tolerance,
            EdgeModifierFailureStage::GeometryValidation,
            EdgeModifierFailureReason::SharedVertexUnsupported,
            Some(indices.to_vec()),
            format!(
                "multi-edge {operation} does not yet support edges sharing a vertex; explicit corner miter construction is required"
            ),
        ));
    }
    Ok(())
}

fn modifier_frames_share_vertex(frames: &[PlanarEdgeFrame], tolerance: f64) -> bool {
    shared_modifier_vertex_pair(frames, tolerance).is_some()
}

fn shared_modifier_vertex_pair(frames: &[PlanarEdgeFrame], tolerance: f64) -> Option<[usize; 2]> {
    for (index, first) in frames.iter().enumerate() {
        let first_endpoints = [first.start, first.start + first.axis * first.length];
        for (second_index, second) in frames[index + 1..].iter().enumerate() {
            let second_endpoints = [second.start, second.start + second.axis * second.length];
            if first_endpoints.iter().any(|first| {
                second_endpoints
                    .iter()
                    .any(|second| (*first - *second).magnitude() <= tolerance)
            }) {
                return Some([index, index + second_index + 1]);
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn boolean_diagnostic(
    feature: &Feature,
    operation: BooleanOperation,
    operands: [FeatureId; 2],
    tolerance_mm: f64,
    left_bounds: Option<AxisAlignedBounds>,
    right_bounds: Option<AxisAlignedBounds>,
    stage: BooleanFailureStage,
    reason: BooleanFailureReason,
    detail: String,
) -> BooleanDiagnostic {
    boolean::diagnostic(
        feature,
        operation,
        operands,
        tolerance_mm,
        left_bounds,
        right_bounds,
        stage,
        reason,
        Vec::new(),
        detail,
    )
}

fn solid_bounds(solid: &Solid) -> Option<AxisAlignedBounds> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut found = false;
    for vertex in solid.vertex_iter() {
        let point = vertex.point();
        let coordinates = [point.x, point.y, point.z];
        if !coordinates.iter().all(|value| value.is_finite()) {
            return None;
        }
        for axis in 0..3 {
            min[axis] = min[axis].min(coordinates[axis]);
            max[axis] = max[axis].max(coordinates[axis]);
        }
        found = true;
    }
    found.then_some(AxisAlignedBounds { min, max })
}

fn supports_geometric_consistency_check(solid: &Solid) -> bool {
    solid.face_iter().all(|face| {
        let surface_is_supported = match face.surface() {
            // Truck's IncludeCurve implementation recurses indefinitely when
            // the revolved entity is a Line, and is unimplemented for an
            // IntersectionCurve. Keep those shapes on CADX's remaining
            // closed-manifold, finite-geometry, tessellation, and naming
            // validation path instead of risking a process-level stack abort.
            Surface::RevolutedCurve(surface) => matches!(
                surface.entity_curve(),
                Curve::BSplineCurve(_) | Curve::NurbsCurve(_)
            ),
            _ => true,
        };
        surface_is_supported
            && face
                .edge_iter()
                .all(|edge| !matches!(edge.curve(), Curve::IntersectionCurve(_)))
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map_or_else(
            || payload.downcast_ref::<&str>().copied(),
            |message| Some(message.as_str()),
        )
        .unwrap_or("non-string panic payload")
}

fn normalized(vector: [f64; 3]) -> Option<[f64; 3]> {
    let length = vector
        .iter()
        .map(|component| component * component)
        .sum::<f64>()
        .sqrt();
    (length.is_finite() && length > f64::EPSILON)
        .then(|| vector.map(|component| component / length))
}

fn resolve_sketch_frame(
    feature: &Feature,
    plane: &SketchPlane,
    datum_frames: &HashMap<FeatureId, SketchFrame>,
    evaluated_parts: &HashMap<FeatureId, EvaluatedPart>,
) -> Result<SketchFrame, KernelError> {
    let frame =
        match plane {
            SketchPlane::WorldXy => SketchFrame::WORLD_XY,
            SketchPlane::WorldXz => SketchFrame::WORLD_XZ,
            SketchPlane::WorldYz => SketchFrame::WORLD_YZ,
            SketchPlane::DatumPlane { datum_id } => datum_frames
                .get(datum_id)
                .copied()
                .ok_or_else(|| KernelError::Evaluation {
                    feature_id: feature.id,
                    message: format!("sketch datum plane {datum_id} was not resolved"),
                })?,
            SketchPlane::PlanarFace { face } => {
                let source = evaluated_parts.get(&face.feature_id).ok_or_else(|| {
                    KernelError::TopologyNaming {
                        feature_id: feature.id,
                        message: format!(
                            "sketch support feature {} was not evaluated",
                            face.feature_id
                        ),
                    }
                })?;
                resolve_planar_face_frame(feature, face, 0.0, true, source, "sketch support")?
            }
        };
    if feature.rotation.x.abs() > f64::EPSILON || feature.rotation.y.abs() > f64::EPSILON {
        return Err(KernelError::Evaluation {
            feature_id: feature.id,
            message: "sketch rotation is limited to the local plane normal (Z)".into(),
        });
    }
    Ok(frame.with_local_transform(feature.translation, feature.rotation.z))
}

fn resolve_datum_frame(
    feature: &Feature,
    face: &FaceRef,
    offset: f64,
    source: &EvaluatedPart,
) -> Result<SketchFrame, KernelError> {
    resolve_planar_face_frame(feature, face, offset, false, source, "datum plane")
}

fn resolve_planar_face_frame(
    feature: &Feature,
    face: &FaceRef,
    offset: f64,
    origin_at_centroid: bool,
    source: &EvaluatedPart,
    subject: &str,
) -> Result<SketchFrame, KernelError> {
    let resolved = match source.resolve_face(face) {
        TopologyResolution::Resolved(face) => face,
        TopologyResolution::Lost => {
            return Err(KernelError::TopologyNaming {
                feature_id: feature.id,
                message: format!("{subject} face reference {face} could not be resolved"),
            });
        }
        TopologyResolution::Ambiguous(candidates) => {
            return Err(KernelError::TopologyNaming {
                feature_id: feature.id,
                message: format!(
                    "{subject} face reference {face} resolved to {} faces",
                    candidates.len()
                ),
            });
        }
    };
    let Some(plane) = resolved.geometry.plane else {
        return Err(KernelError::TopologyNaming {
            feature_id: feature.id,
            message: format!("{subject} face reference {face} is not planar"),
        });
    };
    let Some(normal) = normalized(resolved.geometry.mean_normal) else {
        return Err(KernelError::TopologyNaming {
            feature_id: feature.id,
            message: format!("{subject} face reference {face} has no stable oriented normal"),
        });
    };
    let base_origin = if origin_at_centroid {
        let distance = plane
            .normal
            .iter()
            .enumerate()
            .fold(0.0, |sum, (axis, component)| {
                (resolved.geometry.centroid[axis] - plane.origin[axis]).mul_add(*component, sum)
            });
        std::array::from_fn(|axis| {
            (-distance).mul_add(plane.normal[axis], resolved.geometry.centroid[axis])
        })
    } else {
        plane.origin
    };
    let origin = std::array::from_fn(|axis| normal[axis].mul_add(offset, base_origin[axis]));
    let alignment = plane
        .normal
        .iter()
        .zip(normal)
        .fold(0.0, |sum, (left, right)| left.mul_add(right, sum));
    if alignment.abs() < 1.0 - 1.0e-8 {
        return Err(KernelError::TopologyNaming {
            feature_id: feature.id,
            message: format!(
                "{subject} face reference {face} has inconsistent analytic and oriented normals"
            ),
        });
    }
    let y_dir = if alignment.is_sign_positive() {
        plane.y_direction
    } else {
        plane.y_direction.map(|component| -component)
    };
    Ok(SketchFrame {
        origin,
        x_dir: plane.x_direction,
        y_dir,
        normal,
    })
}

fn convert_compressed_shell(
    feature: &Feature,
    shell: CompressedShell<Point3, Curve3D, StepSurface>,
) -> Result<CompressedShell<Point3, Curve, Surface>, KernelError> {
    let CompressedShell {
        vertices,
        edges,
        faces,
    } = shell;
    let edges = edges
        .into_iter()
        .map(|edge| {
            Ok(CompressedEdge {
                vertices: edge.vertices,
                curve: convert_step_curve(feature, edge.curve)?,
            })
        })
        .collect::<Result<Vec<_>, KernelError>>()?;
    let faces = faces
        .into_iter()
        .map(|face| {
            Ok(CompressedFace {
                boundaries: face.boundaries,
                orientation: face.orientation,
                surface: convert_step_surface(feature, face.surface)?,
            })
        })
        .collect::<Result<Vec<_>, KernelError>>()?;
    Ok(CompressedShell {
        vertices,
        edges,
        faces,
    })
}

fn convert_step_curve(feature: &Feature, curve: Curve3D) -> Result<Curve, KernelError> {
    match curve {
        Curve3D::Line(curve) => Ok(Curve::Line(curve)),
        Curve3D::BSplineCurve(curve) => Ok(Curve::BSplineCurve(curve)),
        Curve3D::NurbsCurve(curve) => Ok(Curve::NurbsCurve(curve)),
        curve => {
            let range = curve.range_tuple();
            truck_modeling::BSplineCurve::cubic_approximation(&curve, range, 1.0e-5, 1.0e-5, 32)
                .map(Curve::BSplineCurve)
                .ok_or_else(|| KernelError::Evaluation {
                    feature_id: feature.id,
                    message: "STEP curve could not be represented by Truck".into(),
                })
        }
    }
}

fn convert_step_surface(feature: &Feature, surface: StepSurface) -> Result<Surface, KernelError> {
    match surface {
        StepSurface::ElementarySurface(surface) => match *surface {
            ElementarySurface::Plane(surface) => Ok(Surface::Plane(surface)),
            ElementarySurface::CylindricalSurface(surface)
            | ElementarySurface::ConicalSurface(surface) => {
                let entity = surface.entity();
                let mapped = truck_modeling::RevolutedCurve::by_revolution(
                    Curve::Line(*entity.entity_curve()),
                    entity.origin(),
                    entity.axis(),
                );
                let mut mapped = Processor::new(mapped).transformed(*surface.transform());
                if !surface.orientation() {
                    mapped.invert();
                }
                Ok(Surface::RevolutedCurve(mapped))
            }
            ElementarySurface::Sphere(surface) => sphere_surface(feature, surface),
            ElementarySurface::ToroidalSurface(surface) => torus_surface(feature, surface),
        },
        StepSurface::BSplineSurface(surface) => Ok(Surface::BSplineSurface(*surface)),
        StepSurface::NurbsSurface(surface) => Ok(Surface::NurbsSurface(*surface)),
        StepSurface::SweptCurve(surface) => match *surface {
            SweptCurve::RevolutedCurve(surface) => {
                let entity = surface.entity().clone();
                let origin = entity.origin();
                let axis = entity.axis();
                let curve = convert_step_curve(feature, entity.into_entity_curve())?;
                let mapped = truck_modeling::RevolutedCurve::by_revolution(curve, origin, axis);
                let mut mapped = Processor::new(mapped).transformed(*surface.transform());
                if !surface.orientation() {
                    mapped.invert();
                }
                Ok(Surface::RevolutedCurve(mapped))
            }
            SweptCurve::ExtrudedCurve(_) => Err(KernelError::Evaluation {
                feature_id: feature.id,
                message: "STEP extruded surface is not supported by Truck".into(),
            }),
        },
    }
}

fn sphere_surface(
    feature: &Feature,
    surface: truck_stepio::r#in::alias::SphericalSurface,
) -> Result<Surface, KernelError> {
    let sphere = surface.entity().0;
    let center = sphere.center();
    let radius = sphere.radius();
    let vertex = builder::vertex(Point3::new(center.x, center.y + radius, center.z));
    let meridian: Wire = builder::rsweep(&vertex, center, Vector3::unit_x(), Rad(TAU / 2.0));
    let shell = builder::cone(&meridian, Vector3::unit_y(), Rad(TAU));
    let Some(base) = shell.face_iter().next() else {
        return Err(KernelError::Evaluation {
            feature_id: feature.id,
            message: "STEP spherical surface could not be reconstructed".into(),
        });
    };
    let mut result = base.surface().transformed(*surface.transform());
    if !surface.orientation() {
        result.invert();
    }
    Ok(result)
}

fn torus_surface(
    feature: &Feature,
    surface: truck_stepio::r#in::alias::ToroidalSurface,
) -> Result<Surface, KernelError> {
    let torus = surface.entity();
    let center = torus.center();
    let section_start = builder::vertex(Point3::new(
        center.x + torus.large_radius(),
        center.y,
        center.z + torus.small_radius(),
    ));
    let section = builder::rsweep(
        &section_start,
        Point3::new(center.x + torus.large_radius(), center.y, center.z),
        Vector3::unit_y(),
        Rad(TAU),
    );
    let shell = builder::rsweep(&section, center, Vector3::unit_z(), Rad(TAU));
    let Some(base) = shell.face_iter().next() else {
        return Err(KernelError::Evaluation {
            feature_id: feature.id,
            message: "STEP toroidal surface could not be reconstructed".into(),
        });
    };
    let mut result = base.surface().transformed(*surface.transform());
    if !surface.orientation() {
        result.invert();
    }
    Ok(result)
}

impl TruckKernel {
    fn kernel_name() -> &'static str {
        "Truck"
    }

    fn kernel_capabilities() -> CadKernelCapabilities {
        let planar_convex_edges = EdgeModifierCapability {
            edge_count: EdgeCountSupport::Multiple,
            source_scope: SourceFeatureScope::Single,
            edge_curves: EdgeCurveSupport::LinearOnly,
            support_surfaces: SupportSurfaceSupport::PlanarOnly,
            edge_convexity: EdgeConvexitySupport::ConvexOnly,
            shared_vertex_support: SharedVertexSupport::Unsupported,
        };
        CadKernelCapabilities {
            chamfer: EdgeModifierCapability {
                shared_vertex_support: SharedVertexSupport::ConvexPolyhedralSource,
                ..planar_convex_edges
            },
            fillet: planar_convex_edges,
            interference_analysis: true,
        }
    }

    fn evaluate_document(
        &self,
        document: &CadDocument,
    ) -> Result<(EvaluatedScene, HashMap<FeatureId, NamedSolid>), KernelError> {
        let graph = document.feature_graph()?;
        let suppressed_features = document.suppressed_assembly_feature_ids()?;
        let shared_imports = shared_imported_step_bodies(document);
        let reusable_render_bodies = reusable_render_bodies(document);
        let mut imported_step_cache = ImportedStepSolidCache::default();
        let mut solids: HashMap<FeatureId, NamedSolid> = HashMap::new();
        let mut evaluated_parts: HashMap<FeatureId, EvaluatedPart> = HashMap::new();
        let mut datum_frames: HashMap<FeatureId, SketchFrame> = HashMap::new();
        let mut sketch_frames: HashMap<FeatureId, SketchFrame> = HashMap::new();
        let mut sketches = Vec::new();
        let mut sketch_diagnostics = Vec::new();
        let mut datum_planes = Vec::new();
        let mut datum_points = Vec::new();
        for id in graph.order() {
            if suppressed_features.contains(id) {
                continue;
            }
            let feature = document
                .feature(*id)
                .ok_or(cadx_core::domain::DocumentError::FeatureNotFound(*id))?;
            if let Primitive::Sketch {
                plane,
                region,
                construction,
                constraints,
            } = &feature.primitive
            {
                let frame = resolve_sketch_frame(feature, plane, &datum_frames, &evaluated_parts)?;
                sketch_frames.insert(feature.id, frame);
                let solved = cadx_sketch::solve_sketch(
                    region,
                    construction,
                    constraints,
                    cadx_sketch::SolverConfig::default(),
                )
                .map_err(|error| KernelError::Evaluation {
                    feature_id: feature.id,
                    message: format!("sketch constraints failed: {error}"),
                })?;
                sketch_diagnostics.push(EvaluatedSketchDiagnostic {
                    feature_id: feature.id,
                    solve: solved.diagnostic.clone(),
                });
                if feature.visible {
                    let constraint_annotations = cadx_sketch::constraint_annotations(
                        &solved.region.profile,
                        &solved.construction,
                        constraints,
                    )
                    .map_err(|error| KernelError::Evaluation {
                        feature_id: feature.id,
                        message: format!("sketch annotations failed: {error}"),
                    })?;
                    sketches.push(EvaluatedSketch {
                        feature_id: feature.id,
                        name: feature.name.clone(),
                        color: feature.color,
                        constraint_annotations,
                        profile: solved
                            .region
                            .profile
                            .sampled_points(PI / 36.0)
                            .into_iter()
                            .map(|point| frame.point_array(point))
                            .collect(),
                        holes: solved
                            .region
                            .holes
                            .iter()
                            .map(|hole| {
                                hole.sampled_points(PI / 36.0)
                                    .into_iter()
                                    .map(|point| frame.point_array(point))
                                    .collect()
                            })
                            .collect(),
                        construction: solved
                            .construction
                            .iter()
                            .map(|segment| {
                                segment
                                    .sampled_points(PI / 36.0)
                                    .into_iter()
                                    .map(|point| frame.point_array(point))
                                    .collect()
                            })
                            .collect(),
                        origin: frame.origin,
                        x_direction: frame.x_dir,
                        y_direction: frame.y_dir,
                        normal: frame.normal,
                    });
                }
                continue;
            }
            if let Primitive::DatumPlane { face, offset } = &feature.primitive {
                let source = evaluated_parts.get(&face.feature_id).ok_or_else(|| {
                    KernelError::TopologyNaming {
                        feature_id: feature.id,
                        message: format!(
                            "datum plane source feature {} was not evaluated",
                            face.feature_id
                        ),
                    }
                })?;
                let frame = resolve_datum_frame(feature, face, *offset, source)?;
                datum_frames.insert(feature.id, frame);
                if feature.visible {
                    datum_planes.push(EvaluatedDatumPlane {
                        feature_id: feature.id,
                        name: feature.name.clone(),
                        color: feature.color,
                        face: face.clone(),
                        origin: frame.origin,
                        x_direction: frame.x_dir,
                        y_direction: frame.y_dir,
                        normal: frame.normal,
                    });
                }
                continue;
            }
            if let Primitive::DatumPoint { vertex, offset } = &feature.primitive {
                let source = evaluated_parts.get(&vertex.feature_id).ok_or_else(|| {
                    KernelError::TopologyNaming {
                        feature_id: feature.id,
                        message: format!(
                            "datum point source feature {} was not evaluated",
                            vertex.feature_id
                        ),
                    }
                })?;
                let resolved = match source.resolve_vertex(vertex) {
                    TopologyResolution::Resolved(vertex) => vertex,
                    TopologyResolution::Lost => {
                        return Err(KernelError::TopologyNaming {
                            feature_id: feature.id,
                            message: format!(
                                "datum point vertex reference {vertex} could not be resolved"
                            ),
                        });
                    }
                    TopologyResolution::Ambiguous(candidates) => {
                        return Err(KernelError::TopologyNaming {
                            feature_id: feature.id,
                            message: format!(
                                "datum point vertex reference {vertex} resolved to {} vertices",
                                candidates.len()
                            ),
                        });
                    }
                };
                let offset = offset.as_array();
                if feature.visible {
                    datum_points.push(EvaluatedDatumPoint {
                        feature_id: feature.id,
                        name: feature.name.clone(),
                        color: feature.color,
                        vertex: vertex.clone(),
                        position: std::array::from_fn(|axis| {
                            resolved.geometry.position[axis] + offset[axis]
                        }),
                    });
                }
                continue;
            }
            let evaluated = match &feature.primitive {
                Primitive::Boolean {
                    operation,
                    left,
                    right,
                } => match catch_unwind(AssertUnwindSafe(|| {
                    (*self).evaluate_feature(
                        feature,
                        document,
                        &solids,
                        &sketch_frames,
                        &shared_imports,
                        &mut imported_step_cache,
                    )
                })) {
                    Ok(evaluated) => evaluated,
                    Err(payload) => Err(boolean_diagnostic(
                        feature,
                        *operation,
                        [*left, *right],
                        self.boolean_tolerance_policy.absolute_mm,
                        None,
                        None,
                        BooleanFailureStage::ResultValidation,
                        BooleanFailureReason::KernelPanic,
                        format!(
                            "Truck panicked while evaluating the boolean result: {}",
                            panic_message(payload.as_ref())
                        ),
                    )
                    .into()),
                },
                Primitive::Chamfer { .. } | Primitive::Fillet { .. } => {
                    match catch_unwind(AssertUnwindSafe(|| {
                        (*self).evaluate_feature(
                            feature,
                            document,
                            &solids,
                            &sketch_frames,
                            &shared_imports,
                            &mut imported_step_cache,
                        )
                    })) {
                        Ok(evaluated) => evaluated,
                        Err(payload) => Err(edge_modifier_error(
                            feature,
                            self.tolerance,
                            EdgeModifierFailureStage::ResultValidation,
                            EdgeModifierFailureReason::KernelPanic,
                            None,
                            format!(
                                "Truck panicked while evaluating the edge modifier result: {}",
                                panic_message(payload.as_ref())
                            ),
                        )),
                    }
                }
                _ => (*self).evaluate_feature(
                    feature,
                    document,
                    &solids,
                    &sketch_frames,
                    &shared_imports,
                    &mut imported_step_cache,
                ),
            };
            let (solid, part) = evaluated?;
            solids.insert(*id, solid);
            evaluated_parts.insert(*id, part);
        }
        let parts = graph
            .order()
            .iter()
            .filter_map(|id| {
                document
                    .feature(*id)
                    .is_some_and(|feature| feature.visible)
                    .then(|| evaluated_parts.remove(id))
                    .flatten()
            })
            .collect::<Vec<_>>();
        let (mesh_definitions, mesh_instances) =
            instanced_render_geometry(document, &parts, &reusable_render_bodies);
        let scene = EvaluatedScene {
            parts,
            mesh_definitions,
            mesh_instances,
            sketches,
            sketch_diagnostics,
            datum_planes,
            datum_points,
        };
        Ok((scene, solids))
    }

    fn interference_pair_outcome(self, left: &Solid, right: &Solid) -> InterferencePairOutcome {
        let precision = InterferenceVolumePrecision::Tessellated {
            chord_tolerance_mm: self.tolerance,
        };
        let proven_non_crossing = catch_unwind(AssertUnwindSafe(|| {
            boolean::resolve_non_crossing_intersection(left, right, self.tolerance)
        }));
        match proven_non_crossing {
            Ok(Some(Ok(boolean::NonCrossingIntersection::Empty))) => {
                return InterferencePairOutcome::Clear {
                    volume_mm3: 0.0,
                    precision,
                    method: InterferenceIntersectionMethod::NonCrossingContact,
                };
            }
            Ok(Some(Ok(boolean::NonCrossingIntersection::Solid(solid)))) => {
                return self.interference_volume_outcome(
                    &solid,
                    InterferenceIntersectionMethod::NonCrossingContact,
                );
            }
            Ok(Some(Err(detail))) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::ResultValidation,
                    reason: InterferenceFailureReason::InvalidResultTopology,
                    detail,
                };
            }
            Ok(None) => {}
            Err(payload) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::ResultValidation,
                    reason: InterferenceFailureReason::KernelPanic,
                    detail: format!(
                        "Truck panicked while classifying non-crossing contact: {}",
                        panic_message(payload.as_ref())
                    ),
                };
            }
        }
        if let Some(solid) = strictly_contained_intersection(left, right, self.tolerance) {
            return self.interference_volume_outcome(
                &solid,
                InterferenceIntersectionMethod::BoundaryClassifiedContainment,
            );
        }

        let intersection = catch_unwind(AssertUnwindSafe(|| {
            truck_shapeops::and(left, right, self.tolerance)
        }));
        let intersection = match intersection {
            Ok(Some(solid)) => solid,
            Ok(None) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::KernelOperation,
                    reason: InterferenceFailureReason::KernelRejected,
                    detail: "Truck returned no intersection without proving the pair clear".into(),
                };
            }
            Err(payload) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::KernelOperation,
                    reason: InterferenceFailureReason::KernelPanic,
                    detail: format!(
                        "Truck panicked while intersecting product bodies: {}",
                        panic_message(payload.as_ref())
                    ),
                };
            }
        };
        self.interference_volume_outcome(&intersection, InterferenceIntersectionMethod::BrepBoolean)
    }

    fn interference_volume_outcome(
        self,
        intersection: &Solid,
        method: InterferenceIntersectionMethod,
    ) -> InterferencePairOutcome {
        let precision = InterferenceVolumePrecision::Tessellated {
            chord_tolerance_mm: self.tolerance,
        };
        if let Err(error) = Solid::try_new(intersection.boundaries().clone()) {
            return InterferencePairOutcome::Failed {
                stage: InterferenceFailureStage::ResultValidation,
                reason: InterferenceFailureReason::InvalidResultTopology,
                detail: format!("intersection is not a closed manifold solid: {error}"),
            };
        }
        let consistency = catch_unwind(AssertUnwindSafe(|| {
            !supports_geometric_consistency_check(intersection)
                || intersection.is_geometric_consistent()
        }));
        match consistency {
            Ok(true) => {}
            Ok(false) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::ResultValidation,
                    reason: InterferenceFailureReason::InvalidResultTopology,
                    detail: "intersection is not geometrically consistent".into(),
                };
            }
            Err(payload) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::ResultValidation,
                    reason: InterferenceFailureReason::KernelPanic,
                    detail: format!(
                        "Truck panicked while validating the intersection: {}",
                        panic_message(payload.as_ref())
                    ),
                };
            }
        }

        let polygon = match catch_unwind(AssertUnwindSafe(|| {
            intersection.triangulation(self.tolerance).to_polygon()
        })) {
            Ok(polygon) => polygon,
            Err(payload) => {
                return InterferencePairOutcome::Failed {
                    stage: InterferenceFailureStage::VolumeIntegration,
                    reason: InterferenceFailureReason::KernelPanic,
                    detail: format!(
                        "Truck panicked while tessellating the intersection: {}",
                        panic_message(payload.as_ref())
                    ),
                };
            }
        };
        if polygon.tri_faces().is_empty() {
            return InterferencePairOutcome::Failed {
                stage: InterferenceFailureStage::VolumeIntegration,
                reason: InterferenceFailureReason::EmptyMesh,
                detail: "the exact intersection produced no volume-integration triangles".into(),
            };
        }
        let volume_mm3 = polygon.volume().abs();
        if !volume_mm3.is_finite() {
            return InterferencePairOutcome::Failed {
                stage: InterferenceFailureStage::VolumeIntegration,
                reason: InterferenceFailureReason::NonFiniteVolume,
                detail: "intersection volume is non-finite".into(),
            };
        }
        if volume_mm3 > self.tolerance.powi(3) {
            InterferencePairOutcome::Interfering {
                volume_mm3,
                precision,
                method,
            }
        } else {
            InterferencePairOutcome::Clear {
                volume_mm3,
                precision,
                method,
            }
        }
    }

    fn analyze_document_interference(
        self,
        document: &CadDocument,
    ) -> Result<InterferenceAnalysis, KernelError> {
        let (_, solids) = self.evaluate_document(document)?;
        let graph = document.feature_graph()?;
        let assembly_instances = document.assembly_feature_instances();
        let mut candidate_feature_ids = graph
            .order()
            .iter()
            .copied()
            .filter(|id| solids.contains_key(id))
            .filter(|id| {
                !graph.dependents(*id).iter().any(|dependent| {
                    solids.contains_key(dependent)
                        && document.feature(*dependent).is_some_and(|feature| {
                            matches!(
                                feature.primitive,
                                Primitive::Boolean { .. }
                                    | Primitive::Chamfer { .. }
                                    | Primitive::Fillet { .. }
                            )
                        })
                })
            })
            .collect::<Vec<_>>();
        candidate_feature_ids.sort_unstable();

        let mut bounds = BTreeMap::new();
        for id in &candidate_feature_ids {
            let solid = &solids
                .get(id)
                .expect("candidate ids are selected from evaluated solids")
                .solid;
            let Some(value) = solid_bounds(solid) else {
                return Err(KernelError::Analysis {
                    message: format!("product body {id} has no finite axis-aligned bounds"),
                });
            };
            bounds.insert(*id, value);
        }

        let mut report = InterferenceAnalysis {
            candidate_feature_ids,
            volume_tolerance_mm3: self.tolerance.powi(3),
            ..InterferenceAnalysis::default()
        };
        for (left_index, left_id) in report.candidate_feature_ids.iter().copied().enumerate() {
            for right_id in report.candidate_feature_ids[left_index + 1..]
                .iter()
                .copied()
            {
                let same_occurrence = assembly_instances
                    .get(&left_id)
                    .zip(assembly_instances.get(&right_id))
                    .is_some_and(|(left, right)| {
                        left.assembly_id == right.assembly_id
                            && left.occurrence_id == right.occurrence_id
                    });
                if same_occurrence {
                    continue;
                }
                report.total_pair_count += 1;
                let pair_bounds = [bounds[&left_id], bounds[&right_id]];
                if pair_bounds[0].is_disjoint_from(pair_bounds[1], 0.0) {
                    report.clear_pair_count += 1;
                    continue;
                }
                report.broad_phase_pair_count += 1;
                let outcome = self
                    .interference_pair_outcome(&solids[&left_id].solid, &solids[&right_id].solid);
                match outcome {
                    InterferencePairOutcome::Clear { .. } => report.clear_pair_count += 1,
                    InterferencePairOutcome::Interfering { .. } => {
                        report.interfering_pair_count += 1;
                    }
                    InterferencePairOutcome::Failed { .. } => report.failed_pair_count += 1,
                }
                report.pairs.push(InterferencePairAnalysis {
                    feature_ids: [left_id, right_id],
                    bounds: pair_bounds,
                    outcome,
                });
            }
        }
        Ok(report)
    }
}

fn strictly_contained_intersection(left: &Solid, right: &Solid, tolerance: f64) -> Option<Solid> {
    let left_bounds = solid_bounds(left)?;
    let right_bounds = solid_bounds(right)?;
    if bounds_strictly_contain(right_bounds, left_bounds, tolerance)
        && boundary_is_inside(left, right, tolerance)
    {
        return Some(left.clone());
    }
    if bounds_strictly_contain(left_bounds, right_bounds, tolerance)
        && boundary_is_inside(right, left, tolerance)
    {
        return Some(right.clone());
    }
    None
}

fn bounds_strictly_contain(
    outer: AxisAlignedBounds,
    inner: AxisAlignedBounds,
    tolerance: f64,
) -> bool {
    (0..3).all(|axis| {
        inner.min[axis] - outer.min[axis] > tolerance
            && outer.max[axis] - inner.max[axis] > tolerance
    })
}

fn boundary_is_inside(candidate: &Solid, host: &Solid, tolerance: f64) -> bool {
    let meshes = catch_unwind(AssertUnwindSafe(|| {
        [
            candidate.triangulation(tolerance).to_polygon(),
            host.triangulation(tolerance).to_polygon(),
        ]
    }));
    let Ok([candidate_mesh, host_mesh]) = meshes else {
        return false;
    };
    if candidate_mesh.positions().is_empty()
        || host_mesh.positions().is_empty()
        || candidate_mesh.collide_with(&host_mesh).is_some()
    {
        return false;
    }
    candidate_mesh
        .positions()
        .iter()
        .all(|point| host_mesh.inside(*point))
}

impl CadKernel for TruckKernel {
    fn name(&self) -> &'static str {
        Self::kernel_name()
    }

    fn capabilities(&self) -> CadKernelCapabilities {
        Self::kernel_capabilities()
    }

    fn evaluate(&self, document: &CadDocument) -> Result<EvaluatedScene, KernelError> {
        self.evaluate_document(document).map(|(scene, _)| scene)
    }

    fn analyze_interference(
        &self,
        document: &CadDocument,
    ) -> Result<InterferenceAnalysis, KernelError> {
        (*self).analyze_document_interference(document)
    }
}

impl ExchangeKernel for TruckKernel {
    fn encode_step(&self, document: &CadDocument, file_name: &str) -> Result<String, KernelError> {
        const MAX_ATTEMPTS: usize = 16;

        let mut last_error = None;
        for _ in 0..MAX_ATTEMPTS {
            let source = encode_step_candidate(*self, document, file_name)?;
            match validate_step_topology(&source, self.tolerance) {
                Ok(()) => return Ok(source),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| KernelError::Exchange {
            format: "STEP",
            message: "Truck could not produce valid topology".into(),
        }))
    }
}

fn encode_step_candidate(
    kernel: TruckKernel,
    document: &CadDocument,
    file_name: &str,
) -> Result<String, KernelError> {
    let assembly_plan = AssemblyStepExportPlan::new(document)?;
    let graph = document.feature_graph()?;
    let suppressed_features = document.suppressed_assembly_feature_ids()?;
    let shared_imports = shared_imported_step_bodies(document);
    let mut imported_step_cache = ImportedStepSolidCache::default();
    let mut upstream = HashMap::new();
    let mut evaluated_parts = HashMap::new();
    let mut datum_frames = HashMap::new();
    let mut sketch_frames = HashMap::new();
    let mut exported = Vec::new();
    let mut exported_colors = Vec::new();
    let mut exported_owners = Vec::new();
    for id in graph.order() {
        if suppressed_features.contains(id) {
            continue;
        }
        let feature = document
            .feature(*id)
            .ok_or(cadx_core::domain::DocumentError::FeatureNotFound(*id))?;
        if let Primitive::Sketch { plane, .. } = &feature.primitive {
            let frame = resolve_sketch_frame(feature, plane, &datum_frames, &evaluated_parts)?;
            sketch_frames.insert(feature.id, frame);
            continue;
        }
        if let Primitive::DatumPlane { face, offset } = &feature.primitive {
            let source = evaluated_parts.get(&face.feature_id).ok_or_else(|| {
                KernelError::TopologyNaming {
                    feature_id: feature.id,
                    message: format!(
                        "datum plane source feature {} was not evaluated",
                        face.feature_id
                    ),
                }
            })?;
            let frame = resolve_datum_frame(feature, face, *offset, source)?;
            datum_frames.insert(feature.id, frame);
            continue;
        }
        if matches!(feature.primitive, Primitive::DatumPoint { .. }) {
            continue;
        }
        let (solid, part) = kernel.evaluate_feature(
            feature,
            document,
            &upstream,
            &sketch_frames,
            &shared_imports,
            &mut imported_step_cache,
        )?;
        if feature.visible
            && let Some(owner) = assembly_plan.body_owner(feature.id)
        {
            let localized = match owner {
                StepExportBodyOwner::AssemblyDefinition(_) => {
                    let world_transform = AssemblyTransform::from_euler_xyz_degrees(
                        feature.translation.as_array(),
                        feature.rotation.as_array(),
                    );
                    Some(builder::transformed(
                        &solid.solid,
                        assembly_transform_matrix(world_transform.inverse()),
                    ))
                }
                StepExportBodyOwner::Standalone(_) => None,
            };
            let export_solid = localized.as_ref().unwrap_or(&solid.solid);
            for step_solid in partition_step_export_solids(feature, export_solid, kernel.tolerance)?
            {
                exported.push(step_solid.compress());
                exported_colors.push(feature.color);
                exported_owners.push(owner);
            }
        }
        upstream.insert(*id, solid);
        evaluated_parts.insert(*id, part);
    }
    if exported.is_empty() {
        return Err(KernelError::Exchange {
            format: "STEP",
            message: "document contains no visible solid bodies".into(),
        });
    }

    let models: StepModels<'_, Point3, Curve, Surface> = exported.iter().collect();
    let header = StepHeaderDescriptor {
        file_name: encode_step_string(file_name),
        organization_system: "CADX".into(),
        ..StepHeaderDescriptor::default()
    };
    let mut source = String::new();
    write!(
        &mut source,
        "ISO-10303-21;\n\
HEADER;\n\
FILE_DESCRIPTION(('CADX B-Rep model'), '2;1');\n\
FILE_NAME('{}', '{}', (''), (''), 'CADX', 'CADX', '');\n\
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
ENDSEC;\n\
DATA;\n\
{models}\
ENDSEC;\n\
END-ISO-10303-21;\n",
        header.file_name, header.time_stamp
    )
    .map_err(|_| KernelError::Exchange {
        format: "STEP",
        message: "Truck rejected the B-Rep topology".into(),
    })?;
    source = append_step_body_colors(source, &exported_colors)?;
    let body_targets = step_body_target_ids(&source)?;
    append_ap242_product_structure(
        source,
        document,
        &assembly_plan,
        &exported_owners,
        &body_targets,
    )
}

fn assembly_transform_matrix(transform: AssemblyTransform) -> Matrix4 {
    let rotation = transform.rotation;
    let [x, y, z] = transform.translation;
    Matrix4::new(
        rotation[0][0],
        rotation[1][0],
        rotation[2][0],
        0.0,
        rotation[0][1],
        rotation[1][1],
        rotation[2][1],
        0.0,
        rotation[0][2],
        rotation[1][2],
        rotation[2][2],
        0.0,
        x,
        y,
        z,
        1.0,
    )
}

fn step_body_target_ids(source: &str) -> Result<Vec<u64>, KernelError> {
    use truck_stepio::r#in::ruststep::ast::EntityInstance;

    let exchange = truck_stepio::r#in::ruststep::parser::parse(source).map_err(|error| {
        KernelError::Exchange {
            format: "STEP",
            message: format!("generated model could not be parsed for body export: {error}"),
        }
    })?;
    let data = exchange.data.first().ok_or_else(|| KernelError::Exchange {
        format: "STEP",
        message: "generated model has no DATA section for body export".into(),
    })?;
    Ok(data
        .entities
        .iter()
        .filter_map(|entity| match entity {
            EntityInstance::Simple { id, record }
                if matches!(
                    record.name.as_str(),
                    "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS"
                ) =>
            {
                Some(*id)
            }
            EntityInstance::Complex { id, subsuper }
                if subsuper.0.iter().any(|record| {
                    matches!(
                        record.name.as_str(),
                        "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS"
                    )
                }) =>
            {
                Some(*id)
            }
            _ => None,
        })
        .collect())
}

fn append_step_body_colors(mut source: String, colors: &[[f32; 4]]) -> Result<String, KernelError> {
    const SUFFIX: &str = "ENDSEC;\nEND-ISO-10303-21;\n";

    use truck_stepio::r#in::ruststep::ast::EntityInstance;

    let exchange = truck_stepio::r#in::ruststep::parser::parse(&source).map_err(|error| {
        KernelError::Exchange {
            format: "STEP",
            message: format!("generated model could not be parsed for color export: {error}"),
        }
    })?;
    let data = exchange.data.first().ok_or_else(|| KernelError::Exchange {
        format: "STEP",
        message: "generated model has no DATA section for color export".into(),
    })?;
    let entity_id = |entity: &EntityInstance| match entity {
        EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
    };
    let targets = step_body_target_ids(&source)?;
    if targets.len() != colors.len() {
        return Err(KernelError::Exchange {
            format: "STEP",
            message: format!(
                "generated model has {} solid records for {} visible feature colors",
                targets.len(),
                colors.len()
            ),
        });
    }

    let mut next_id = data
        .entities
        .iter()
        .map(entity_id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| KernelError::Exchange {
            format: "STEP",
            message: "generated model exhausted STEP entity ids while exporting colors".into(),
        })?;
    let mut styles = String::new();
    let mut styled_items = Vec::with_capacity(colors.len());
    for (target, color) in targets.into_iter().zip(colors) {
        if !color
            .iter()
            .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel))
        {
            return Err(KernelError::Exchange {
                format: "STEP",
                message: "visible feature has invalid RGBA color".into(),
            });
        }
        let first_id = next_id;
        next_id = first_id
            .checked_add(9)
            .ok_or_else(|| KernelError::Exchange {
                format: "STEP",
                message: "generated model exhausted STEP entity ids while exporting colors".into(),
            })?;
        let ids = [
            first_id,
            first_id + 1,
            first_id + 2,
            first_id + 3,
            first_id + 4,
            first_id + 5,
            first_id + 6,
            first_id + 7,
            first_id + 8,
        ];
        let transparency = 1.0 - color[3];
        writeln!(
            &mut styles,
            "#{}=COLOUR_RGB('CADX body color',{:.9},{:.9},{:.9});",
            ids[0], color[0], color[1], color[2]
        )
        .expect("writing STEP color to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=FILL_AREA_STYLE_COLOUR('',#{});",
            ids[1], ids[0]
        )
        .expect("writing STEP fill color to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=FILL_AREA_STYLE('',(#{}));",
            ids[2], ids[1]
        )
        .expect("writing STEP fill style to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=SURFACE_STYLE_FILL_AREA(#{});",
            ids[3], ids[2]
        )
        .expect("writing STEP surface fill style to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=SURFACE_STYLE_TRANSPARENT({transparency:.9});",
            ids[4]
        )
        .expect("writing STEP transparency style to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=SURFACE_SIDE_STYLE('',(#{} ,#{}));",
            ids[5], ids[3], ids[4]
        )
        .expect("writing STEP side style to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=SURFACE_STYLE_USAGE(.BOTH.,#{});",
            ids[6], ids[5]
        )
        .expect("writing STEP style usage to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=PRESENTATION_STYLE_ASSIGNMENT((#{}));",
            ids[7], ids[6]
        )
        .expect("writing STEP presentation style to a String cannot fail");
        writeln!(
            &mut styles,
            "#{}=STYLED_ITEM('',(#{}),#{});",
            ids[8], ids[7], target
        )
        .expect("writing STEP styled item to a String cannot fail");
        styled_items.push(ids[8]);
    }
    let presentation_id = next_id;
    let styled_items = styled_items
        .iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(",");
    writeln!(
        &mut styles,
        "#{presentation_id}=MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({styled_items}),#11);"
    )
    .expect("writing STEP presentation representation to a String cannot fail");

    let insert_at =
        source
            .strip_suffix(SUFFIX)
            .map(str::len)
            .ok_or_else(|| KernelError::Exchange {
                format: "STEP",
                message: "generated model has an unexpected physical-file trailer".into(),
            })?;
    source.insert_str(insert_at, &styles);
    Ok(source)
}

fn validate_step_topology(source: &str, tolerance: f64) -> Result<(), KernelError> {
    let table = Table::from_step(source).ok_or_else(|| KernelError::Exchange {
        format: "STEP",
        message: "Truck could not parse generated topology".into(),
    })?;
    if table.shell.is_empty() {
        return Err(KernelError::Exchange {
            format: "STEP",
            message: "generated model contains no shells".into(),
        });
    }
    for shell in table.shell.values() {
        let shell = table
            .to_compressed_shell(shell)
            .map_err(|error| KernelError::Exchange {
                format: "STEP",
                message: format!("generated shell could not be reconstructed: {error}"),
            })?;
        if shell
            .triangulation(tolerance)
            .to_polygon()
            .tri_faces()
            .is_empty()
        {
            return Err(KernelError::Exchange {
                format: "STEP",
                message: "generated shell has no reconstructable faces".into(),
            });
        }
    }
    Ok(())
}

fn encode_step_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut unicode = false;
    for character in value.chars() {
        let printable_ascii = character.is_ascii() && !character.is_ascii_control();
        if printable_ascii {
            if unicode {
                output.push_str("\\X0\\");
                unicode = false;
            }
            match character {
                '\'' => output.push('_'),
                '\\' => output.push_str("\\\\"),
                _ => output.push(character),
            }
        } else {
            if !unicode {
                output.push_str("\\X2\\");
                unicode = true;
            }
            let mut units = [0; 2];
            for unit in character.encode_utf16(&mut units) {
                write!(&mut output, "{unit:04X}").expect("writing to a String cannot fail");
            }
        }
    }
    if unicode {
        output.push_str("\\X0\\");
    }
    output
}

#[allow(clippy::cast_possible_truncation)]
fn polygon_to_render_mesh(polygon: &PolygonMesh) -> Result<TriangleMesh, KernelError> {
    let mut mesh = TriangleMesh::default();
    let triangle_count = polygon.tri_faces().len();
    let vertex_count = triangle_count
        .checked_mul(3)
        .ok_or(KernelError::MeshTooLarge)?;
    if vertex_count > u32::MAX as usize {
        return Err(KernelError::MeshTooLarge);
    }
    mesh.positions.reserve(vertex_count);
    mesh.normals.reserve(vertex_count);
    mesh.indices.reserve(vertex_count);

    for triangle in polygon.tri_faces() {
        let points = triangle.map(|vertex| polygon.positions()[vertex.pos]);
        let a = [points[0].x, points[0].y, points[0].z];
        let b = [points[1].x, points[1].y, points[1].z];
        let c = [points[2].x, points[2].y, points[2].z];
        let normal = face_normal(a, b, c);
        for point in [a, b, c] {
            mesh.positions
                .push([point[0] as f32, point[1] as f32, point[2] as f32]);
            mesh.normals.push(normal);
            let index = u32::try_from(mesh.indices.len()).map_err(|_| KernelError::MeshTooLarge)?;
            mesh.indices.push(index);
        }
    }
    Ok(mesh)
}

#[allow(clippy::cast_possible_truncation)]
fn face_normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let length = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
    if length <= f64::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [
            (cross[0] / length) as f32,
            (cross[1] / length) as f32,
            (cross[2] / length) as f32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use cadx_core::assembly::{
        AssemblyTransform, ComponentDefinition, ComponentKind, ComponentOccurrence,
    };
    use cadx_core::domain::{BooleanOperation, Constraint, ModelCommand, SketchPlane};
    use cadx_core::kernel::EvaluatedPart;
    use cadx_core::topology::{
        CurveKind, EdgeRef, FaceName, FaceRef, PrimitiveFace, SurfaceKind, VertexRef,
    };
    use cadx_io::{encode_3mf, encode_binary_stl, parse_step, validate_3mf, validate_step};

    fn face_references(part: &EvaluatedPart) -> BTreeSet<FaceRef> {
        part.faces
            .iter()
            .map(|face| face.reference.clone())
            .collect()
    }

    fn imported_box_step(size: [f64; 3]) -> (String, u64) {
        let mut source_document = CadDocument::default();
        source_document
            .apply(ModelCommand::CreateBox {
                name: "definition body".into(),
                size,
                position: [0.0; 3],
            })
            .unwrap();
        let source = TruckKernel::default()
            .encode_step(&source_document, "definition.step")
            .unwrap();
        let shell_id = *Table::from_step(&source)
            .unwrap()
            .shell
            .keys()
            .next()
            .unwrap();
        (source, shell_id)
    }

    fn repeated_native_box_assembly() -> (
        CadDocument,
        [FeatureId; 2],
        AssemblyTransform,
        [AssemblyTransform; 2],
    ) {
        let root_transform =
            AssemblyTransform::from_euler_xyz_degrees([5.0, 6.0, 7.0], [0.0, 0.0, 90.0]);
        let local_transforms = [
            AssemblyTransform::from_euler_xyz_degrees([10.0, 0.0, 0.0], [0.0; 3]),
            AssemblyTransform::from_euler_xyz_degrees([0.0, 20.0, 0.0], [0.0, 0.0, 90.0]),
        ];
        let world_transforms = local_transforms.map(|local| root_transform.compose(local));
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction(world_transforms.iter().enumerate().map(|(index, world)| {
                ModelCommand::CreateBox {
                    name: format!("bracket body {}", index + 1),
                    size: [2.0, 3.0, 4.0],
                    position: world.translation,
                }
            }))
            .unwrap();
        for (id, world) in ids.iter().zip(world_transforms) {
            document
                .apply(ModelCommand::Rotate {
                    id: *id,
                    rotation: world.euler_xyz_degrees(),
                })
                .unwrap();
            document
                .apply(ModelCommand::SetColor {
                    id: *id,
                    color: [0.2, 0.4, 0.8, 1.0],
                })
                .unwrap();
        }
        let ids = [ids[0], ids[1]];
        document
            .apply(ModelCommand::CreateAssembly {
                name: "Export fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "Top assembly".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "Bracket part".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "Root placement".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: root_transform,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "Bracket left".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: local_transforms[0],
                        feature_ids: vec![ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "Bracket right".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: local_transforms[1],
                        feature_ids: vec![ids[1]],
                        source: None,
                    },
                ],
            })
            .unwrap();
        (document, ids, root_transform, local_transforms)
    }

    fn repeated_imported_box_assembly() -> (CadDocument, [FeatureId; 2]) {
        let (source, shell_id) = imported_box_step([10.0; 3]);
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::ImportStep {
                    name: "bracket 1".into(),
                    source: source.clone(),
                    data_section: 0,
                    shell_id,
                    void_shells: Vec::new(),
                    length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                    color: None,
                    position: [0.0; 3],
                },
                ModelCommand::ImportStep {
                    name: "bracket 2".into(),
                    source,
                    data_section: 0,
                    shell_id,
                    void_shells: Vec::new(),
                    length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                    color: Some([0.2, 0.4, 0.8, 1.0]),
                    position: [4.0, 3.0, 2.0],
                },
            ])
            .unwrap();
        let ids = [ids[0], ids[1]];
        document
            .apply(ModelCommand::CreateAssembly {
                name: "fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "fixture".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "bracket".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "fixture".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "bracket 1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: vec![ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "bracket 2".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [4.0, 3.0, 2.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![ids[1]],
                        source: None,
                    },
                ],
            })
            .unwrap();
        (document, ids)
    }

    fn repeated_multi_body_assembly() -> (CadDocument, [[FeatureId; 2]; 2]) {
        let bodies = [imported_box_step([10.0; 3]), imported_box_step([6.0; 3])];
        let mut document = CadDocument::default();
        let commands = [([0.0; 3], "1"), ([20.0, 0.0, 0.0], "2")]
            .into_iter()
            .flat_map(|(position, occurrence)| {
                bodies
                    .iter()
                    .enumerate()
                    .map(move |(body_slot, body)| ModelCommand::ImportStep {
                        name: format!("component {occurrence} body {}", body_slot + 1),
                        source: body.0.clone(),
                        data_section: 0,
                        shell_id: body.1,
                        void_shells: Vec::new(),
                        length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                        color: None,
                        position,
                    })
            });
        let ids = document.apply_transaction(commands).unwrap();
        let ids = [[ids[0], ids[1]], [ids[2], ids[3]]];
        document
            .apply(ModelCommand::CreateAssembly {
                name: "multi-body fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "fixture".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "two-body component".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "fixture".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "component 1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: ids[0].to_vec(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "component 2".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [20.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: ids[1].to_vec(),
                        source: None,
                    },
                ],
            })
            .unwrap();
        (document, ids)
    }

    fn circle_loop(center: [f64; 2], radius: f64, ccw: bool) -> SketchLoop2D {
        let right = [center[0] + radius, center[1]];
        let left = [center[0] - radius, center[1]];
        SketchLoop2D {
            segments: vec![
                SketchSegment2D::Arc {
                    start: right,
                    end: left,
                    center,
                    ccw,
                },
                SketchSegment2D::Arc {
                    start: left,
                    end: right,
                    center,
                    ccw,
                },
            ],
        }
    }

    fn edge_references(part: &EvaluatedPart) -> BTreeSet<EdgeRef> {
        part.edges
            .iter()
            .map(|edge| edge.reference.clone())
            .collect()
    }

    fn vertex_references(part: &EvaluatedPart) -> BTreeSet<VertexRef> {
        part.vertices
            .iter()
            .map(|vertex| vertex.reference.clone())
            .collect()
    }

    fn incident_edge_pair(part: &EvaluatedPart) -> Option<[EdgeRef; 2]> {
        part.edges.iter().enumerate().find_map(|(index, first)| {
            part.edges[index + 1..].iter().find_map(|second| {
                first
                    .geometry
                    .endpoints
                    .iter()
                    .any(|first| {
                        second.geometry.endpoints.iter().any(|second| {
                            first
                                .iter()
                                .zip(second)
                                .all(|(first, second)| (*first - *second).abs() < 1.0e-8)
                        })
                    })
                    .then(|| [first.reference.clone(), second.reference.clone()])
            })
        })
    }

    fn mesh_bounds(mesh: &TriangleMesh) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for point in &mesh.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        (min, max)
    }

    fn assert_topology_partition(part: &EvaluatedPart) {
        let mut triangle = 0_u32;
        let mut references = BTreeSet::new();
        for face in &part.faces {
            assert_eq!(face.triangles.start, triangle);
            assert!(face.triangles.end > face.triangles.start);
            triangle = face.triangles.end;
            assert!(references.insert(face.reference.clone()));
            assert!(face.geometry.area.is_finite() && face.geometry.area > 0.0);
            assert!(face.geometry.centroid.iter().all(|value| value.is_finite()));
            assert!(
                face.geometry
                    .mean_normal
                    .iter()
                    .all(|value| value.is_finite())
            );
            if face.geometry.surface == SurfaceKind::Plane {
                let plane = face
                    .geometry
                    .plane
                    .expect("planar B-Rep faces must expose an analytic equation");
                assert!(plane.origin.iter().all(|value| value.is_finite()));
                let normal_length = plane
                    .normal
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt();
                assert!((normal_length - 1.0).abs() < 1.0e-10);
            }
        }
        assert_eq!(triangle as usize, part.mesh.triangle_count());

        let edge_references = edge_references(part);
        assert_eq!(edge_references.len(), part.edges.len());
        for edge in &part.edges {
            assert!(part.edge(&edge.reference).is_some());
            assert!(
                edge.reference
                    .adjacent_faces
                    .iter()
                    .all(|reference| part.face(reference).is_some())
            );
            assert!(edge.geometry.length.is_finite() && edge.geometry.length > 0.0);
            assert!(
                edge.geometry
                    .length_error_estimate
                    .is_none_or(|error| error.is_finite() && error >= 0.0)
            );
            assert!(edge.geometry.polyline.len() >= 2);
        }

        let vertex_references = vertex_references(part);
        assert_eq!(vertex_references.len(), part.vertices.len());
        for vertex in &part.vertices {
            assert!(part.vertex(&vertex.reference).is_some());
            assert!(
                vertex
                    .reference
                    .incident_edges
                    .iter()
                    .all(|reference| edge_references.contains(reference))
            );
            assert!(
                vertex
                    .geometry
                    .position
                    .iter()
                    .all(|value| value.is_finite())
            );
        }
    }

    #[test]
    fn truck_evaluates_supported_primitives() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "box".into(),
                    size: [10.0, 20.0, 30.0],
                    position: [0.0; 3],
                },
                ModelCommand::CreateCylinder {
                    name: "cylinder".into(),
                    radius: 5.0,
                    height: 12.0,
                    position: [20.0, 0.0, 0.0],
                },
                ModelCommand::CreateSphere {
                    name: "sphere".into(),
                    radius: 6.0,
                    position: [40.0, 0.0, 6.0],
                },
                ModelCommand::CreateCone {
                    name: "frustum".into(),
                    bottom_radius: 7.0,
                    top_radius: 3.0,
                    height: 14.0,
                    position: [60.0, 0.0, 0.0],
                },
                ModelCommand::CreateTorus {
                    name: "seal".into(),
                    major_radius: 12.0,
                    minor_radius: 3.0,
                    position: [80.0, 0.0, 4.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::SetMaterial {
                id: ids[0],
                name: "Steel".into(),
                density_kg_m3: 7_850.0,
            })
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 5);
        assert!(scene.triangle_count() >= 100);
        assert!(scene.parts.iter().all(|part| {
            part.mesh.positions.len() == part.mesh.normals.len()
                && part.mesh.indices.len() == part.mesh.positions.len()
        }));
        for part in &scene.parts {
            assert_topology_partition(part);
        }
        assert_eq!(scene.parts[0].material.as_ref().unwrap().name, "Steel");
        assert!(scene.parts[1..].iter().all(|part| part.material.is_none()));
        let cylinder_curves = scene.parts[1]
            .edges
            .iter()
            .filter(|edge| edge.geometry.curve != CurveKind::Line)
            .collect::<Vec<_>>();
        assert!(!cylinder_curves.is_empty());
        let total_circular_length = cylinder_curves
            .iter()
            .map(|edge| edge.geometry.length)
            .sum::<f64>();
        assert!((total_circular_length - 2.0 * TAU * 5.0).abs() < 1.0e-7);
        assert!(cylinder_curves.iter().all(|edge| {
            edge.geometry
                .length_error_estimate
                .is_some_and(|error| error <= 1.0e-8)
        }));
        let rebuilt = TruckKernel::default().evaluate(&document).unwrap();
        for (before, after) in scene.parts.iter().zip(&rebuilt.parts) {
            assert_eq!(face_references(before), face_references(after));
            assert_eq!(edge_references(before), edge_references(after));
            assert_eq!(vertex_references(before), vertex_references(after));
        }
    }

    #[test]
    fn truck_declares_its_edge_modifier_contract() {
        let capabilities = TruckKernel::default().capabilities();
        assert_eq!(capabilities.chamfer.edge_count, EdgeCountSupport::Multiple);
        assert_eq!(
            capabilities.chamfer.source_scope,
            SourceFeatureScope::Single
        );
        assert_eq!(
            capabilities.chamfer.edge_curves,
            EdgeCurveSupport::LinearOnly
        );
        assert_eq!(
            capabilities.chamfer.support_surfaces,
            SupportSurfaceSupport::PlanarOnly
        );
        assert_eq!(
            capabilities.chamfer.edge_convexity,
            EdgeConvexitySupport::ConvexOnly
        );
        assert_eq!(
            capabilities.chamfer.shared_vertex_support,
            SharedVertexSupport::ConvexPolyhedralSource
        );
        assert_eq!(
            capabilities.fillet.shared_vertex_support,
            SharedVertexSupport::Unsupported
        );
    }

    #[test]
    fn truck_evaluates_a_pointed_cone() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateCone {
                name: "pointed cone".into(),
                bottom_radius: 8.0,
                top_radius: 0.0,
                height: 20.0,
                position: [0.0; 3],
            })
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(scene.parts[0].mesh.triangle_count() >= 16);
        assert_topology_partition(&scene.parts[0]);
    }

    #[test]
    fn truck_applies_rotation_around_feature_origin() {
        let mut document = CadDocument::default();
        let id = document
            .apply(ModelCommand::CreateBox {
                name: "rotated box".into(),
                size: [10.0, 5.0, 2.0],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::Rotate {
                id,
                rotation: [0.0, 0.0, 90.0],
            })
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let positions = &scene.parts[0].mesh.positions;
        let min_x = positions
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let max_y = positions
            .iter()
            .map(|point| point[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((min_x + 5.0).abs() < 1.0e-4);
        assert!((max_y - 10.0).abs() < 1.0e-4);
    }

    #[test]
    fn primitive_face_references_survive_rebuild_resize_and_transform() {
        let mut document = CadDocument::default();
        let id = document
            .apply(ModelCommand::CreateBox {
                name: "stable box".into(),
                size: [10.0, 5.0, 2.0],
                position: [1.0, 2.0, 3.0],
            })
            .unwrap()
            .unwrap();
        let kernel = TruckKernel::default();
        let before = kernel.evaluate(&document).unwrap();
        let before_refs = face_references(&before.parts[0]);
        let before_edges = edge_references(&before.parts[0]);
        let before_vertices = vertex_references(&before.parts[0]);
        assert_topology_partition(&before.parts[0]);
        assert_eq!(before_refs.len(), 6);
        assert_eq!(before_edges.len(), 12);
        assert_eq!(before_vertices.len(), 8);
        for face in [
            PrimitiveFace::BoxXMin,
            PrimitiveFace::BoxXMax,
            PrimitiveFace::BoxYMin,
            PrimitiveFace::BoxYMax,
            PrimitiveFace::BoxZMin,
            PrimitiveFace::BoxZMax,
        ] {
            let reference = FaceRef::primitive(id, face);
            assert!(before.face(&reference).is_some());
        }

        document
            .apply_transaction([
                ModelCommand::ResizeBox {
                    id,
                    size: [24.0, 7.0, 4.0],
                },
                ModelCommand::Move {
                    id,
                    position: [-3.0, 8.0, 1.0],
                },
                ModelCommand::Rotate {
                    id,
                    rotation: [17.0, 31.0, 73.0],
                },
            ])
            .unwrap();
        let after = kernel.evaluate(&document).unwrap();
        assert_topology_partition(&after.parts[0]);
        assert_eq!(face_references(&after.parts[0]), before_refs);
        assert_eq!(edge_references(&after.parts[0]), before_edges);
        assert_eq!(vertex_references(&after.parts[0]), before_vertices);
    }

    #[test]
    fn datum_plane_resolves_a_persistent_face_without_creating_a_solid() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "fixture".into(),
                size: [24.0, 16.0, 8.0],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let reference = FaceRef::primitive(source, PrimitiveFace::BoxZMax);
        let datum = document
            .apply(ModelCommand::CreateDatumPlane {
                name: "top machining datum".into(),
                face: reference,
                offset: 1.5,
            })
            .unwrap()
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, source);
        assert!(scene.parts.iter().all(|part| part.feature_id != datum));
        assert_eq!(scene.datum_planes.len(), 1);
        assert_eq!(scene.datum_planes[0].feature_id, datum);
        assert_eq!(scene.datum_planes[0].name, "top machining datum");
        assert!((scene.datum_planes[0].origin[2] - 9.5).abs() < 1.0e-8);
        assert!(scene.datum_planes[0].normal[2] > 0.999);

        document
            .apply(ModelCommand::ResizeBox {
                id: source,
                size: [32.0, 18.0, 10.0],
            })
            .unwrap();
        TruckKernel::default().evaluate(&document).unwrap();
    }

    #[test]
    fn datum_plane_drives_a_transformed_sketch_extrusion_and_step_export() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "fixture".into(),
                size: [6.0, 8.0, 10.0],
                position: [10.0, 20.0, 30.0],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::Rotate {
                id: source,
                rotation: [17.0, 31.0, 73.0],
            })
            .unwrap();
        let datum = document
            .apply(ModelCommand::CreateDatumPlane {
                name: "side datum".into(),
                face: FaceRef::primitive(source, PrimitiveFace::BoxXMax),
                offset: 2.0,
            })
            .unwrap()
            .unwrap();
        let kernel = TruckKernel::default();
        let initial_scene = kernel.evaluate(&document).unwrap();
        let datum_plane = &initial_scene.datum_planes[0];
        let datum_origin = datum_plane.origin;
        let datum_x = datum_plane.x_direction;
        let datum_y = datum_plane.y_direction;
        let datum_normal = datum_plane.normal;
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                name: "side profile".into(),
                plane: SketchPlane::DatumPlane { datum_id: datum },
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 5.0], [0.0, 5.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [1.0, 2.0, 3.0],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::Rotate {
                id: sketch,
                rotation: [0.0, 0.0, 90.0],
            })
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "side pad".into(),
                sketch_id: sketch,
                height: 7.0,
                position: [2.0, -1.0, 3.0],
            })
            .unwrap()
            .unwrap();
        for id in [source, datum] {
            document
                .apply(ModelCommand::SetVisibility { id, visible: false })
                .unwrap();
        }

        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.sketches.len(), 1);
        assert_eq!(scene.sketches[0].feature_id, sketch);
        assert_eq!(scene.parts[0].feature_id, pad);
        assert_topology_partition(&scene.parts[0]);
        let profile_origin: [f64; 3] = std::array::from_fn(|axis| {
            datum_x[axis].mul_add(
                1.0,
                datum_y[axis].mul_add(2.0, datum_normal[axis].mul_add(3.0, datum_origin[axis])),
            )
        });
        let sketch_origin: [f64; 3] =
            std::array::from_fn(|axis| profile_origin[axis] + [2.0, -1.0, 3.0][axis]);
        let rotated_x = datum_y;
        let rotated_y = datum_x.map(|component| -component);
        for (actual, point) in scene.sketches[0].profile.iter().zip([
            [0.0_f64, 0.0],
            [4.0, 0.0],
            [4.0, 5.0],
            [0.0, 5.0],
        ]) {
            let expected: [f64; 3] = std::array::from_fn(|axis| {
                point[0].mul_add(
                    rotated_x[axis],
                    point[1].mul_add(rotated_y[axis], profile_origin[axis]),
                )
            });
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-8)
            );
        }
        let mut expected_min = [f64::INFINITY; 3];
        let mut expected_max = [f64::NEG_INFINITY; 3];
        for point in [[0.0_f64, 0.0], [4.0, 0.0], [4.0, 5.0], [0.0, 5.0]] {
            for height in [0.0_f64, 7.0] {
                let position: [f64; 3] = std::array::from_fn(|axis| {
                    point[0].mul_add(
                        rotated_x[axis],
                        point[1].mul_add(
                            rotated_y[axis],
                            height.mul_add(datum_normal[axis], sketch_origin[axis]),
                        ),
                    )
                });
                for axis in 0..3 {
                    expected_min[axis] = expected_min[axis].min(position[axis]);
                    expected_max[axis] = expected_max[axis].max(position[axis]);
                }
            }
        }
        let (min, max) = mesh_bounds(&scene.parts[0].mesh);
        for axis in 0..3 {
            assert!((f64::from(min[axis]) - expected_min[axis]).abs() < 1.0e-4);
            assert!((f64::from(max[axis]) - expected_max[axis]).abs() < 1.0e-4);
        }
        let names = face_references(&scene.parts[0]);
        assert!(names.contains(&FaceRef::primitive(pad, PrimitiveFace::StartCap)));
        assert!(names.contains(&FaceRef::primitive(pad, PrimitiveFace::EndCap)));
        for segment in 0..4 {
            assert!(names.contains(&FaceRef::primitive(
                pad,
                PrimitiveFace::ProfileSide { segment }
            )));
        }

        document
            .apply(ModelCommand::SetDatumPlaneOffset {
                id: datum,
                offset: 4.0,
            })
            .unwrap();
        document
            .apply(ModelCommand::SetVisibility {
                id: sketch,
                visible: false,
            })
            .unwrap();
        let rebuilt = kernel.evaluate(&document).unwrap();
        assert!(rebuilt.sketches.is_empty());
        assert_eq!(face_references(&rebuilt.parts[0]), names);
        let (rebuilt_min, rebuilt_max) = mesh_bounds(&rebuilt.parts[0].mesh);
        for axis in 0..3 {
            let shift = datum_normal[axis] * 2.0;
            assert!((f64::from(rebuilt_min[axis] - min[axis]) - shift).abs() < 1.0e-4);
            assert!((f64::from(rebuilt_max[axis] - max[axis]) - shift).abs() < 1.0e-4);
        }

        let step = kernel.encode_step(&document, "datum-pad.step").unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn planar_face_drives_a_transformed_sketch_and_extrusion_directly() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "rotated housing".into(),
                size: [6.0, 8.0, 10.0],
                position: [10.0, 20.0, 30.0],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::Rotate {
                id: source,
                rotation: [17.0, 31.0, 73.0],
            })
            .unwrap();
        let face = FaceRef::primitive(source, PrimitiveFace::BoxXMax);
        let kernel = TruckKernel::default();
        let source_scene = kernel.evaluate(&document).unwrap();
        let support = source_scene.face(&face).unwrap();
        let plane = support.geometry.plane.unwrap();
        let normal = normalized(support.geometry.mean_normal).unwrap();
        let distance = plane
            .normal
            .iter()
            .enumerate()
            .fold(0.0, |sum, (axis, component)| {
                (support.geometry.centroid[axis] - plane.origin[axis]).mul_add(*component, sum)
            });
        let expected_origin: [f64; 3] = std::array::from_fn(|axis| {
            (-distance).mul_add(plane.normal[axis], support.geometry.centroid[axis])
        });
        let alignment = plane
            .normal
            .iter()
            .zip(normal)
            .fold(0.0, |sum, (left, right)| left.mul_add(right, sum));
        let expected_y = if alignment.is_sign_positive() {
            plane.y_direction
        } else {
            plane.y_direction.map(|component| -component)
        };
        let profile = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 5.0], [0.0, 5.0]];
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                name: "direct side profile".into(),
                plane: SketchPlane::PlanarFace { face: face.clone() },
                profile: profile.clone(),
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "direct side pad".into(),
                sketch_id: sketch,
                height: 7.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::SetVisibility {
                id: source,
                visible: false,
            })
            .unwrap();

        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, pad);
        assert_eq!(scene.sketches.len(), 1);
        let overlay = &scene.sketches[0];
        assert_eq!(overlay.feature_id, sketch);
        for (actual, expected) in [
            (&overlay.origin, expected_origin),
            (&overlay.x_direction, plane.x_direction),
            (&overlay.y_direction, expected_y),
            (&overlay.normal, normal),
        ] {
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-8)
            );
        }
        for (actual, point) in overlay.profile.iter().zip(profile) {
            let expected: [f64; 3] = std::array::from_fn(|axis| {
                point[0].mul_add(
                    plane.x_direction[axis],
                    point[1].mul_add(expected_y[axis], expected_origin[axis]),
                )
            });
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-8)
            );
        }

        assert_topology_partition(&scene.parts[0]);
        let names = face_references(&scene.parts[0]);
        assert!(names.contains(&FaceRef::primitive(pad, PrimitiveFace::StartCap)));
        assert!(names.contains(&FaceRef::primitive(pad, PrimitiveFace::EndCap)));
        for segment in 0..4 {
            assert!(names.contains(&FaceRef::primitive(
                pad,
                PrimitiveFace::ProfileSide { segment }
            )));
        }
        let start = scene
            .face(&FaceRef::primitive(pad, PrimitiveFace::StartCap))
            .unwrap();
        let end = scene
            .face(&FaceRef::primitive(pad, PrimitiveFace::EndCap))
            .unwrap();
        let cap_offset: [f64; 3] =
            std::array::from_fn(|axis| end.geometry.centroid[axis] - start.geometry.centroid[axis]);
        assert!(
            cap_offset
                .iter()
                .zip(normal)
                .all(|(actual, component)| (*actual - component * 7.0).abs() < 1.0e-4)
        );
    }

    #[test]
    fn planar_face_sketch_rejects_a_spherical_support() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateSphere {
                name: "spherical support".into(),
                radius: 5.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                name: "invalid profile".into(),
                plane: SketchPlane::PlanarFace {
                    face: FaceRef::primitive(source, PrimitiveFace::Patch { index: 0 }),
                },
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::TopologyNaming { feature_id, .. }) if feature_id == sketch
        ));
    }

    #[test]
    fn datum_point_resolves_persistent_vertex_and_model_space_offset() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "fixture".into(),
                size: [24.0, 16.0, 8.0],
                position: [2.0, 3.0, 4.0],
            })
            .unwrap()
            .unwrap();
        let kernel = TruckKernel::default();
        let before = kernel.evaluate(&document).unwrap();
        let source_vertex = before.parts[0].vertices[0].clone();
        let datum = document
            .apply(ModelCommand::CreateDatumPoint {
                name: "setup origin".into(),
                vertex: source_vertex.reference.clone(),
                offset: [1.0, -2.0, 3.0],
            })
            .unwrap()
            .unwrap();

        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.datum_points.len(), 1);
        let point = &scene.datum_points[0];
        assert_eq!(point.feature_id, datum);
        assert_eq!(point.name, "setup origin");
        assert_eq!(point.vertex, source_vertex.reference);
        let expected_position: [f64; 3] = std::array::from_fn(|axis| {
            source_vertex.geometry.position[axis] + [1.0, -2.0, 3.0][axis]
        });
        assert!(
            point
                .position
                .iter()
                .zip(expected_position)
                .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-8)
        );

        document
            .apply(ModelCommand::ResizeBox {
                id: source,
                size: [32.0, 18.0, 10.0],
            })
            .unwrap();
        let rebuilt_position = kernel.evaluate(&document).unwrap().datum_points[0].position;
        document
            .apply(ModelCommand::SetVisibility {
                id: source,
                visible: false,
            })
            .unwrap();
        let hidden_source = kernel.evaluate(&document).unwrap();
        assert!(hidden_source.parts.is_empty());
        assert!(
            hidden_source.datum_points[0]
                .position
                .iter()
                .zip(rebuilt_position)
                .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-8)
        );
    }

    #[test]
    fn datum_point_fails_closed_when_the_vertex_name_is_lost() {
        let mut document = CadDocument::default();
        let _source = document
            .apply(ModelCommand::CreateBox {
                name: "fixture".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let kernel = TruckKernel::default();
        let mut reference = kernel.evaluate(&document).unwrap().parts[0].vertices[0]
            .reference
            .clone();
        reference.fragment = u32::MAX;
        document
            .apply(ModelCommand::CreateDatumPoint {
                name: "invalid datum".into(),
                vertex: reference,
                offset: [0.0; 3],
            })
            .unwrap();

        assert!(matches!(
            kernel.evaluate(&document),
            Err(KernelError::TopologyNaming { feature_id: 2, .. })
        ));
    }

    #[test]
    fn datum_plane_fails_closed_when_the_face_name_is_lost() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "fixture".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateDatumPlane {
                name: "invalid datum".into(),
                face: FaceRef::primitive(source, PrimitiveFace::Patch { index: 99 }),
                offset: 0.0,
            })
            .unwrap();

        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::TopologyNaming { feature_id: 2, .. })
        ));
    }

    #[test]
    fn datum_plane_rejects_a_non_planar_source_face() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateCylinder {
                name: "fixture".into(),
                radius: 5.0,
                height: 10.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let kernel = TruckKernel::default();
        let reference = kernel.evaluate(&document).unwrap().parts[0]
            .faces
            .iter()
            .find(|face| face.geometry.plane.is_none())
            .unwrap()
            .reference
            .clone();
        document
            .apply(ModelCommand::CreateDatumPlane {
                name: "invalid datum".into(),
                face: reference,
                offset: 0.0,
            })
            .unwrap();

        assert_eq!(source, 1);
        assert!(matches!(
            kernel.evaluate(&document),
            Err(KernelError::TopologyNaming { feature_id: 2, .. })
        ));
    }

    #[test]
    fn truck_evaluates_torus_with_expected_bounds() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateTorus {
                name: "ring".into(),
                major_radius: 10.0,
                minor_radius: 2.0,
                position: [3.0, 4.0, 5.0],
            })
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let positions = &scene.parts[0].mesh.positions;
        let min_x = positions
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = positions
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((min_x - (-9.0)).abs() < 0.2);
        assert!((max_x - 15.0).abs() < 0.2);
        assert!(scene.parts[0].mesh.triangle_count() >= 32);
    }

    #[test]
    fn truck_evaluates_extrusion_profile() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateExtrusion {
                name: "plate".into(),
                profile: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]],
                height: 8.0,
                position: [3.0, 4.0, 5.0],
            })
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(scene.parts[0].mesh.triangle_count() >= 12);
        let max_z = scene.parts[0]
            .mesh
            .positions
            .iter()
            .map(|point| point[2])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((max_z - 13.0).abs() < 1.0e-4);
    }

    #[test]
    fn linked_extrusion_rebuilds_from_latest_sketch() {
        let mut document = CadDocument::default();
        let sketch_id = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "outline".into(),
                profile: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "pad".into(),
                sketch_id,
                height: 5.0,
                position: [0.0; 3],
            })
            .unwrap();
        let before = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(before.parts.len(), 1);
        let before_refs = face_references(&before.parts[0]);
        assert_topology_partition(&before.parts[0]);
        let before_max_x = before.parts[0]
            .mesh
            .positions
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        document
            .apply(ModelCommand::ResizeSketch {
                id: sketch_id,
                profile: vec![[0.0, 0.0], [24.0, 0.0], [24.0, 8.0], [0.0, 8.0]],
            })
            .unwrap();
        let after = TruckKernel::default().evaluate(&document).unwrap();
        assert_topology_partition(&after.parts[0]);
        assert_eq!(face_references(&after.parts[0]), before_refs);
        let after_max_x = after.parts[0]
            .mesh
            .positions
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((before_max_x - 10.0).abs() < 1.0e-4);
        assert!((after_max_x - 24.0).abs() < 1.0e-4);
    }

    #[test]
    fn truck_extrudes_sketch_holes_as_inner_brep_walls() {
        let mut document = CadDocument::default();
        let holes = vec![
            vec![[6.0, 5.0], [14.0, 5.0], [14.0, 11.0], [6.0, 11.0]],
            vec![[2.0, 2.0], [2.0, 5.0], [4.0, 5.0], [4.0, 2.0]],
        ];
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::WorldXy,
                name: "mounting plate profile".into(),
                profile: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 16.0], [0.0, 16.0]],
                holes: holes.clone(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "mounting plate".into(),
                sketch_id: sketch,
                height: 5.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, pad);
        assert_topology_partition(&scene.parts[0]);
        assert_eq!(scene.parts[0].faces.len(), 14);
        let names = face_references(&scene.parts[0]);
        assert!(names.contains(&FaceRef::primitive(pad, PrimitiveFace::StartCap)));
        assert!(names.contains(&FaceRef::primitive(pad, PrimitiveFace::EndCap)));
        for segment in 0..4 {
            assert!(names.contains(&FaceRef::primitive(
                pad,
                PrimitiveFace::ProfileSide { segment }
            )));
            for hole in 0..2 {
                assert!(names.contains(&FaceRef::primitive(
                    pad,
                    PrimitiveFace::HoleSide { hole, segment }
                )));
            }
        }
        for cap in [PrimitiveFace::StartCap, PrimitiveFace::EndCap] {
            let geometry = &scene.face(&FaceRef::primitive(pad, cap)).unwrap().geometry;
            assert!(
                (geometry.area - 266.0).abs() < 1.0e-3,
                "unexpected cap area: {}",
                geometry.area
            );
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 1_330.0).abs() < 1.0e-4);
        let (min, max) = mesh_bounds(&scene.parts[0].mesh);
        assert!(
            min.into_iter()
                .zip([0.0, 0.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-4)
        );
        assert!(
            max.into_iter()
                .zip([20.0, 16.0, 5.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-4)
        );

        let stl = encode_binary_stl(&scene).unwrap();
        assert!(stl.len() > 84);
        let three_mf = encode_3mf(&scene).unwrap();
        validate_3mf(&three_mf).unwrap();

        let step = kernel
            .encode_step(&document, "mounting-plate.step")
            .unwrap();
        validate_step(&step).unwrap();

        document
            .apply(ModelCommand::SetSketchHoles {
                id: sketch,
                holes: holes
                    .into_iter()
                    .map(|hole| hole.into_iter().rev().collect())
                    .collect(),
            })
            .unwrap();
        let reversed = kernel.evaluate(&document).unwrap();
        assert_eq!(face_references(&reversed.parts[0]), names);
        assert_topology_partition(&reversed.parts[0]);
    }

    #[test]
    fn truck_extrudes_exact_arc_regions_with_curved_outer_and_hole_walls() {
        let mut document = CadDocument::default();
        let region = SketchRegion2D {
            profile: circle_loop([0.0, 0.0], 10.0, true),
            holes: vec![circle_loop([0.0, 0.0], 4.0, true)],
        };
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "exact annulus profile".into(),
                region,
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "exact annulus".into(),
                sketch_id: sketch,
                height: 5.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::new(0.005);
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        let part = &scene.parts[0];
        assert_topology_partition(part);
        assert_eq!(part.faces.len(), 6);
        let names = face_references(part);
        for segment in 0..2 {
            assert!(
                names.contains(&FaceRef::primitive(
                    pad,
                    PrimitiveFace::ProfileSide { segment }
                )),
                "missing outer segment {segment}: {names:?}"
            );
            assert!(
                names.contains(&FaceRef::primitive(
                    pad,
                    PrimitiveFace::HoleSide { hole: 0, segment }
                )),
                "missing hole segment {segment}: {names:?}"
            );
        }
        assert!(
            part.edges
                .iter()
                .filter(|edge| matches!(edge.geometry.curve, CurveKind::Nurbs))
                .count()
                >= 8
        );
        let expected_area = std::f64::consts::PI * (100.0 - 16.0);
        for cap in [PrimitiveFace::StartCap, PrimitiveFace::EndCap] {
            let area = scene
                .face(&FaceRef::primitive(pad, cap))
                .unwrap()
                .geometry
                .area;
            assert!(
                (area - expected_area).abs() < 0.2,
                "unexpected cap area {area}"
            );
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - expected_area * 5.0).abs() < 1.0);
        encode_binary_stl(&scene).unwrap();
        let three_mf = encode_3mf(&scene).unwrap();
        validate_3mf(&three_mf).unwrap();
        let step = kernel.encode_step(&document, "exact-annulus.step").unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn truck_extrudes_an_advanced_constrained_line_profile_and_exports_step() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::WorldXy,
                name: "advanced constrained rectangle".into(),
                profile: vec![[0.0, 0.0], [9.0, 1.0], [10.0, 6.0], [1.0, 5.0]],
                holes: Vec::new(),
                constraints: vec![
                    Constraint::Fixed {
                        point: 0,
                        x: 0.0,
                        y: 0.0,
                    },
                    Constraint::Length {
                        segment: 0,
                        length: 10.0,
                    },
                    Constraint::Length {
                        segment: 1,
                        length: 5.0,
                    },
                    Constraint::Parallel {
                        first: 0,
                        second: 2,
                    },
                    Constraint::EqualLength {
                        first: 0,
                        second: 2,
                    },
                    Constraint::Perpendicular {
                        first: 0,
                        second: 1,
                    },
                    Constraint::Parallel {
                        first: 1,
                        second: 3,
                    },
                    Constraint::EqualLength {
                        first: 1,
                        second: 3,
                    },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "advanced constrained pad".into(),
                sketch_id: sketch,
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::new(0.005);
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        let part = &scene.parts[0];
        assert_topology_partition(part);
        assert_eq!(part.faces.len(), 6);
        let names = face_references(part);
        for segment in 0..4 {
            assert!(
                names.contains(&FaceRef::primitive(
                    pad,
                    PrimitiveFace::ProfileSide { segment }
                )),
                "missing constrained segment {segment}: {names:?}"
            );
        }
        for cap in [PrimitiveFace::StartCap, PrimitiveFace::EndCap] {
            let area = scene
                .face(&FaceRef::primitive(pad, cap))
                .unwrap()
                .geometry
                .area;
            assert!((area - 50.0).abs() < 1.0e-5, "unexpected cap area {area}");
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 200.0).abs() < 1.0e-5);
        assert!(
            part.edges
                .iter()
                .all(|edge| edge.geometry.curve == CurveKind::Line)
        );
        let step = kernel
            .encode_step(&document, "advanced-constrained-pad.step")
            .unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn truck_solves_construction_relationships_without_exporting_reference_geometry() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "construction constrained rectangle".into(),
                region: SketchRegion2D {
                    profile: SketchLoop2D::from_polygon(vec![
                        [0.0, 0.0],
                        [10.0, 0.0],
                        [10.0, 8.0],
                        [0.0, 8.0],
                    ]),
                    holes: Vec::new(),
                },
                construction: vec![
                    SketchSegment2D::Line {
                        start: [0.0, -5.0],
                        end: [0.0, 5.0],
                    },
                    SketchSegment2D::Line {
                        start: [-3.0, 2.0],
                        end: [-5.0, 2.0],
                    },
                    SketchSegment2D::Line {
                        start: [3.0, 2.0],
                        end: [5.0, 2.0],
                    },
                    SketchSegment2D::Line {
                        start: [-5.0, 0.0],
                        end: [-5.0, 4.0],
                    },
                    SketchSegment2D::Line {
                        start: [5.0, 0.0],
                        end: [5.0, 4.0],
                    },
                ],
                constraints: vec![
                    Constraint::Symmetric {
                        first: 6,
                        second: 8,
                        axis: 4,
                    },
                    Constraint::Midpoint {
                        point: 7,
                        segment: 7,
                    },
                    Constraint::PointOnCurve {
                        point: 9,
                        segment: 8,
                    },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "construction constrained pad".into(),
                sketch_id: sketch,
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::new(0.005);
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.sketches.len(), 1);
        assert_eq!(scene.sketches[0].construction.len(), 5);
        assert_eq!(scene.parts.len(), 1);
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, pad);
        assert_topology_partition(part);
        assert_eq!(part.faces.len(), 6);
        assert_eq!(part.edges.len(), 12);
        for cap in [PrimitiveFace::StartCap, PrimitiveFace::EndCap] {
            let area = scene
                .face(&FaceRef::primitive(pad, cap))
                .unwrap()
                .geometry
                .area;
            assert!((area - 80.0).abs() < 1.0e-6);
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 320.0).abs() < 1.0e-5);
        let step = kernel
            .encode_step(&document, "construction-constrained-pad.step")
            .unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn truck_reports_point_dimension_dof_without_exporting_arc_construction() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "dimensioned construction references".into(),
                region: SketchRegion2D::from_polygons(
                    vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
                    Vec::new(),
                ),
                construction: vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [4.0, 0.0],
                    },
                    SketchSegment2D::Arc {
                        start: [8.0, 2.0],
                        end: [4.0, 2.0],
                        center: [6.0, 0.0],
                        ccw: true,
                    },
                ],
                constraints: vec![
                    Constraint::HorizontalDistance {
                        first: 4,
                        second: 5,
                        distance: 4.0,
                    },
                    Constraint::VerticalDistance {
                        first: 4,
                        second: 5,
                        distance: 0.0,
                    },
                    Constraint::PointLineDistance {
                        point: 6,
                        line: 4,
                        distance: 2.0,
                    },
                    Constraint::LineThroughCenter { line: 4, arc: 5 },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "dimensioned pad".into(),
                sketch_id: sketch,
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::new(0.005);
        let scene = kernel.evaluate(&document).unwrap();
        let diagnostic = scene.sketch_diagnostic(sketch).unwrap();
        assert_eq!(diagnostic.parameter_count, 18);
        assert_eq!(diagnostic.equation_count, 5);
        assert_eq!(diagnostic.rank, 5);
        assert_eq!(diagnostic.degrees_of_freedom, 13);
        assert!(diagnostic.redundant_constraints.is_empty());
        assert_eq!(scene.sketches[0].construction.len(), 2);
        assert_eq!(scene.sketches[0].constraint_annotations.len(), 4);
        assert!(
            scene.sketches[0].constraint_annotations[..3]
                .iter()
                .all(|annotation| annotation.constraint.dimension().is_some())
        );
        assert!(
            scene.sketches[0].constraint_annotations[3]
                .constraint
                .dimension()
                .is_none()
        );
        let part = scene
            .parts
            .iter()
            .find(|part| part.feature_id == pad)
            .unwrap();
        assert_eq!(part.faces.len(), 6);
        assert_eq!(part.edges.len(), 12);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 320.0).abs() < 1.0e-5);
        let step = kernel
            .encode_step(&document, "point-dimension-pad.step")
            .unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn truck_extrudes_a_constrained_exact_circle_and_exports_step() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "constrained exact circle".into(),
                region: SketchRegion2D {
                    profile: circle_loop([1.0, -2.0], 4.0, true),
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: vec![
                    Constraint::FixedCenter {
                        segment: 0,
                        x: 3.0,
                        y: 5.0,
                    },
                    Constraint::Radius {
                        segment: 0,
                        radius: 6.0,
                    },
                    Constraint::Concentric {
                        first: 0,
                        second: 1,
                    },
                    Constraint::EqualRadius {
                        first: 0,
                        second: 1,
                    },
                    Constraint::CurvatureContinuous {
                        first: 0,
                        second: 1,
                    },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "constrained exact cylinder".into(),
                sketch_id: sketch,
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::new(0.005);
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        let evaluated_sketch = scene
            .sketches
            .iter()
            .find(|item| item.feature_id == sketch)
            .unwrap();
        assert_eq!(evaluated_sketch.constraint_annotations.len(), 5);
        assert!(matches!(
            evaluated_sketch.constraint_annotations[4].constraint,
            Constraint::CurvatureContinuous {
                first: 0,
                second: 1
            }
        ));
        let part = &scene.parts[0];
        assert_topology_partition(part);
        assert_eq!(part.faces.len(), 4);
        let names = face_references(part);
        for segment in 0..2 {
            assert!(names.contains(&FaceRef::primitive(
                pad,
                PrimitiveFace::ProfileSide { segment }
            )));
        }
        assert!(
            part.edges
                .iter()
                .filter(|edge| matches!(edge.geometry.curve, CurveKind::Nurbs))
                .count()
                >= 4
        );
        let expected_area = std::f64::consts::PI * 36.0;
        for cap in [PrimitiveFace::StartCap, PrimitiveFace::EndCap] {
            let area = scene
                .face(&FaceRef::primitive(pad, cap))
                .unwrap()
                .geometry
                .area;
            assert!(
                (area - expected_area).abs() < 0.1,
                "unexpected constrained circle cap area {area}"
            );
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - expected_area * 4.0).abs() < 0.5);
        let (minimum, maximum) = mesh_bounds(&part.mesh);
        for (actual, expected) in minimum.into_iter().zip([-3.0, -1.0, 0.0]) {
            assert!((actual - expected).abs() < 0.01);
        }
        for (actual, expected) in maximum.into_iter().zip([9.0, 11.0, 4.0]) {
            assert!((actual - expected).abs() < 0.01);
        }
        let step = kernel
            .encode_step(&document, "constrained-exact-circle.step")
            .unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn truck_samples_exact_arc_overlays_through_circle_extrema() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "exact overlay circle".into(),
                region: SketchRegion2D {
                    profile: circle_loop([3.0, -2.0], 10.0, true),
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.sketches.len(), 1);
        let profile = &scene.sketches[0].profile;
        assert!(profile.len() > 8);
        for expected in [
            [13.0, -2.0, 0.0],
            [3.0, 8.0, 0.0],
            [-7.0, -2.0, 0.0],
            [3.0, -12.0, 0.0],
        ] {
            assert!(profile.iter().any(|actual| {
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-8)
            }));
        }
    }

    #[test]
    fn truck_evaluates_revolve_from_sketch() {
        let mut document = CadDocument::default();
        let sketch_id = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "turning profile".into(),
                profile: vec![[5.0, 0.0], [10.0, 0.0], [10.0, 12.0], [5.0, 12.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "turned body".into(),
                sketch_id,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
                position: [2.0, 3.0, 4.0],
            })
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(scene.parts[0].mesh.triangle_count() > 0);
        let min_x = scene.parts[0]
            .mesh
            .positions
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = scene.parts[0]
            .mesh
            .positions
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(min_x < -2.5 && max_x > 11.5);
    }

    #[test]
    fn truck_lofts_exact_curved_sections_with_stable_faces_and_step_export() {
        let mut document = CadDocument::default();
        let mut sections = Vec::new();
        for (index, (radius, z)) in [(5.0, 0.0), (3.0, 10.0), (6.0, 20.0)]
            .into_iter()
            .enumerate()
        {
            sections.push(
                document
                    .apply(ModelCommand::CreateSketchRegion {
                        name: format!("loft section {index}"),
                        plane: SketchPlane::WorldXy,
                        region: SketchRegion2D {
                            profile: circle_loop([0.0, 0.0], radius, true),
                            holes: Vec::new(),
                        },
                        construction: Vec::new(),
                        constraints: Vec::new(),
                        position: [0.0, 0.0, z],
                    })
                    .unwrap()
                    .unwrap(),
            );
        }
        let loft = document
            .apply(ModelCommand::CreateLoftFromSketches {
                name: "exact ruled loft".into(),
                sketch_ids: sections.clone(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        let part = &scene.parts[0];
        assert_topology_partition(part);
        assert_eq!(part.faces.len(), 6);
        let names = face_references(part);
        assert!(names.contains(&FaceRef::primitive(loft, PrimitiveFace::StartCap)));
        assert!(names.contains(&FaceRef::primitive(loft, PrimitiveFace::EndCap)));
        for transition in 0..2 {
            for segment in 0..2 {
                assert!(names.contains(&FaceRef::primitive(
                    loft,
                    PrimitiveFace::LoftSide {
                        transition,
                        segment,
                    },
                )));
            }
        }
        assert!(
            part.faces
                .iter()
                .any(|face| face.geometry.surface == SurfaceKind::Swept)
        );
        assert!(
            part.edges
                .iter()
                .any(|edge| edge.geometry.curve == CurveKind::Nurbs)
        );
        let (minimum, maximum) = mesh_bounds(&part.mesh);
        assert!((minimum[2] - 0.0).abs() < 0.01);
        assert!((maximum[2] - 20.0).abs() < 0.01);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!(analysis.total_volume_mm3 > 1_000.0);

        document
            .apply(ModelCommand::SetSketchRegion {
                id: sections[1],
                region: SketchRegion2D {
                    profile: circle_loop([1.0, 0.0], 4.0, true),
                    holes: Vec::new(),
                },
            })
            .unwrap();
        let rebuilt = kernel.evaluate(&document).unwrap();
        assert_eq!(face_references(&rebuilt.parts[0]), names);
        let step = kernel.encode_step(&document, "exact-loft.step").unwrap();
        validate_step(&step).unwrap();
        assert!(step.contains("RATIONAL_B_SPLINE_SURFACE"));
        assert_eq!(step.matches("CLOSED_SHELL").count(), 1);
    }

    #[test]
    fn truck_loft_rejects_folded_section_order() {
        let mut document = CadDocument::default();
        let mut sections = Vec::new();
        for (index, z) in [0.0, 20.0, 10.0].into_iter().enumerate() {
            sections.push(
                document
                    .apply(ModelCommand::CreateSketch {
                        name: format!("folded section {index}"),
                        plane: SketchPlane::WorldXy,
                        profile: vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]],
                        holes: Vec::new(),
                        constraints: Vec::new(),
                        position: [0.0, 0.0, z],
                    })
                    .unwrap()
                    .unwrap(),
            );
        }
        document
            .apply(ModelCommand::CreateLoftFromSketches {
                name: "folded loft".into(),
                sketch_ids: sections,
                position: [0.0; 3],
            })
            .unwrap();
        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::Evaluation { message, .. })
                if message.contains("does not advance monotonically")
        ));
    }

    #[test]
    fn truck_revolves_an_exact_arc_profile_and_exports_step() {
        let mut document = CadDocument::default();
        let profile = SketchLoop2D {
            segments: vec![
                SketchSegment2D::Arc {
                    start: [6.0, -4.0],
                    end: [6.0, 4.0],
                    center: [6.0, 0.0],
                    ccw: true,
                },
                SketchSegment2D::Line {
                    start: [6.0, 4.0],
                    end: [6.0, -4.0],
                },
            ],
        };
        let sketch_id = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "exact turning profile".into(),
                region: SketchRegion2D {
                    profile,
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "exact revolved body".into(),
                sketch_id,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 270.0,
                position: [0.0; 3],
            })
            .unwrap();

        let kernel = TruckKernel::new(0.01);
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(scene.parts[0].mesh.triangle_count() > 0);
        assert_topology_partition(&scene.parts[0]);
        assert!(
            scene.parts[0]
                .edges
                .iter()
                .any(|edge| matches!(edge.geometry.curve, CurveKind::Nurbs))
        );
        let step = kernel
            .encode_step(&document, "exact-arc-revolve.step")
            .unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn removed_profile_segment_invalidates_its_face_reference() {
        let mut document = CadDocument::default();
        let id = document
            .apply(ModelCommand::CreateExtrusion {
                name: "changing profile".into(),
                profile: vec![[0.0, 0.0], [12.0, 0.0], [14.0, 5.0], [8.0, 9.0], [0.0, 7.0]],
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let removed = FaceRef::primitive(id, PrimitiveFace::ProfileSide { segment: 4 });
        let kernel = TruckKernel::default();
        let before = kernel.evaluate(&document).unwrap();
        assert!(before.face(&removed).is_some());

        document
            .apply(ModelCommand::ResizeExtrusion {
                id,
                profile: vec![[0.0, 0.0], [12.0, 0.0], [12.0, 7.0], [0.0, 7.0]],
                height: 4.0,
            })
            .unwrap();
        let after = kernel.evaluate(&document).unwrap();
        assert!(after.face(&removed).is_none());
    }

    #[test]
    fn boolean_feature_rebuilds_from_hidden_upstream_solids() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "base".into(),
                    size: [10.0, 10.0, 10.0],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "extension".into(),
                    size: [10.0, 10.0, 10.0],
                    position: [5.0, 2.0, 1.0],
                },
            ])
            .unwrap();
        let result = document
            .apply(ModelCommand::CreateBoolean {
                name: "joined".into(),
                operation: BooleanOperation::Union,
                left: ids[0],
                right: ids[1],
            })
            .unwrap()
            .unwrap();
        assert!(!document.feature(ids[0]).unwrap().visible);
        assert!(!document.feature(ids[1]).unwrap().visible);

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, result);
        let positions = &scene.parts[0].mesh.positions;
        let min_x = positions
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = positions
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((min_x - 0.0).abs() < 1.0e-4);
        assert!((max_x - 15.0).abs() < 1.0e-4);

        let before_refs = face_references(&scene.parts[0]);
        let before_edges = edge_references(&scene.parts[0]);
        let before_vertices = vertex_references(&scene.parts[0]);
        assert!(scene.parts[0].faces.iter().all(|face| {
            matches!(
                &face.reference.name,
                FaceName::Derived { sources, .. }
                    if !sources.is_empty()
                        && sources
                            .iter()
                            .all(|source| ids.contains(&source.feature_id))
            )
        }));
        assert_topology_partition(&scene.parts[0]);
        let rebuilt = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(face_references(&rebuilt.parts[0]), before_refs);
        assert_eq!(edge_references(&rebuilt.parts[0]), before_edges);
        assert_eq!(vertex_references(&rebuilt.parts[0]), before_vertices);

        document
            .apply(ModelCommand::ResizeBox {
                id: ids[1],
                size: [12.0, 10.0, 10.0],
            })
            .unwrap();
        let resized = TruckKernel::default().evaluate(&document).unwrap();
        assert_topology_partition(&resized.parts[0]);
        assert_eq!(face_references(&resized.parts[0]), before_refs);
        assert_eq!(edge_references(&resized.parts[0]), before_edges);
        assert_eq!(vertex_references(&resized.parts[0]), before_vertices);
    }

    #[test]
    fn disjoint_intersection_reports_structured_bounds_without_running_shapeops() {
        let mut document = CadDocument::default();
        let left = document
            .apply(ModelCommand::CreateBox {
                name: "left".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let right = document
            .apply(ModelCommand::CreateBox {
                name: "right".into(),
                size: [10.0; 3],
                position: [30.0, 0.0, 0.0],
            })
            .unwrap()
            .unwrap();
        let boolean = document
            .apply(ModelCommand::CreateBoolean {
                name: "empty intersection".into(),
                operation: BooleanOperation::Intersect,
                left,
                right,
            })
            .unwrap()
            .unwrap();

        let error = TruckKernel::default().evaluate(&document).unwrap_err();
        let KernelError::Boolean(diagnostic) = error else {
            panic!("expected structured boolean diagnostic");
        };
        assert_eq!(diagnostic.feature_id, boolean);
        assert_eq!(diagnostic.operands, [left, right]);
        assert_eq!(diagnostic.stage, BooleanFailureStage::BroadPhase);
        assert_eq!(diagnostic.reason, BooleanFailureReason::DisjointOperands);
        assert_eq!(diagnostic.operand_separation_mm(), Some([20.0, 0.0, 0.0]));
    }

    #[test]
    fn disjoint_union_returns_one_valid_two_shell_result() {
        let mut document = CadDocument::default();
        let operands = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "left".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "right".into(),
                    size: [10.0; 3],
                    position: [30.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        let result = document
            .apply(ModelCommand::CreateBoolean {
                name: "two bodies".into(),
                operation: BooleanOperation::Union,
                left: operands[0],
                right: operands[1],
            })
            .unwrap()
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, result);
        assert_eq!(scene.parts[0].faces.len(), 12);
        assert_eq!(scene.parts[0].edges.len(), 24);
        assert_eq!(scene.parts[0].vertices.len(), 16);
        assert_topology_partition(&scene.parts[0]);
        let bounds = mesh_bounds(&scene.parts[0].mesh);
        assert_eq!(bounds, ([0.0, 0.0, 0.0], [40.0, 10.0, 10.0]));

        let step = TruckKernel::default()
            .encode_step(&document, "disjoint-union.step")
            .unwrap();
        assert!(!step.contains("BREP_WITH_VOIDS"));
        assert_eq!(step.matches("MANIFOLD_SOLID_BREP").count(), 2);
        let imported = parse_step(step).unwrap();
        assert_eq!(imported.bodies.len(), 2);
        assert!(
            imported
                .bodies
                .iter()
                .all(|body| body.void_shells.is_empty())
        );
    }

    #[test]
    fn disjoint_subtraction_preserves_left_body_bounds_and_topology() {
        let mut document = CadDocument::default();
        let operands = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "left".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "right".into(),
                    size: [10.0; 3],
                    position: [30.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        let result = document
            .apply(ModelCommand::CreateBoolean {
                name: "unchanged left".into(),
                operation: BooleanOperation::Subtract,
                left: operands[0],
                right: operands[1],
            })
            .unwrap()
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, result);
        assert_eq!(scene.parts[0].faces.len(), 6);
        assert_eq!(scene.parts[0].edges.len(), 12);
        assert_eq!(scene.parts[0].vertices.len(), 8);
        assert_topology_partition(&scene.parts[0]);
        let bounds = mesh_bounds(&scene.parts[0].mesh);
        assert_eq!(bounds, ([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
    }

    #[test]
    fn chamfer_rebuilds_a_persistent_planar_box_edge() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let source_faces = [
            FaceRef::primitive(body, PrimitiveFace::BoxXMax),
            FaceRef::primitive(body, PrimitiveFace::BoxZMax),
        ];
        let edge = EdgeRef::new(body, source_faces[0].clone(), source_faces[1].clone(), 0);
        let chamfer = document
            .apply(ModelCommand::CreateChamfer {
                name: "edge break".into(),
                edges: vec![edge],
                distance: 2.0,
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, chamfer);
        assert_eq!(part.faces.len(), 7);
        assert_eq!(part.edges.len(), 15);
        assert_eq!(part.vertices.len(), 10);
        assert_topology_partition(part);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 980.0).abs() < 1.0e-4);
        assert!(part.faces.iter().any(|face| {
            matches!(
                &face.reference.name,
                FaceName::Derived { sources, .. } if sources == &source_faces
            )
        }));
        assert!(part.vertices.iter().all(|vertex| {
            let [x, _, z] = vertex.geometry.position;
            (x - 10.0).abs() > 1.0e-8 || (z - 10.0).abs() > 1.0e-8
        }));
        let before_faces = face_references(part);

        document
            .apply(ModelCommand::ResizeBox {
                id: body,
                size: [14.0, 10.0, 10.0],
            })
            .unwrap();
        let rebuilt = kernel.evaluate(&document).unwrap();
        assert_eq!(face_references(&rebuilt.parts[0]), before_faces);
        assert_eq!(
            mesh_bounds(&rebuilt.parts[0].mesh),
            ([0.0, 0.0, 0.0], [14.0, 10.0, 10.0])
        );
        assert!(rebuilt.parts[0].vertices.iter().all(|vertex| {
            let [x, _, z] = vertex.geometry.position;
            (x - 14.0).abs() > 1.0e-8 || (z - 10.0).abs() > 1.0e-8
        }));
    }

    #[test]
    fn chamfer_builds_explicit_two_edge_corner_miter() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let top = FaceRef::primitive(body, PrimitiveFace::BoxZMax);
        let selected_sources = [
            [
                FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                top.clone(),
            ],
            [FaceRef::primitive(body, PrimitiveFace::BoxYMax), top],
        ];
        let edges = selected_sources
            .iter()
            .map(|faces| EdgeRef::new(body, faces[0].clone(), faces[1].clone(), 0))
            .collect();
        let chamfer = document
            .apply(ModelCommand::CreateChamfer {
                name: "corner edge breaks".into(),
                edges,
                distance: 2.0,
            })
            .unwrap()
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, chamfer);
        assert_eq!(part.faces.len(), 8);
        assert_eq!(part.edges.len(), 17);
        assert_eq!(part.vertices.len(), 11);
        assert_topology_partition(part);
        for sources in selected_sources {
            assert!(part.faces.iter().any(|face| {
                matches!(&face.reference.name, FaceName::Derived { sources: actual, .. } if actual == &sources)
            }));
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        let expected_volume = 1_000.0 - (40.0 - 8.0 / 3.0);
        assert!(
            (analysis.total_volume_mm3 - expected_volume).abs() < 1.0e-4,
            "expected {expected_volume}, got {}",
            analysis.total_volume_mm3
        );
        let step = TruckKernel::default()
            .encode_step(&document, "corner-miter.step")
            .unwrap();
        validate_step(&step).unwrap();
    }

    #[test]
    fn chamfer_builds_explicit_three_edge_corner_miter() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let x_max = FaceRef::primitive(body, PrimitiveFace::BoxXMax);
        let y_max = FaceRef::primitive(body, PrimitiveFace::BoxYMax);
        let z_max = FaceRef::primitive(body, PrimitiveFace::BoxZMax);
        let selected_sources = [
            [x_max.clone(), z_max.clone()],
            [y_max.clone(), z_max],
            [x_max, y_max],
        ];
        let edges = selected_sources
            .iter()
            .map(|faces| EdgeRef::new(body, faces[0].clone(), faces[1].clone(), 0))
            .collect();
        let chamfer = document
            .apply(ModelCommand::CreateChamfer {
                name: "three-way corner miter".into(),
                edges,
                distance: 2.0,
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, chamfer);
        assert_eq!(part.faces.len(), 9);
        assert_eq!(part.edges.len(), 21);
        assert_eq!(part.vertices.len(), 14);
        assert_topology_partition(part);
        for sources in selected_sources {
            assert!(part.faces.iter().any(|face| {
                matches!(&face.reference.name, FaceName::Derived { sources: actual, .. } if actual == &sources)
            }));
        }
        assert!(part.vertices.iter().any(|vertex| {
            vertex
                .geometry
                .position
                .iter()
                .all(|coordinate| (*coordinate - 9.0).abs() < 1.0e-8)
        }));
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 946.0).abs() < 1.0e-4);
        let before_faces = face_references(part);

        document
            .apply(ModelCommand::ResizeBox {
                id: body,
                size: [14.0, 12.0, 10.0],
            })
            .unwrap();
        let rebuilt = kernel.evaluate(&document).unwrap();
        assert_eq!(face_references(&rebuilt.parts[0]), before_faces);
    }

    #[test]
    fn corner_miter_supports_a_general_convex_polyhedral_extrusion() {
        let mut document = CadDocument::default();
        let _body = document
            .apply(ModelCommand::CreateExtrusion {
                name: "triangular prism".into(),
                profile: vec![[0.0, 0.0], [12.0, 0.0], [3.0, 9.0]],
                height: 8.0,
                position: [2.0, 3.0, 4.0],
            })
            .unwrap()
            .unwrap();
        let initial = TruckKernel::default().evaluate(&document).unwrap();
        document
            .apply(ModelCommand::CreateChamfer {
                name: "prism corner miter".into(),
                edges: incident_edge_pair(&initial.parts[0])
                    .expect("a triangular prism has incident edge pairs")
                    .into(),
                distance: 0.75,
            })
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts[0].faces.len(), 7);
        assert_topology_partition(&scene.parts[0]);
    }

    #[test]
    fn corner_miter_fails_closed_for_a_non_convex_polyhedron() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateExtrusion {
                name: "concave prism".into(),
                profile: vec![
                    [0.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 4.0],
                    [4.0, 4.0],
                    [4.0, 10.0],
                    [0.0, 10.0],
                ],
                height: 8.0,
                position: [0.0; 3],
            })
            .unwrap();
        let initial = TruckKernel::default().evaluate(&document).unwrap();
        document
            .apply(ModelCommand::CreateChamfer {
                name: "unsupported concave miter".into(),
                edges: incident_edge_pair(&initial.parts[0])
                    .expect("a concave prism has incident edge pairs")
                    .into(),
                distance: 0.75,
            })
            .unwrap();

        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::EdgeModifier(diagnostic))
                if diagnostic.feature_id == 2
                    && diagnostic.stage == EdgeModifierFailureStage::GeometryValidation
                    && diagnostic.reason == EdgeModifierFailureReason::NonConvexSource
        ));
    }

    #[test]
    fn chamfer_builds_two_disjoint_edges_with_independent_lineage() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let selected_sources = [
            [
                FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                FaceRef::primitive(body, PrimitiveFace::BoxZMax),
            ],
            [
                FaceRef::primitive(body, PrimitiveFace::BoxXMin),
                FaceRef::primitive(body, PrimitiveFace::BoxZMin),
            ],
        ];
        let edges = selected_sources
            .iter()
            .map(|faces| EdgeRef::new(body, faces[0].clone(), faces[1].clone(), 0))
            .collect();
        let chamfer = document
            .apply(ModelCommand::CreateChamfer {
                name: "opposite edge breaks".into(),
                edges,
                distance: 2.0,
            })
            .unwrap()
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, chamfer);
        assert_topology_partition(part);
        for sources in selected_sources {
            assert!(part.faces.iter().any(|face| {
                matches!(&face.reference.name, FaceName::Derived { sources: actual, .. } if actual == &sources)
            }));
        }
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 960.0).abs() < 1.0e-4);
    }

    #[test]
    fn fillet_rebuilds_a_persistent_planar_box_edge() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let source_faces = [
            FaceRef::primitive(body, PrimitiveFace::BoxXMax),
            FaceRef::primitive(body, PrimitiveFace::BoxZMax),
        ];
        let edge = EdgeRef::new(body, source_faces[0].clone(), source_faces[1].clone(), 0);
        let fillet = document
            .apply(ModelCommand::CreateFillet {
                name: "round".into(),
                edges: vec![edge],
                radius: 2.0,
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, fillet);
        assert_eq!(part.faces.len(), 7);
        assert_eq!(part.edges.len(), 15);
        assert_eq!(part.vertices.len(), 10);
        assert_topology_partition(part);
        let expected_volume = 1_000.0 - 40.0 * (1.0 - std::f64::consts::FRAC_PI_4);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!(
            (analysis.total_volume_mm3 - expected_volume).abs() < 1.0,
            "expected {expected_volume}, got {}",
            analysis.total_volume_mm3
        );
        assert!(part.faces.iter().any(|face| {
            face.geometry.surface == SurfaceKind::Cylinder
                && matches!(
                    &face.reference.name,
                    FaceName::Derived { sources, .. } if sources == &source_faces
                )
        }));
        assert!(part.vertices.iter().all(|vertex| {
            let [x, _, z] = vertex.geometry.position;
            (x - 10.0).abs() > 1.0e-8 || (z - 10.0).abs() > 1.0e-8
        }));
        let before_faces = face_references(part);

        document
            .apply(ModelCommand::ResizeBox {
                id: body,
                size: [14.0, 10.0, 10.0],
            })
            .unwrap();
        let rebuilt = kernel.evaluate(&document).unwrap();
        assert_eq!(face_references(&rebuilt.parts[0]), before_faces);
        assert_eq!(
            mesh_bounds(&rebuilt.parts[0].mesh),
            ([0.0, 0.0, 0.0], [14.0, 10.0, 10.0])
        );
    }

    #[test]
    fn fillet_builds_two_disjoint_edges_with_independent_lineage() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let selected_sources = [
            [
                FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                FaceRef::primitive(body, PrimitiveFace::BoxZMax),
            ],
            [
                FaceRef::primitive(body, PrimitiveFace::BoxXMin),
                FaceRef::primitive(body, PrimitiveFace::BoxZMin),
            ],
        ];
        let edges = selected_sources
            .iter()
            .map(|faces| EdgeRef::new(body, faces[0].clone(), faces[1].clone(), 0))
            .collect();
        let fillet = document
            .apply(ModelCommand::CreateFillet {
                name: "opposite rounds".into(),
                edges,
                radius: 2.0,
            })
            .unwrap()
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let part = &scene.parts[0];
        assert_eq!(part.feature_id, fillet);
        assert_topology_partition(part);
        assert_eq!(
            part.faces
                .iter()
                .filter(|face| face.geometry.surface == SurfaceKind::Cylinder)
                .count(),
            2
        );
        for sources in selected_sources {
            assert!(part.faces.iter().any(|face| {
                face.geometry.surface == SurfaceKind::Cylinder
                    && matches!(&face.reference.name, FaceName::Derived { sources: actual, .. } if actual == &sources)
            }));
        }
        let expected_volume = 1_000.0 - 80.0 * (1.0 - std::f64::consts::FRAC_PI_4);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!(
            (analysis.total_volume_mm3 - expected_volume).abs() < 2.0,
            "expected {expected_volume}, got {}",
            analysis.total_volume_mm3
        );
    }

    #[test]
    fn fillet_rejects_shared_vertex_edges_before_shape_operations() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let top = FaceRef::primitive(body, PrimitiveFace::BoxZMax);
        let edges = vec![
            EdgeRef::new(
                body,
                FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                top.clone(),
                0,
            ),
            EdgeRef::new(
                body,
                FaceRef::primitive(body, PrimitiveFace::BoxYMax),
                top,
                0,
            ),
        ];
        document
            .apply(ModelCommand::CreateFillet {
                name: "unsupported corner round".into(),
                edges,
                radius: 2.0,
            })
            .unwrap();

        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::EdgeModifier(diagnostic))
                if diagnostic.feature_id == 2
                    && diagnostic.operation == EdgeModifierOperation::Fillet
                    && diagnostic.reason == EdgeModifierFailureReason::SharedVertexUnsupported
                    && diagnostic.offending_edge_indices == Some(vec![0, 1])
        ));
    }

    #[test]
    fn edge_modifier_reports_parameter_below_tolerance_without_building() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let edge = EdgeRef::new(
            body,
            FaceRef::primitive(body, PrimitiveFace::BoxXMax),
            FaceRef::primitive(body, PrimitiveFace::BoxZMax),
            0,
        );
        document
            .apply(ModelCommand::CreateChamfer {
                name: "sub-tolerance".into(),
                edges: vec![edge.clone()],
                distance: 0.01,
            })
            .unwrap();

        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::EdgeModifier(diagnostic))
                if diagnostic.operation == EdgeModifierOperation::Chamfer
                    && diagnostic.source_feature_id == Some(body)
                    && diagnostic.edges == vec![edge]
                    && diagnostic.stage == EdgeModifierFailureStage::GeometryValidation
                    && diagnostic.reason == EdgeModifierFailureReason::ParameterBelowTolerance
                    && diagnostic.parameter == EdgeModifierParameter::Distance
                    && (diagnostic.tolerance_mm - 0.05).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn edge_modifier_reports_a_lost_persistent_edge_reference() {
        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let edge = EdgeRef::new(
            body,
            FaceRef::primitive(body, PrimitiveFace::Patch { index: 98 }),
            FaceRef::primitive(body, PrimitiveFace::Patch { index: 99 }),
            0,
        );
        document
            .apply(ModelCommand::CreateFillet {
                name: "lost edge".into(),
                edges: vec![edge],
                radius: 1.0,
            })
            .unwrap();

        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::EdgeModifier(diagnostic))
                if diagnostic.stage == EdgeModifierFailureStage::ReferenceResolution
                    && diagnostic.reason == EdgeModifierFailureReason::LostReference
                    && diagnostic.offending_edge_indices == Some(vec![0])
        ));
    }

    #[test]
    fn chamfer_fails_closed_for_a_non_planar_adjacent_face() {
        let mut document = CadDocument::default();
        let _body = document
            .apply(ModelCommand::CreateCylinder {
                name: "body".into(),
                radius: 5.0,
                height: 10.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let edge = scene.parts[0]
            .edges
            .iter()
            .find(|edge| {
                edge.reference.adjacent_faces.iter().any(|face| {
                    scene
                        .face(face)
                        .is_some_and(|face| face.geometry.surface != SurfaceKind::Plane)
                })
            })
            .unwrap()
            .reference
            .clone();
        document
            .apply(ModelCommand::CreateChamfer {
                name: "unsupported".into(),
                edges: vec![edge],
                distance: 1.0,
            })
            .unwrap();

        let error = TruckKernel::default().evaluate(&document).unwrap_err();
        assert!(matches!(
            error,
            KernelError::EdgeModifier(diagnostic)
                if diagnostic.feature_id == 2
                    && matches!(
                        diagnostic.reason,
                        EdgeModifierFailureReason::NonLinearEdge
                            | EdgeModifierFailureReason::NonPlanarSupport
                    )
                    && diagnostic.offending_edge_indices == Some(vec![0])
        ));
    }

    #[test]
    fn fillet_fails_closed_for_unsupported_geometry_and_radius() {
        let mut curved = CadDocument::default();
        let _body = curved
            .apply(ModelCommand::CreateCylinder {
                name: "body".into(),
                radius: 5.0,
                height: 10.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let scene = TruckKernel::default().evaluate(&curved).unwrap();
        let edge = scene.parts[0]
            .edges
            .iter()
            .find(|edge| {
                edge.reference.adjacent_faces.iter().any(|face| {
                    scene
                        .face(face)
                        .is_some_and(|face| face.geometry.surface != SurfaceKind::Plane)
                })
            })
            .unwrap()
            .reference
            .clone();
        curved
            .apply(ModelCommand::CreateFillet {
                name: "unsupported".into(),
                edges: vec![edge],
                radius: 1.0,
            })
            .unwrap();
        let error = TruckKernel::default().evaluate(&curved).unwrap_err();
        assert!(matches!(
            error,
            KernelError::EdgeModifier(diagnostic)
                if diagnostic.feature_id == 2
                    && matches!(
                        diagnostic.reason,
                        EdgeModifierFailureReason::NonLinearEdge
                            | EdgeModifierFailureReason::NonPlanarSupport
                    )
        ));

        let mut oversized = CadDocument::default();
        let body = oversized
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        oversized
            .apply(ModelCommand::CreateFillet {
                name: "too large".into(),
                edges: vec![EdgeRef::new(
                    body,
                    FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                    0,
                )],
                radius: 11.0,
            })
            .unwrap();
        assert!(matches!(
            TruckKernel::default().evaluate(&oversized),
            Err(KernelError::EdgeModifier(diagnostic))
                if diagnostic.feature_id == 2
                    && diagnostic.parameter == EdgeModifierParameter::Radius
                    && (diagnostic.parameter_value_mm - 11.0).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn extended_wedge_prism_can_cut_a_box_transversely() {
        let mut document = CadDocument::default();
        let operands = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "body".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateExtrusion {
                    name: "wedge".into(),
                    profile: vec![[7.0, 11.0], [11.0, 7.0], [21.0, 17.0], [17.0, 21.0]],
                    height: 12.0,
                    position: [0.0, 0.0, -1.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::CreateBoolean {
                name: "cut".into(),
                operation: BooleanOperation::Subtract,
                left: operands[0],
                right: operands[1],
            })
            .unwrap();

        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts[0].faces.len(), 7);
        assert_topology_partition(&scene.parts[0]);
    }

    #[test]
    fn step_export_round_trips_visible_brep_parts_and_header() {
        use truck_stepio::r#in::Table;

        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "Bracket's base".into(),
                    size: [10.0, 20.0, 30.0],
                    position: [0.0; 3],
                },
                ModelCommand::CreateCylinder {
                    name: "\u{652f}\u{67b6}".into(),
                    radius: 4.0,
                    height: 12.0,
                    position: [20.0, 0.0, 0.0],
                },
                ModelCommand::CreateSphere {
                    name: "hidden reference".into(),
                    radius: 3.0,
                    position: [40.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::SetVisibility {
                id: ids[2],
                visible: false,
            })
            .unwrap();
        let source = TruckKernel::default()
            .encode_step(&document, "\u{88c5}\u{914d}'s.step")
            .unwrap();
        validate_step(&source).unwrap();
        let table = Table::from_step(&source).unwrap();
        assert_eq!(table.shell.len(), 2);
        for shell in table.shell.values() {
            let reimported = table.to_compressed_shell(shell).unwrap();
            assert!(
                !reimported
                    .triangulation(0.05)
                    .to_polygon()
                    .tri_faces()
                    .is_empty()
            );
        }
        assert!(source.contains("\\X2\\88C5914D\\X0\\_s.step"));
        assert!(source.contains("AUTOMOTIVE_DESIGN"));
    }

    #[test]
    fn step_export_round_trips_body_colors_and_transparency() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "blue housing".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateCylinder {
                    name: "orange pin".into(),
                    radius: 2.0,
                    height: 8.0,
                    position: [20.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        let expected = [[0.1, 0.2, 0.8, 0.75], [0.9, 0.35, 0.1, 1.0]];
        for (id, color) in ids.into_iter().zip(expected) {
            document
                .apply(ModelCommand::SetColor { id, color })
                .unwrap();
        }

        let source = TruckKernel::default()
            .encode_step(&document, "painted-assembly.step")
            .unwrap();
        assert_eq!(source.matches("STYLED_ITEM").count(), 2);
        assert!(source.contains("MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION"));
        let imported = parse_step(source).unwrap();
        assert_eq!(imported.bodies.len(), 2);
        for expected_color in expected {
            assert!(imported.bodies.iter().any(|body| {
                body.color.color().is_some_and(|actual| {
                    actual
                        .into_iter()
                        .zip(expected_color)
                        .all(|(actual, expected)| (actual - expected).abs() < 1.0e-6)
                })
            }));
        }
    }

    #[test]
    fn step_export_preserves_boolean_brep() {
        use truck_stepio::r#in::Table;

        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "base".into(),
                    size: [10.0, 10.0, 10.0],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "extension".into(),
                    size: [10.0, 10.0, 10.0],
                    position: [5.0, 2.0, 1.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::CreateBoolean {
                name: "joined block".into(),
                operation: BooleanOperation::Union,
                left: ids[0],
                right: ids[1],
            })
            .unwrap();

        let source = TruckKernel::default()
            .encode_step(&document, "boolean.step")
            .unwrap();
        let table = Table::from_step(&source).unwrap();
        assert_eq!(table.shell.len(), 1);
        let reimported = table
            .to_compressed_shell(table.shell.values().next().unwrap())
            .unwrap();
        assert!(
            !reimported
                .triangulation(0.05)
                .to_polygon()
                .tri_faces()
                .is_empty()
        );
    }

    #[test]
    fn step_export_preserves_chamfer_brep() {
        use truck_stepio::r#in::Table;

        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateChamfer {
                name: "edge break".into(),
                edges: vec![EdgeRef::new(
                    body,
                    FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                    0,
                )],
                distance: 2.0,
            })
            .unwrap();

        let source = TruckKernel::default()
            .encode_step(&document, "chamfer.step")
            .unwrap();
        validate_step(&source).unwrap();
        let table = Table::from_step(&source).unwrap();
        assert_eq!(table.shell.len(), 1);
        let reimported = table
            .to_compressed_shell(table.shell.values().next().unwrap())
            .unwrap();
        assert_eq!(reimported.faces.len(), 7);
        assert!(
            !reimported
                .triangulation(0.05)
                .to_polygon()
                .tri_faces()
                .is_empty()
        );
    }

    #[test]
    fn step_export_preserves_fillet_brep() {
        use truck_stepio::r#in::Table;

        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateFillet {
                name: "round".into(),
                edges: vec![EdgeRef::new(
                    body,
                    FaceRef::primitive(body, PrimitiveFace::BoxXMax),
                    FaceRef::primitive(body, PrimitiveFace::BoxZMax),
                    0,
                )],
                radius: 2.0,
            })
            .unwrap();

        let source = TruckKernel::default()
            .encode_step(&document, "fillet.step")
            .unwrap();
        validate_step(&source).unwrap();
        let table = Table::from_step(&source).unwrap();
        assert_eq!(table.shell.len(), 1);
        let reimported = table
            .to_compressed_shell(table.shell.values().next().unwrap())
            .unwrap();
        assert_eq!(reimported.faces.len(), 7);
        assert!(
            !reimported
                .triangulation(0.05)
                .to_polygon()
                .tri_faces()
                .is_empty()
        );
    }

    #[test]
    fn step_export_rejects_documents_without_visible_solids() {
        assert!(matches!(
            TruckKernel::default().encode_step(&CadDocument::default(), "empty.step"),
            Err(KernelError::Exchange { format: "STEP", .. })
        ));
    }

    #[test]
    fn step_export_writes_reusable_ap242_product_structure_and_local_brep() {
        let (document, _, root_transform, local_transforms) = repeated_native_box_assembly();
        let exported = TruckKernel::default()
            .encode_step(&document, "fixture.step")
            .unwrap();

        assert!(exported.contains("CADX AP242 assembly model"));
        assert!(exported.contains("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF"));
        assert_eq!(exported.matches("MANIFOLD_SOLID_BREP").count(), 1);
        assert_eq!(
            exported.matches("NEXT_ASSEMBLY_USAGE_OCCURRENCE").count(),
            3
        );
        validate_step(&exported).unwrap();
        let table = Table::from_step(&exported).unwrap();
        assert_eq!(table.shell.len(), 1);
        assert!(
            !table
                .to_compressed_shell(table.shell.values().next().unwrap())
                .unwrap()
                .triangulation(0.01)
                .to_polygon()
                .tri_faces()
                .is_empty()
        );

        let parsed = parse_step(exported).unwrap();
        assert_eq!(parsed.bodies.len(), 1);
        assert!(parsed.standalone_body_indices.is_empty());
        assert_eq!(parsed.assemblies.len(), 1);
        let assembly = &parsed.assemblies[0];
        assert_eq!(assembly.name, "Export fixture");
        assert_eq!(assembly.definitions.len(), 3);
        assert!(
            assembly
                .definitions
                .iter()
                .any(|definition| definition.name == "Top assembly")
        );
        assert!(
            assembly
                .definitions
                .iter()
                .any(|definition| definition.name == "Bracket part")
        );
        assert_eq!(assembly.occurrences.len(), 4);
        let root = assembly
            .occurrences
            .iter()
            .find(|occurrence| occurrence.name == "Root placement")
            .unwrap();
        assert!(root.transform.approximately_equals(root_transform, 1.0e-10));
        for (name, expected) in ["Bracket left", "Bracket right"]
            .into_iter()
            .zip(local_transforms)
        {
            let occurrence = assembly
                .occurrences
                .iter()
                .find(|occurrence| occurrence.name == name)
                .unwrap();
            assert!(occurrence.transform.approximately_equals(expected, 1.0e-10));
            assert_eq!(occurrence.parent_id, Some(root.id));
            assert_eq!(occurrence.body_indices, vec![0]);
        }
    }

    #[test]
    fn step_export_keeps_standalone_body_outside_ap242_assembly() {
        let (mut document, _, _, _) = repeated_native_box_assembly();
        document
            .apply(ModelCommand::CreateBox {
                name: "Loose body".into(),
                size: [5.0, 6.0, 7.0],
                position: [-30.0, 0.0, 0.0],
            })
            .unwrap();

        let exported = TruckKernel::default()
            .encode_step(&document, "fixture-with-loose-body.step")
            .unwrap();
        assert!(exported.contains("'Loose body'"));
        let parsed = parse_step(exported).unwrap();
        assert_eq!(parsed.bodies.len(), 2);
        assert_eq!(parsed.standalone_body_indices, vec![1]);
        assert_eq!(parsed.assemblies.len(), 1);
        let repeated = parsed.assemblies[0]
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.name.starts_with("Bracket "))
            .collect::<Vec<_>>();
        assert_eq!(repeated.len(), 2);
        assert!(
            repeated
                .iter()
                .all(|occurrence| occurrence.body_indices == vec![0])
        );
    }

    #[test]
    fn step_export_excludes_effectively_suppressed_subtree() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "suppressed descendant".into(),
                    size: [2.0; 3],
                    position: [10.0, 0.0, 0.0],
                },
                ModelCommand::CreateBox {
                    name: "standalone survivor".into(),
                    size: [3.0; 3],
                    position: [-10.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "Suppressed fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "Root".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "Branch".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 3,
                        name: "Leaf".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "active root".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "suppressed branch".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: true,
                        transform: AssemblyTransform {
                            translation: [5.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "inherited leaf".into(),
                        definition_id: 3,
                        parent_id: Some(2),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [5.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![ids[0]],
                        source: None,
                    },
                ],
            })
            .unwrap();

        let exported = TruckKernel::default()
            .encode_step(&document, "suppressed-tree.step")
            .unwrap();
        assert!(!exported.contains("suppressed branch"));
        assert!(!exported.contains("inherited leaf"));
        let parsed = parse_step(exported).unwrap();
        assert_eq!(parsed.bodies.len(), 1);
        assert_eq!(parsed.standalone_body_indices, vec![0]);
        assert_eq!(parsed.assemblies[0].occurrences.len(), 2);
    }

    #[test]
    fn step_export_rejects_occurrence_specific_repeated_geometry() {
        let (mut document, ids, _, _) = repeated_native_box_assembly();
        document
            .apply(ModelCommand::ResizeBox {
                id: ids[1],
                size: [9.0, 3.0, 4.0],
            })
            .unwrap();

        assert!(matches!(
            TruckKernel::default().encode_step(&document, "incompatible.step"),
            Err(KernelError::Exchange {
                format: "STEP",
                message
            }) if message.contains("one reusable AP242 product definition")
        ));
    }

    #[test]
    fn step_import_rebuilds_a_persistable_solid() {
        use truck_stepio::r#in::Table;

        let mut source_document = CadDocument::default();
        source_document
            .apply(ModelCommand::CreateBox {
                name: "source".into(),
                size: [10.0, 12.0, 14.0],
                position: [2.0, 3.0, 4.0],
            })
            .unwrap();
        let source = TruckKernel::default()
            .encode_step(&source_document, "source.step")
            .unwrap();
        let table = Table::from_step(&source).unwrap();
        let shell_id = *table.shell.keys().next().unwrap();

        let mut imported = CadDocument::default();
        let id = imported
            .apply(ModelCommand::ImportStep {
                name: "imported source".into(),
                source,
                data_section: 0,
                shell_id,
                void_shells: Vec::new(),
                length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                color: None,
                position: [5.0, 6.0, 7.0],
            })
            .unwrap()
            .unwrap();
        let scene = TruckKernel::default().evaluate(&imported).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, id);
        assert!(scene.parts[0].mesh.triangle_count() > 0);
        assert!(!scene.parts[0].faces.is_empty());
        let min_x = scene.parts[0]
            .mesh
            .positions
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        assert!((min_x - 7.0).abs() < 1.0e-4);
    }

    #[test]
    fn step_import_converts_source_length_units_to_millimeters() {
        use truck_stepio::r#in::Table;

        let mut source_document = CadDocument::default();
        source_document
            .apply(ModelCommand::CreateBox {
                name: "one source unit".into(),
                size: [1.0, 2.0, 3.0],
                position: [0.0; 3],
            })
            .unwrap();
        let source = TruckKernel::default()
            .encode_step(&source_document, "inch-source.step")
            .unwrap();
        let shell_id = *Table::from_step(&source)
            .unwrap()
            .shell
            .keys()
            .next()
            .unwrap();
        let mut imported = CadDocument::default();
        imported
            .apply(ModelCommand::ImportStep {
                name: "inch body".into(),
                source,
                data_section: 0,
                shell_id,
                void_shells: Vec::new(),
                length_unit: cadx_core::domain::StepLengthUnit {
                    name: "inch".into(),
                    millimeters_per_unit: 25.4,
                    declared: true,
                },
                color: None,
                position: [0.0; 3],
            })
            .unwrap();

        let scene = TruckKernel::default().evaluate(&imported).unwrap();
        let bounds = scene.parts[0].mesh.positions.iter().fold(
            ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
            |(mut min, mut max), point| {
                for axis in 0..3 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
                (min, max)
            },
        );
        assert!((bounds.1[0] - 25.4).abs() < 1.0e-3);
        assert!((bounds.1[1] - 50.8).abs() < 1.0e-3);
        assert!((bounds.1[2] - 76.2).abs() < 1.0e-3);
    }

    #[test]
    fn step_import_reconstructs_brep_with_voids_as_one_solid() {
        let box_solid = |origin: Point3, size: f64| {
            let vertex = builder::vertex(origin);
            let edge = builder::tsweep(&vertex, Vector3::new(size, 0.0, 0.0));
            let face = builder::tsweep(&edge, Vector3::new(0.0, size, 0.0));
            builder::tsweep(&face, Vector3::new(0.0, 0.0, size))
        };
        let outer = box_solid(Point3::origin(), 10.0)
            .into_boundaries()
            .pop()
            .unwrap();
        let cavity = box_solid(Point3::new(2.0, 2.0, 2.0), 6.0)
            .into_boundaries()
            .pop()
            .unwrap();
        let hollow = Solid::try_new(vec![outer, cavity]).unwrap().compress();
        let generated = truck_stepio::out::CompleteStepDisplay::new(
            truck_stepio::out::StepModel::from(&hollow),
            StepHeaderDescriptor::default(),
        )
        .to_string();
        let source = generated
            .lines()
            .map(|line| {
                if line.contains("ORIENTED_CLOSED_SHELL") {
                    line.replace(".T.);", ".F.);")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let kernel = TruckKernel::default();
        assert!(source.contains("BREP_WITH_VOIDS"));

        let mut parsed = parse_step(source.clone()).unwrap();
        assert_eq!(parsed.bodies.len(), 1);
        let body = parsed.bodies.pop().unwrap();
        assert_eq!(body.void_shells.len(), 1);
        assert!(!body.void_shells[0].orientation);

        let mut imported = CadDocument::default();
        imported
            .apply(ModelCommand::ImportStep {
                name: body.name.unwrap_or_else(|| "hollow housing".into()),
                source,
                data_section: body.data_section,
                shell_id: body.shell_id,
                void_shells: body.void_shells,
                length_unit: body.length_unit,
                color: body.color.color(),
                position: [0.0; 3],
            })
            .unwrap();

        let scene = kernel.evaluate(&imported).unwrap();
        assert_eq!(scene.parts.len(), 1);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 784.0).abs() < 1.0e-3);

        let reexported = kernel
            .encode_step(&imported, "reexported-hollow-housing.step")
            .unwrap();
        assert!(reexported.contains("BREP_WITH_VOIDS"));
        let reparsed = parse_step(reexported).unwrap();
        assert_eq!(reparsed.bodies.len(), 1);
        assert_eq!(reparsed.bodies[0].void_shells.len(), 1);
    }

    #[test]
    fn step_import_rebuilds_planar_and_curved_solids() {
        use truck_stepio::r#in::Table;

        let mut source_document = CadDocument::default();
        source_document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "box".into(),
                    size: [8.0, 9.0, 10.0],
                    position: [0.0, 0.0, 0.0],
                },
                ModelCommand::CreateCylinder {
                    name: "cylinder".into(),
                    radius: 3.0,
                    height: 8.0,
                    position: [20.0, 0.0, 0.0],
                },
                ModelCommand::CreateSphere {
                    name: "sphere".into(),
                    radius: 4.0,
                    position: [40.0, 0.0, 0.0],
                },
                ModelCommand::CreateCone {
                    name: "cone".into(),
                    bottom_radius: 4.0,
                    top_radius: 2.0,
                    height: 9.0,
                    position: [60.0, 0.0, 0.0],
                },
                ModelCommand::CreateTorus {
                    name: "torus".into(),
                    major_radius: 6.0,
                    minor_radius: 2.0,
                    position: [85.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        let source = TruckKernel::default()
            .encode_step(&source_document, "curved.step")
            .unwrap();
        let mut shell_ids = Table::from_step(&source)
            .unwrap()
            .shell
            .keys()
            .copied()
            .collect::<Vec<_>>();
        shell_ids.sort_unstable();
        let commands =
            shell_ids
                .into_iter()
                .enumerate()
                .map(|(index, shell_id)| ModelCommand::ImportStep {
                    name: format!("imported {index}"),
                    source: source.clone(),
                    data_section: 0,
                    shell_id,
                    void_shells: Vec::new(),
                    length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                    color: None,
                    position: [0.0; 3],
                });
        let mut imported = CadDocument::default();
        imported.apply_transaction(commands).unwrap();
        let scene = TruckKernel::default().evaluate(&imported).unwrap();
        assert_eq!(scene.parts.len(), 5);
        assert!(
            scene
                .parts
                .iter()
                .all(|part| part.mesh.triangle_count() > 0 && !part.faces.is_empty())
        );
    }

    #[test]
    fn repeated_component_imports_share_local_brep_and_keep_instance_topology() {
        let (mut document, ids) = repeated_imported_box_assembly();
        let reusable = shared_imported_step_bodies(&document);
        assert_eq!(reusable.len(), 2);
        assert_eq!(reusable[&ids[0]], reusable[&ids[1]]);

        let mut cache = ImportedStepSolidCache::default();
        let first_solid = TruckKernel::default()
            .build_solid(
                document.feature(ids[0]).unwrap(),
                &document,
                &HashMap::new(),
                &HashMap::new(),
                &reusable,
                &mut cache,
            )
            .unwrap();
        let second_solid = TruckKernel::default()
            .build_solid(
                document.feature(ids[1]).unwrap(),
                &document,
                &HashMap::new(),
                &HashMap::new(),
                &reusable,
                &mut cache,
            )
            .unwrap();
        assert_eq!(cache.local_solids.len(), 1);
        assert!(
            first_solid
                .solid
                .face_iter()
                .zip(second_solid.solid.face_iter())
                .all(|(first, second)| first.id() != second.id())
        );

        reset_imported_step_reconstruction_count();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(imported_step_reconstruction_count(), 1);
        assert_eq!(scene.parts.len(), 2);
        assert_eq!(scene.mesh_definitions.len(), 1);
        assert_eq!(scene.mesh_instances.len(), 2);
        let definition = &scene.mesh_definitions[0];
        assert_eq!(definition.key, reusable[&ids[0]]);
        let (local_min, local_max) = mesh_bounds(&definition.mesh);
        assert!(
            local_min
                .into_iter()
                .zip([0.0; 3])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-6)
        );
        assert!(
            local_max
                .into_iter()
                .zip([10.0; 3])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-6)
        );
        for instance in &scene.mesh_instances {
            let part = scene
                .parts
                .iter()
                .find(|part| part.feature_id == instance.feature_id)
                .unwrap();
            let reconstructed = transform_triangle_mesh(&definition.mesh, instance.transform);
            assert_eq!(reconstructed.indices, part.mesh.indices);
            assert!(
                reconstructed
                    .positions
                    .iter()
                    .flatten()
                    .zip(part.mesh.positions.iter().flatten())
                    .all(|(actual, expected)| (actual - expected).abs() < 1.0e-4)
            );
            assert!(
                reconstructed
                    .normals
                    .iter()
                    .flatten()
                    .zip(part.mesh.normals.iter().flatten())
                    .all(|(actual, expected)| (actual - expected).abs() < 1.0e-4)
            );
        }

        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        let first = analysis.part(ids[0]).unwrap();
        let second = analysis.part(ids[1]).unwrap();
        assert!((first.bounds.min[0] - 0.0).abs() < 1.0e-6);
        assert!((first.bounds.max[0] - 10.0).abs() < 1.0e-6);
        assert!((second.bounds.min[0] - 4.0).abs() < 1.0e-6);
        assert!((second.bounds.max[0] - 14.0).abs() < 1.0e-6);

        let first_part = scene
            .parts
            .iter()
            .find(|part| part.feature_id == ids[0])
            .unwrap();
        let second_part = scene
            .parts
            .iter()
            .find(|part| part.feature_id == ids[1])
            .unwrap();
        assert!(
            first_part
                .faces
                .iter()
                .all(|face| face.reference.feature_id == ids[0])
        );
        assert!(
            second_part
                .faces
                .iter()
                .all(|face| face.reference.feature_id == ids[1])
        );
        let first_face = &first_part.faces[0].reference;
        let second_face = &second_part.faces[0].reference;
        assert_ne!(first_face, second_face);
        assert!(scene.face(first_face).is_some());
        assert!(scene.face(second_face).is_some());

        reset_imported_step_reconstruction_count();
        assert!(matches!(
            TruckKernel::default().encode_step(&document, "repeated-components.step"),
            Err(KernelError::Exchange { format: "STEP", .. })
        ));
        assert_eq!(imported_step_reconstruction_count(), 0);

        let definition_color = document.feature(ids[0]).unwrap().color;
        document
            .apply(ModelCommand::SetColor {
                id: ids[1],
                color: definition_color,
            })
            .unwrap();
        reset_imported_step_reconstruction_count();
        let exported = TruckKernel::default()
            .encode_step(&document, "repeated-components.step")
            .unwrap();
        assert_eq!(imported_step_reconstruction_count(), 1);
        let parsed = parse_step(exported).unwrap();
        assert_eq!(parsed.bodies.len(), 1);
        assert_eq!(parsed.assemblies[0].occurrences.len(), 4);
    }

    #[test]
    fn interference_analysis_reports_exact_brep_overlap_deterministically() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "first".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "second".into(),
                    size: [10.0; 3],
                    position: [5.0, 2.0, 1.0],
                },
                ModelCommand::CreateBox {
                    name: "clear".into(),
                    size: [10.0; 3],
                    position: [30.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::SetVisibility {
                id: ids[1],
                visible: false,
            })
            .unwrap();

        let kernel = TruckKernel::default();
        assert!(kernel.capabilities().interference_analysis);
        let first = kernel.analyze_interference(&document).unwrap();
        let second = kernel.analyze_interference(&document).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.candidate_feature_ids, ids);
        assert_eq!(first.total_pair_count, 3);
        assert_eq!(first.broad_phase_pair_count, 1);
        assert_eq!(first.interfering_pair_count, 1);
        assert_eq!(first.clear_pair_count, 2);
        assert_eq!(first.failed_pair_count, 0);
        assert_eq!(first.pairs[0].feature_ids, [ids[0], ids[1]]);
        let InterferencePairOutcome::Interfering { volume_mm3, .. } = first.pairs[0].outcome else {
            panic!("partially overlapping boxes must interfere");
        };
        assert!((volume_mm3 - 360.0).abs() < 1.0e-6);
    }

    #[test]
    fn interference_analysis_distinguishes_touching_and_containment() {
        let kernel = TruckKernel::default();
        for (position, size, expected_volume, expected_overlap) in [
            ([10.0, 0.0, 0.0], [10.0; 3], 0.0, false),
            ([2.0; 3], [6.0; 3], 216.0, true),
        ] {
            let mut document = CadDocument::default();
            document
                .apply_transaction([
                    ModelCommand::CreateBox {
                        name: "outer".into(),
                        size: [10.0; 3],
                        position: [0.0; 3],
                    },
                    ModelCommand::CreateBox {
                        name: "candidate".into(),
                        size,
                        position,
                    },
                ])
                .unwrap();
            let report = kernel.analyze_interference(&document).unwrap();
            assert_eq!(report.total_pair_count, 1);
            assert_eq!(report.broad_phase_pair_count, 1);
            assert_eq!(report.failed_pair_count, 0);
            assert_eq!(report.has_interference(), expected_overlap);
            let volume = match report.pairs[0].outcome {
                InterferencePairOutcome::Clear { volume_mm3, .. }
                | InterferencePairOutcome::Interfering { volume_mm3, .. } => volume_mm3,
                InterferencePairOutcome::Failed { ref detail, .. } => {
                    panic!("pair analysis failed: {detail}")
                }
            };
            assert!((volume - expected_volume).abs() < 1.0e-6);
        }
    }

    #[test]
    fn interference_analysis_uses_terminal_solid_products() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "left".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "right".into(),
                    size: [10.0; 3],
                    position: [5.0, 2.0, 1.0],
                },
            ])
            .unwrap();
        let result = document
            .apply(ModelCommand::CreateBoolean {
                name: "union".into(),
                operation: BooleanOperation::Union,
                left: ids[0],
                right: ids[1],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::SetVisibility {
                id: ids[0],
                visible: true,
            })
            .unwrap();

        let report = TruckKernel::default()
            .analyze_interference(&document)
            .unwrap();
        assert_eq!(report.candidate_feature_ids, vec![result]);
        assert_eq!(report.total_pair_count, 0);
        assert!(!report.has_interference());
    }

    #[test]
    fn occurrence_interference_respects_suppression_and_reuses_definitions() {
        let (mut document, ids) = repeated_imported_box_assembly();
        reset_imported_step_reconstruction_count();
        let report = TruckKernel::default()
            .analyze_interference(&document)
            .unwrap();
        assert_eq!(imported_step_reconstruction_count(), 1);
        assert_eq!(report.candidate_feature_ids, ids);
        assert_eq!(report.interfering_pair_count, 1);
        let InterferencePairOutcome::Interfering { volume_mm3, .. } = report.pairs[0].outcome
        else {
            panic!("repeated occurrences overlap");
        };
        assert!((volume_mm3 - 336.0).abs() < 1.0e-6);

        document
            .apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 3,
                suppressed: true,
            })
            .unwrap();
        let suppressed = TruckKernel::default()
            .analyze_interference(&document)
            .unwrap();
        assert_eq!(suppressed.candidate_feature_ids, vec![ids[0]]);
        assert_eq!(suppressed.total_pair_count, 0);
    }

    #[test]
    fn multi_body_occurrences_do_not_self_interfere() {
        let (document, _) = repeated_multi_body_assembly();
        let report = TruckKernel::default()
            .analyze_interference(&document)
            .unwrap();
        assert_eq!(report.candidate_feature_ids.len(), 4);
        assert_eq!(report.total_pair_count, 4);
        assert_eq!(report.broad_phase_pair_count, 0);
        assert_eq!(report.clear_pair_count, 4);
    }

    #[test]
    fn repeated_component_import_cache_feeds_transformed_boolean_operands() {
        let (mut document, ids) = repeated_imported_box_assembly();
        let boolean_id = document
            .apply(ModelCommand::CreateBoolean {
                name: "shared bracket intersection".into(),
                operation: BooleanOperation::Intersect,
                left: ids[0],
                right: ids[1],
            })
            .unwrap()
            .unwrap();

        reset_imported_step_reconstruction_count();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(imported_step_reconstruction_count(), 1);
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, boolean_id);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!((analysis.total_volume_mm3 - 336.0).abs() < 1.0e-3);
    }

    #[test]
    fn suppressed_occurrence_is_excluded_from_scene_analysis_and_step_export() {
        let (mut document, ids) = repeated_imported_box_assembly();
        document
            .apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 3,
                suppressed: true,
            })
            .unwrap();

        reset_imported_step_reconstruction_count();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(imported_step_reconstruction_count(), 1);
        assert_eq!(scene.parts.len(), 1);
        assert_eq!(scene.parts[0].feature_id, ids[0]);
        assert!(scene.mesh_definitions.is_empty());
        assert!(scene.mesh_instances.is_empty());
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        assert!(analysis.part(ids[0]).is_some());
        assert!(analysis.part(ids[1]).is_none());

        reset_imported_step_reconstruction_count();
        let exported = TruckKernel::default()
            .encode_step(&document, "suppressed-component.step")
            .unwrap();
        assert_eq!(imported_step_reconstruction_count(), 1);
        assert_eq!(parse_step(exported).unwrap().bodies.len(), 1);
    }

    #[test]
    fn repeated_native_component_bodies_emit_renderer_instances() {
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "native bracket 1".into(),
                    size: [2.0, 3.0, 4.0],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "native bracket 2".into(),
                    size: [2.0, 3.0, 4.0],
                    position: [15.0, 5.0, 0.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::Rotate {
                id: ids[1],
                rotation: [0.0, 0.0, 90.0],
            })
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "native fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "fixture".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "native bracket".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "fixture".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "native bracket 1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: vec![ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "native bracket 2".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::from_euler_xyz_degrees(
                            [15.0, 5.0, 0.0],
                            [0.0, 0.0, 90.0],
                        ),
                        feature_ids: vec![ids[1]],
                        source: None,
                    },
                ],
            })
            .unwrap();

        assert!(shared_imported_step_bodies(&document).is_empty());
        let reusable = reusable_render_bodies(&document);
        assert_eq!(reusable.len(), 2);
        assert_eq!(reusable[&ids[0]], reusable[&ids[1]]);
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.mesh_definitions.len(), 1);
        assert_eq!(scene.mesh_instances.len(), 2);
        let local_bounds = mesh_bounds(&scene.mesh_definitions[0].mesh);
        assert!(
            local_bounds
                .0
                .into_iter()
                .zip([0.0; 3])
                .chain(local_bounds.1.into_iter().zip([2.0, 3.0, 4.0]))
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-5)
        );
    }

    #[test]
    fn repeated_multi_body_components_cache_each_ordered_body_slot() {
        let (document, ids) = repeated_multi_body_assembly();
        let reusable = shared_imported_step_bodies(&document);
        assert_eq!(reusable.len(), 4);
        assert_eq!(reusable[&ids[0][0]], reusable[&ids[1][0]]);
        assert_eq!(reusable[&ids[0][1]], reusable[&ids[1][1]]);
        assert_ne!(reusable[&ids[0][0]], reusable[&ids[0][1]]);

        reset_imported_step_reconstruction_count();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 4);
        assert_eq!(imported_step_reconstruction_count(), 2);
        assert_eq!(scene.mesh_definitions.len(), 2);
        assert_eq!(scene.mesh_instances.len(), 4);
        assert_ne!(scene.mesh_definitions[0].key, scene.mesh_definitions[1].key);
        let analysis = cadx_analysis::analyze_scene(&scene, None).unwrap();
        for occurrence in ids {
            assert!((analysis.part(occurrence[0]).unwrap().volume_mm3 - 1_000.0).abs() < 1.0e-3);
            assert!((analysis.part(occurrence[1]).unwrap().volume_mm3 - 216.0).abs() < 1.0e-3);
        }
    }

    #[test]
    fn imported_step_cache_rejects_incompatible_definition_bodies() {
        let (mut document, ids) = repeated_imported_box_assembly();
        let Primitive::ImportedStep { length_unit, .. } = &mut document
            .features
            .iter_mut()
            .find(|feature| feature.id == ids[1])
            .unwrap()
            .primitive
        else {
            unreachable!();
        };
        *length_unit = cadx_core::domain::StepLengthUnit::assumed_millimeter();
        document.validate_and_repair().unwrap();
        assert!(shared_imported_step_bodies(&document).is_empty());

        reset_imported_step_reconstruction_count();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 2);
        assert_eq!(imported_step_reconstruction_count(), 2);
    }

    #[test]
    fn identical_standalone_imports_are_reconstructed_independently() {
        let (mut document, _) = repeated_imported_box_assembly();
        document
            .apply(ModelCommand::DeleteAssembly { id: 1 })
            .unwrap();
        assert!(shared_imported_step_bodies(&document).is_empty());

        reset_imported_step_reconstruction_count();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 2);
        assert_eq!(imported_step_reconstruction_count(), 2);
    }

    #[test]
    fn imported_step_solid_can_drive_a_boolean() {
        use truck_stepio::r#in::Table;

        let mut source_document = CadDocument::default();
        source_document
            .apply(ModelCommand::CreateBox {
                name: "source".into(),
                size: [10.0, 10.0, 10.0],
                position: [0.0; 3],
            })
            .unwrap();
        let source = TruckKernel::default()
            .encode_step(&source_document, "source.step")
            .unwrap();
        let shell_id = *Table::from_step(&source)
            .unwrap()
            .shell
            .keys()
            .next()
            .unwrap();
        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::ImportStep {
                    name: "imported".into(),
                    source,
                    data_section: 0,
                    shell_id,
                    void_shells: Vec::new(),
                    length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                    color: None,
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "tool".into(),
                    size: [4.0, 4.0, 4.0],
                    position: [3.0, 3.0, 3.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::CreateBoolean {
                name: "cut imported".into(),
                operation: BooleanOperation::Subtract,
                left: ids[0],
                right: ids[1],
            })
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(scene.parts[0].mesh.triangle_count() > 0);
    }

    #[test]
    fn malformed_imported_step_fails_closed() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::ImportStep {
                name: "invalid".into(),
                source: "ISO-10303-21;\nHEADER;\nENDSEC;\nEND-ISO-10303-21;".into(),
                data_section: 0,
                shell_id: 1,
                void_shells: Vec::new(),
                length_unit: cadx_core::domain::StepLengthUnit::default(),
                color: None,
                position: [0.0; 3],
            })
            .unwrap();
        assert!(matches!(
            TruckKernel::default().evaluate(&document),
            Err(KernelError::Evaluation { .. })
        ));
    }

    #[test]
    fn truck_extrudes_exact_rational_and_cubic_sketch_edges_and_exports_step() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                name: "Exact freeform profile".into(),
                plane: SketchPlane::WorldXy,
                region: SketchRegion2D {
                    profile: SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::CubicBezier {
                                start: [0.0, 0.0],
                                control1: [3.0, -2.0],
                                control2: [7.0, -2.0],
                                end: [10.0, 0.0],
                            },
                            SketchSegment2D::Line {
                                start: [10.0, 0.0],
                                end: [10.0, 10.0],
                            },
                            SketchSegment2D::RationalQuadratic {
                                start: [10.0, 10.0],
                                control: [5.0, 14.0],
                                end: [0.0, 10.0],
                                weight: 0.8,
                            },
                            SketchSegment2D::Line {
                                start: [0.0, 10.0],
                                end: [0.0, 0.0],
                            },
                        ],
                    },
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "Exact freeform solid".into(),
                sketch_id: sketch,
                height: 5.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        let overlay = scene
            .sketches
            .iter()
            .find(|evaluated| evaluated.feature_id == sketch)
            .unwrap();
        assert!(overlay.profile.len() > 8);
        assert!(
            overlay
                .profile
                .iter()
                .all(|point| point.iter().all(|value| value.is_finite()))
        );
        let part = scene
            .parts
            .iter()
            .find(|part| part.feature_id == extrusion)
            .unwrap();
        assert!(
            part.edges
                .iter()
                .any(|edge| edge.geometry.curve == CurveKind::BSpline)
        );
        assert!(
            part.edges
                .iter()
                .any(|edge| edge.geometry.curve == CurveKind::Nurbs)
        );
        assert!(part.mesh.triangle_count() > 0);

        let step = kernel.encode_step(&document, "freeform.step").unwrap();
        validate_step(&step).unwrap();
        assert!(step.contains("B_SPLINE_CURVE"));
        assert!(step.contains("RATIONAL_B_SPLINE_CURVE"));
    }

    #[test]
    fn truck_revolves_exact_rational_and_cubic_sketch_edges() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                name: "Freeform turning profile".into(),
                plane: SketchPlane::WorldXz,
                region: SketchRegion2D {
                    profile: SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::CubicBezier {
                                start: [2.0, 0.0],
                                control1: [4.0, -1.0],
                                control2: [8.0, -1.0],
                                end: [10.0, 0.0],
                            },
                            SketchSegment2D::Line {
                                start: [10.0, 0.0],
                                end: [10.0, 8.0],
                            },
                            SketchSegment2D::RationalQuadratic {
                                start: [10.0, 8.0],
                                control: [6.0, 10.0],
                                end: [2.0, 8.0],
                                weight: 0.75,
                            },
                            SketchSegment2D::Line {
                                start: [2.0, 8.0],
                                end: [2.0, 0.0],
                            },
                        ],
                    },
                    holes: Vec::new(),
                },
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "Freeform turned body".into(),
                sketch_id: sketch,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 180.0,
                position: [0.0; 3],
            })
            .unwrap();

        let kernel = TruckKernel::default();
        let scene = kernel.evaluate(&document).unwrap();
        assert_eq!(scene.parts.len(), 1);
        assert!(scene.parts[0].mesh.triangle_count() > 0);
        assert!(
            scene.parts[0]
                .edges
                .iter()
                .any(|edge| matches!(edge.geometry.curve, CurveKind::BSpline | CurveKind::Nurbs))
        );
        validate_step(
            &kernel
                .encode_step(&document, "freeform-revolve.step")
                .unwrap(),
        )
        .unwrap();
    }
}
