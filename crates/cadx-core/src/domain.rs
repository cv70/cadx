use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use thiserror::Error;

use crate::{
    assembly::{
        Assembly, AssemblyError, AssemblyId, AssemblyMate, AssemblyMateId, AssemblyTransform,
        ComponentDefinition, ComponentOccurrence, ComponentOccurrenceId,
    },
    diagnostics::{SketchConstraintDiagnostic, SketchConstraintFailureReason},
    topology::{EdgeRef, FaceRef, VertexRef},
};

pub use cadx_sketch::{
    Constraint, MAX_CONSTRUCTION_SEGMENTS, SketchDimension, SketchDimensionKind, SketchLoop2D,
    SketchRegion2D, SketchSegment2D, construction_point_ids, construction_segment_id,
};
use cadx_sketch::{SketchError, SolverConfig, solve_sketch};

pub type FeatureId = u64;

pub const MAX_MATERIAL_DENSITY_KG_M3: f64 = 100_000.0;
pub const MAX_LOFT_SECTIONS: usize = 32;
pub const MAX_STEP_UNIT_NAME_LENGTH: usize = 80;
pub const MAX_STEP_VOID_SHELLS: usize = 4_096;

/// Length-unit declaration retained with an imported STEP body.
///
/// CADX geometry is always evaluated in millimeters. Keeping the source unit
/// and its exact conversion factor makes an import portable and auditable
/// without reparsing external files or relying on UI preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepLengthUnit {
    pub name: String,
    pub millimeters_per_unit: f64,
    /// False only for legacy or unitless exchange files interpreted as mm.
    #[serde(default)]
    pub declared: bool,
}

impl StepLengthUnit {
    #[must_use]
    pub fn millimeter() -> Self {
        Self {
            name: "millimetre".into(),
            millimeters_per_unit: 1.0,
            declared: true,
        }
    }

    #[must_use]
    pub fn assumed_millimeter() -> Self {
        Self {
            declared: false,
            ..Self::millimeter()
        }
    }

    fn validate(&self) -> Result<(), DocumentError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_STEP_UNIT_NAME_LENGTH {
            return Err(DocumentError::InvalidParameter(format!(
                "STEP length unit name must contain 1 to {MAX_STEP_UNIT_NAME_LENGTH} characters"
            )));
        }
        if !self.millimeters_per_unit.is_finite() || self.millimeters_per_unit <= 0.0 {
            return Err(DocumentError::InvalidParameter(
                "STEP millimeters-per-unit factor must be finite and greater than zero".into(),
            ));
        }
        if !self.declared
            && (name != "millimetre" || (self.millimeters_per_unit - 1.0).abs() > f64::EPSILON)
        {
            return Err(DocumentError::InvalidParameter(
                "an assumed STEP length unit must be millimetres at 1 mm per unit".into(),
            ));
        }
        Ok(())
    }
}

impl Default for StepLengthUnit {
    fn default() -> Self {
        Self::assumed_millimeter()
    }
}

/// One oriented inner boundary of a STEP `BREP_WITH_VOIDS` solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepShellBoundary {
    /// Entity id of the underlying `CLOSED_SHELL` in the persisted DATA section.
    pub shell_id: u64,
    /// Whether the oriented shell uses the underlying shell's face orientation.
    pub orientation: bool,
}

/// Kernel-neutral physical material metadata attached to a solid feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub density_kg_m3: f64,
}

impl Material {
    fn validated(name: &str, density_kg_m3: f64) -> Result<Self, DocumentError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DocumentError::InvalidParameter(
                "material name must not be empty".into(),
            ));
        }
        if name.chars().count() > 80 {
            return Err(DocumentError::InvalidParameter(
                "material name must not exceed 80 characters".into(),
            ));
        }
        if !density_kg_m3.is_finite()
            || density_kg_m3 <= 0.0
            || density_kg_m3 > MAX_MATERIAL_DENSITY_KG_M3
        {
            return Err(DocumentError::InvalidParameter(format!(
                "material density must be finite and between 0 and {MAX_MATERIAL_DENSITY_KG_M3} kg/m^3"
            )));
        }
        Ok(Self {
            name: name.into(),
            density_kg_m3,
        })
    }

    fn validate(&self) -> Result<(), DocumentError> {
        Self::validated(&self.name, self.density_kg_m3).map(|_| ())
    }
}

/// A parametric solid operation that consumes two upstream features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOperation {
    Union,
    Subtract,
    Intersect,
}

impl BooleanOperation {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Union => "Union",
            Self::Subtract => "Subtract",
            Self::Intersect => "Intersect",
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedEdgeRefs {
    One(EdgeRef),
    Many(Vec<EdgeRef>),
}

fn deserialize_edge_refs<'de, D>(deserializer: D) -> Result<Vec<EdgeRef>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match SerializedEdgeRefs::deserialize(deserializer)? {
        SerializedEdgeRefs::One(edge) => vec![edge],
        SerializedEdgeRefs::Many(edges) => edges,
    })
}

fn edge_refs_are_canonical(edges: &[EdgeRef]) -> bool {
    let Some(source_id) = edges.first().map(|edge| edge.feature_id) else {
        return false;
    };
    source_id != 0
        && edges.iter().all(|edge| edge.feature_id == source_id)
        && edges.windows(2).all(|pair| pair[0] < pair[1])
}

fn canonicalize_edge_refs(edges: &mut Vec<EdgeRef>) -> Result<(), DocumentError> {
    if edges.is_empty() {
        return Err(DocumentError::InvalidParameter(
            "edge modifier requires at least one edge".into(),
        ));
    }
    let source_id = edges[0].feature_id;
    if source_id == 0 || edges.iter().any(|edge| edge.feature_id != source_id) {
        return Err(DocumentError::InvalidParameter(
            "edge modifier edges must belong to one non-zero source feature".into(),
        ));
    }
    edges.sort_unstable();
    edges.dedup();
    Ok(())
}

fn vertex_ref_is_canonical(vertex: &VertexRef) -> bool {
    vertex.feature_id != 0
        && !vertex.incident_edges.is_empty()
        && vertex.incident_edges.iter().all(|edge| {
            edge.feature_id == vertex.feature_id
                && edge
                    .adjacent_faces
                    .iter()
                    .all(|face| face.feature_id == vertex.feature_id)
        })
        && vertex
            .incident_edges
            .windows(2)
            .all(|pair| pair[0] < pair[1])
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn from_array(value: [f64; 3]) -> Self {
        Self::new(value[0], value[1], value[2])
    }

    #[must_use]
    pub const fn as_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    fn is_strictly_positive(self) -> bool {
        self.x > 0.0 && self.y > 0.0 && self.z > 0.0
    }
}

impl Default for Vec3 {
    fn default() -> Self {
        Self::ZERO
    }
}

/// Kernel-neutral attachment for a two-dimensional sketch.
///
/// Datum attachments retain the parametric feature reference instead of a
/// resolved frame so rebuilding the datum also rebuilds every dependent solid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SketchPlane {
    #[default]
    WorldXy,
    WorldXz,
    WorldYz,
    DatumPlane {
        datum_id: FeatureId,
    },
    PlanarFace {
        face: FaceRef,
    },
}

impl SketchPlane {
    #[must_use]
    pub const fn dependency_id(&self) -> Option<FeatureId> {
        match self {
            Self::DatumPlane { datum_id } => Some(*datum_id),
            Self::PlanarFace { face } => Some(face.feature_id),
            Self::WorldXy | Self::WorldXz | Self::WorldYz => None,
        }
    }

    const fn is_valid(&self) -> bool {
        match self {
            Self::DatumPlane { datum_id } => *datum_id != 0,
            Self::PlanarFace { face } => face.feature_id != 0,
            Self::WorldXy | Self::WorldXz | Self::WorldYz => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Primitive {
    Box {
        size: Vec3,
    },
    Cylinder {
        radius: f64,
        height: f64,
    },
    Sphere {
        radius: f64,
    },
    Cone {
        bottom_radius: f64,
        top_radius: f64,
        height: f64,
    },
    Torus {
        major_radius: f64,
        minor_radius: f64,
    },
    Extrusion {
        profile: Vec<[f64; 2]>,
        height: f64,
    },
    ExtrusionFromSketch {
        sketch_id: FeatureId,
        #[serde(flatten)]
        region: SketchRegion2D,
        height: f64,
    },
    /// A solid created by revolving a solved sketch profile around a local 2D axis.
    RevolveFromSketch {
        sketch_id: FeatureId,
        profile: SketchLoop2D,
        axis_origin: [f64; 2],
        axis_direction: [f64; 2],
        angle: f64,
    },
    /// A ruled solid interpolated through ordered, solved sketch profiles.
    LoftFromSketches {
        sketch_ids: Vec<FeatureId>,
        profiles: Vec<SketchLoop2D>,
    },
    /// A solid imported from a STEP physical file.
    ///
    /// The source is embedded so a CADX document can be evaluated on another
    /// machine without depending on the original filesystem path. The DATA
    /// section and `shell_id` pair identifies the outer shell; `void_shells`
    /// retains every oriented inner boundary. Its source unit is persisted so
    /// every evaluation reconstructs millimeter geometry.
    ImportedStep {
        source: String,
        #[serde(default)]
        data_section: usize,
        shell_id: u64,
        #[serde(default)]
        void_shells: Vec<StepShellBoundary>,
        #[serde(default)]
        length_unit: StepLengthUnit,
    },
    Boolean {
        operation: BooleanOperation,
        left: FeatureId,
        right: FeatureId,
    },
    /// Equal-distance bevel applied to persistent topological edges on one solid.
    Chamfer {
        #[serde(alias = "edge", deserialize_with = "deserialize_edge_refs")]
        edges: Vec<EdgeRef>,
        distance: f64,
    },
    /// Constant-radius round applied to persistent topological edges on one solid.
    Fillet {
        #[serde(alias = "edge", deserialize_with = "deserialize_edge_refs")]
        edges: Vec<EdgeRef>,
        radius: f64,
    },
    /// Reference geometry attached to a persistent topological face.
    ///
    /// Datum planes do not create a solid; they stay in the feature graph so
    /// downstream sketch and manufacturing features can depend on them.
    DatumPlane {
        face: FaceRef,
        offset: f64,
    },
    /// Reference geometry attached to a persistent topological vertex.
    ///
    /// Datum points do not create a solid. Their model-space offset is applied
    /// to the resolved vertex position during kernel evaluation.
    DatumPoint {
        vertex: VertexRef,
        #[serde(default)]
        offset: Vec3,
    },
    Sketch {
        #[serde(default)]
        plane: SketchPlane,
        #[serde(flatten)]
        region: SketchRegion2D,
        #[serde(default)]
        construction: Vec<SketchSegment2D>,
        #[serde(default)]
        constraints: Vec<Constraint>,
    },
}

impl Primitive {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Box { .. } => "Box",
            Self::Cylinder { .. } => "Cylinder",
            Self::Sphere { .. } => "Sphere",
            Self::Cone { .. } => "Cone",
            Self::Torus { .. } => "Torus",
            Self::Extrusion { .. } | Self::ExtrusionFromSketch { .. } => "Extrusion",
            Self::RevolveFromSketch { .. } => "Revolve",
            Self::LoftFromSketches { .. } => "Loft",
            Self::ImportedStep { .. } => "Imported STEP",
            Self::Boolean { operation, .. } => operation.label(),
            Self::Chamfer { .. } => "Chamfer",
            Self::Fillet { .. } => "Fillet",
            Self::DatumPlane { .. } => "Datum plane",
            Self::DatumPoint { .. } => "Datum point",
            Self::Sketch { .. } => "Sketch",
        }
    }

    #[must_use]
    pub const fn is_reference_geometry(&self) -> bool {
        matches!(
            self,
            Self::Sketch { .. } | Self::DatumPlane { .. } | Self::DatumPoint { .. }
        )
    }

    /// Returns all upstream feature ids consumed by this feature.
    #[must_use]
    pub fn dependencies(&self) -> Vec<FeatureId> {
        match self {
            Self::ExtrusionFromSketch { sketch_id, .. }
            | Self::RevolveFromSketch { sketch_id, .. } => vec![*sketch_id],
            Self::LoftFromSketches { sketch_ids, .. } => sketch_ids.clone(),
            Self::Boolean { left, right, .. } => vec![*left, *right],
            Self::Chamfer { edges, .. } | Self::Fillet { edges, .. } => edges
                .first()
                .map_or_else(Vec::new, |edge| vec![edge.feature_id]),
            Self::DatumPlane { face, .. } => vec![face.feature_id],
            Self::DatumPoint { vertex, .. } => vec![vertex.feature_id],
            Self::Sketch { plane, .. } => plane.dependency_id().into_iter().collect(),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn source_sketch(&self) -> Option<FeatureId> {
        match self {
            Self::ExtrusionFromSketch { sketch_id, .. }
            | Self::RevolveFromSketch { sketch_id, .. } => Some(*sketch_id),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), DocumentError> {
        match self {
            Self::Box { size } if size.is_finite() && size.is_strictly_positive() => Ok(()),
            Self::Cylinder { radius, height }
                if radius.is_finite() && height.is_finite() && *radius > 0.0 && *height > 0.0 =>
            {
                Ok(())
            }
            Self::Sphere { radius } if radius.is_finite() && *radius > 0.0 => Ok(()),
            Self::Cone {
                bottom_radius,
                top_radius,
                height,
            } if bottom_radius.is_finite()
                && top_radius.is_finite()
                && height.is_finite()
                && *bottom_radius > 0.0
                && *top_radius >= 0.0
                && *height > 0.0 =>
            {
                Ok(())
            }
            Self::Torus {
                major_radius,
                minor_radius,
            } if major_radius.is_finite()
                && minor_radius.is_finite()
                && *major_radius > 0.0
                && *minor_radius > 0.0
                && *major_radius > *minor_radius =>
            {
                Ok(())
            }
            Self::Extrusion {
                profile, height
            }
                if profile_is_valid(profile) && height.is_finite() && *height > 0.0 =>
            {
                Ok(())
            }
            Self::ExtrusionFromSketch {
                region,
                height,
                ..
            }
                if region.validate().is_ok() && height.is_finite() && *height > 0.0 =>
            {
                Ok(())
            }
            Self::RevolveFromSketch {
                profile,
                axis_origin,
                axis_direction,
                angle,
                ..
            } if revolve_is_valid(profile, *axis_origin, *axis_direction, *angle) => {
                Ok(())
            }
            Self::LoftFromSketches {
                sketch_ids,
                profiles,
            } => validate_loft_definition(sketch_ids, profiles),
            Self::ImportedStep {
                source,
                shell_id,
                void_shells,
                length_unit,
                ..
            } if !source.trim().is_empty() && *shell_id != 0 => {
                validate_step_boundaries(*shell_id, void_shells)?;
                length_unit.validate()
            }
            Self::Sketch {
                plane,
                region,
                construction,
                constraints,
            } => {
                if !plane.is_valid() {
                    return Err(DocumentError::InvalidParameter(
                        "datum-attached sketches must reference a non-zero datum feature id"
                            .into(),
                    ));
                }
                region.validate().map_err(|error| {
                    DocumentError::InvalidParameter(format!("invalid sketch region: {error}"))
                })?;
                solve_sketch_region(region, construction, constraints).map(|_| ())
            }
            Self::Box { .. } => Err(DocumentError::InvalidParameter(
                "box dimensions must be finite and greater than zero".into(),
            )),
            Self::Cylinder { .. } => Err(DocumentError::InvalidParameter(
                "cylinder radius and height must be finite and greater than zero".into(),
            )),
            Self::Sphere { .. } => Err(DocumentError::InvalidParameter(
                "sphere radius must be finite and greater than zero".into(),
            )),
            Self::Cone { .. } => Err(DocumentError::InvalidParameter(
                "cone radii and height must be finite; bottom radius and height must be greater than zero"
                    .into(),
            )),
            Self::Torus { .. } => Err(DocumentError::InvalidParameter(
                "torus radii must be finite and greater than zero; major radius must be larger than minor radius".into(),
            )),
            Self::Extrusion { .. } => Err(DocumentError::InvalidParameter(
                "extrusion profile must be a finite, non-self-intersecting closed polygon with at least three points and height must be greater than zero".into(),
            )),
            Self::ExtrusionFromSketch { .. } => Err(DocumentError::InvalidParameter(
                "sketch-driven extrusion profile must be finite, non-self-intersecting, and height must be greater than zero".into(),
            )),
            Self::RevolveFromSketch { .. } => Err(DocumentError::InvalidParameter(
                "sketch-driven revolve profile must be finite, non-self-intersecting, clear of its axis, and use a finite axis and angle between zero and 360 degrees".into(),
            )),
            Self::ImportedStep { .. } => Err(DocumentError::InvalidParameter(
                "imported STEP features must contain a non-empty source and a non-zero shell entity id".into(),
            )),
            Self::Boolean { left, right, .. } if left == right || *left == 0 || *right == 0 => {
                Err(DocumentError::InvalidParameter(
                    "boolean operands must be two distinct, non-zero feature ids".into(),
                ))
            }
            Self::Boolean { .. } => Ok(()),
            Self::Chamfer { edges, distance }
                if edge_refs_are_canonical(edges) && distance.is_finite() && *distance > 0.0 =>
            {
                Ok(())
            }
            Self::Chamfer { .. } => Err(DocumentError::InvalidParameter(
                "chamfer edges must be a non-empty, sorted, unique set from one non-zero source feature and distance must be finite and greater than zero".into(),
            )),
            Self::Fillet { edges, radius }
                if edge_refs_are_canonical(edges) && radius.is_finite() && *radius > 0.0 =>
            {
                Ok(())
            }
            Self::Fillet { .. } => Err(DocumentError::InvalidParameter(
                "fillet edges must be a non-empty, sorted, unique set from one non-zero source feature and radius must be finite and greater than zero".into(),
            )),
            Self::DatumPlane { face, offset }
                if face.feature_id != 0 && offset.is_finite() =>
            {
                Ok(())
            }
            Self::DatumPlane { .. } => Err(DocumentError::InvalidParameter(
                "datum plane face references must use a non-zero feature id and a finite offset"
                    .into(),
            )),
            Self::DatumPoint { vertex, offset }
                if vertex_ref_is_canonical(vertex) && offset.is_finite() =>
            {
                Ok(())
            }
            Self::DatumPoint { .. } => Err(DocumentError::InvalidParameter(
                "datum point vertex references must contain a non-empty, sorted, unique incident edge set from one non-zero source feature and a finite offset".into(),
            )),
        }
    }
}

fn validate_loft_definition(
    sketch_ids: &[FeatureId],
    profiles: &[SketchLoop2D],
) -> Result<(), DocumentError> {
    if !(2..=MAX_LOFT_SECTIONS).contains(&sketch_ids.len())
        || profiles.len() != sketch_ids.len()
        || sketch_ids.contains(&0)
        || sketch_ids.iter().copied().collect::<BTreeSet<_>>().len() != sketch_ids.len()
    {
        return Err(DocumentError::InvalidParameter(format!(
            "loft requires 2 to {MAX_LOFT_SECTIONS} ordered, unique, non-zero sketch ids with one cached profile per sketch"
        )));
    }
    let segment_count = profiles[0].segments.len();
    let winding = profiles[0].signed_area().is_sign_positive();
    for (index, profile) in profiles.iter().enumerate() {
        profile.validate().map_err(|error| {
            DocumentError::InvalidParameter(format!(
                "loft section {index} is not a valid closed profile: {error}"
            ))
        })?;
        if profile.segments.len() != segment_count {
            return Err(DocumentError::InvalidParameter(format!(
                "loft section {index} has {} segments but section 0 has {segment_count}",
                profile.segments.len()
            )));
        }
        if profile.signed_area().is_sign_positive() != winding {
            return Err(DocumentError::InvalidParameter(format!(
                "loft section {index} traverses in the opposite direction to section 0"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub primitive: Primitive,
    pub translation: Vec3,
    #[serde(default)]
    pub rotation: Vec3,
    pub visible: bool,
    pub color: [f32; 4],
    #[serde(default)]
    pub material: Option<Material>,
}

/// Deterministic dependency graph for a document's parametric features.
///
/// The graph is rebuilt from the declarative document and never stores kernel
/// objects. Its order is stable for a given feature-list order, while always
/// placing dependencies before their consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureGraph {
    order: Vec<FeatureId>,
    dependencies: HashMap<FeatureId, Vec<FeatureId>>,
    dependents: HashMap<FeatureId, Vec<FeatureId>>,
}

impl FeatureGraph {
    fn build(document: &CadDocument) -> Result<Self, DocumentError> {
        let mut index_by_id = HashMap::with_capacity(document.features.len());
        for (index, feature) in document.features.iter().enumerate() {
            if index_by_id.insert(feature.id, index).is_some() {
                return Err(DocumentError::InvalidFeatureId(feature.id));
            }
        }

        let mut dependencies = HashMap::with_capacity(document.features.len());
        let mut dependents: HashMap<FeatureId, Vec<FeatureId>> = HashMap::new();
        let mut indegree = HashMap::with_capacity(document.features.len());
        for feature in &document.features {
            let feature_dependencies = feature.primitive.dependencies();
            if matches!(
                feature.primitive,
                Primitive::Boolean { .. } | Primitive::Chamfer { .. } | Primitive::Fillet { .. }
            ) {
                for dependency in &feature_dependencies {
                    let Some(source) = document.feature(*dependency) else {
                        return Err(DocumentError::InvalidDependency {
                            feature: feature.id,
                            dependency: *dependency,
                            expected: "solid feature",
                        });
                    };
                    if source.primitive.is_reference_geometry() {
                        return Err(DocumentError::InvalidDependency {
                            feature: feature.id,
                            dependency: *dependency,
                            expected: "solid feature",
                        });
                    }
                }
            } else if let Primitive::DatumPlane { face, .. } = &feature.primitive {
                let Some(source) = document.feature(face.feature_id) else {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency: face.feature_id,
                        expected: "solid feature",
                    });
                };
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency: face.feature_id,
                        expected: "solid feature",
                    });
                }
            } else if let Primitive::DatumPoint { vertex, .. } = &feature.primitive {
                let Some(source) = document.feature(vertex.feature_id) else {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency: vertex.feature_id,
                        expected: "solid feature",
                    });
                };
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency: vertex.feature_id,
                        expected: "solid feature",
                    });
                }
            } else if let Primitive::Sketch { plane, .. } = &feature.primitive {
                match plane {
                    SketchPlane::DatumPlane { datum_id } => {
                        let Some(source) = document.feature(*datum_id) else {
                            return Err(DocumentError::InvalidDependency {
                                feature: feature.id,
                                dependency: *datum_id,
                                expected: "datum plane",
                            });
                        };
                        if !matches!(source.primitive, Primitive::DatumPlane { .. }) {
                            return Err(DocumentError::InvalidDependency {
                                feature: feature.id,
                                dependency: *datum_id,
                                expected: "datum plane",
                            });
                        }
                    }
                    SketchPlane::PlanarFace { face } => {
                        let Some(source) = document.feature(face.feature_id) else {
                            return Err(DocumentError::InvalidDependency {
                                feature: feature.id,
                                dependency: face.feature_id,
                                expected: "solid feature",
                            });
                        };
                        if source.primitive.is_reference_geometry() {
                            return Err(DocumentError::InvalidDependency {
                                feature: feature.id,
                                dependency: face.feature_id,
                                expected: "solid feature",
                            });
                        }
                    }
                    SketchPlane::WorldXy | SketchPlane::WorldXz | SketchPlane::WorldYz => {}
                }
            } else if let Primitive::LoftFromSketches { sketch_ids, .. } = &feature.primitive {
                for dependency in sketch_ids {
                    let Some(source) = document.feature(*dependency) else {
                        return Err(DocumentError::InvalidDependency {
                            feature: feature.id,
                            dependency: *dependency,
                            expected: "sketch",
                        });
                    };
                    if !matches!(source.primitive, Primitive::Sketch { .. }) {
                        return Err(DocumentError::InvalidDependency {
                            feature: feature.id,
                            dependency: *dependency,
                            expected: "sketch",
                        });
                    }
                }
            } else if let Some(dependency) = feature.primitive.source_sketch() {
                let Some(source) = document.feature(dependency) else {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency,
                        expected: "sketch",
                    });
                };
                if !matches!(source.primitive, Primitive::Sketch { .. }) {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency,
                        expected: "sketch",
                    });
                }
            }
            for dependency in &feature_dependencies {
                if !index_by_id.contains_key(dependency) {
                    return Err(DocumentError::InvalidDependency {
                        feature: feature.id,
                        dependency: *dependency,
                        expected: "feature",
                    });
                }
                dependents.entry(*dependency).or_default().push(feature.id);
            }
            indegree.insert(feature.id, feature_dependencies.len());
            dependencies.insert(feature.id, feature_dependencies);
        }

        let mut ready = BTreeSet::new();
        for feature in &document.features {
            if indegree.get(&feature.id) == Some(&0) {
                ready.insert((index_by_id[&feature.id], feature.id));
            }
        }
        let mut order = Vec::with_capacity(document.features.len());
        while let Some((_, id)) = ready.pop_first() {
            order.push(id);
            if let Some(children) = dependents.get(&id) {
                for child in children {
                    let degree = indegree
                        .get_mut(child)
                        .expect("feature graph contains every feature id");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert((index_by_id[child], *child));
                    }
                }
            }
        }
        if order.len() != document.features.len() {
            let cycle = document
                .features
                .iter()
                .filter_map(|feature| (indegree[&feature.id] > 0).then_some(feature.id))
                .collect();
            return Err(DocumentError::DependencyCycle { cycle });
        }
        Ok(Self {
            order,
            dependencies,
            dependents,
        })
    }

    #[must_use]
    pub fn order(&self) -> &[FeatureId] {
        &self.order
    }

    #[must_use]
    pub fn dependencies(&self, id: FeatureId) -> Option<&[FeatureId]> {
        self.dependencies.get(&id).map(Vec::as_slice)
    }

    #[must_use]
    pub fn dependents(&self, id: FeatureId) -> &[FeatureId] {
        self.dependents.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Returns all downstream features in deterministic graph order.
    #[must_use]
    pub fn transitive_dependents(&self, id: FeatureId) -> Vec<FeatureId> {
        self.order
            .iter()
            .copied()
            .filter(|candidate| *candidate != id && self.depends_on(*candidate, id))
            .collect()
    }

    fn depends_on(&self, candidate: FeatureId, target: FeatureId) -> bool {
        let mut pending = self.dependencies(candidate).unwrap_or_default().to_vec();
        let mut visited = std::collections::HashSet::new();
        while let Some(id) = pending.pop() {
            if id == target {
                return true;
            }
            if visited.insert(id) {
                pending.extend(self.dependencies(id).unwrap_or_default());
            }
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadDocument {
    pub name: String,
    pub features: Vec<Feature>,
    #[serde(default)]
    pub assemblies: Vec<Assembly>,
    next_id: FeatureId,
    #[serde(default)]
    next_assembly_id: AssemblyId,
}

impl Default for CadDocument {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            features: Vec::new(),
            assemblies: Vec::new(),
            next_id: 1,
            next_assembly_id: 1,
        }
    }
}

impl CadDocument {
    #[must_use]
    pub fn demo() -> Self {
        let mut document = Self::default();
        let _ = document.apply(ModelCommand::CreateBox {
            name: "Base plate".into(),
            size: [48.0, 32.0, 6.0],
            position: [-24.0, -16.0, 0.0],
        });
        let _ = document.apply(ModelCommand::CreateCylinder {
            name: "Boss".into(),
            radius: 8.0,
            height: 18.0,
            position: [0.0, 0.0, 6.0],
        });
        document
    }

    #[must_use]
    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|feature| feature.id == id)
    }

    #[must_use]
    pub fn assembly(&self, id: AssemblyId) -> Option<&Assembly> {
        self.assemblies.iter().find(|assembly| assembly.id == id)
    }

    #[must_use]
    pub fn assembly_occurrence_for_feature(
        &self,
        feature_id: FeatureId,
    ) -> Option<(&Assembly, &ComponentOccurrence)> {
        self.assemblies.iter().find_map(|assembly| {
            assembly
                .occurrences
                .iter()
                .find(|occurrence| occurrence.feature_ids.contains(&feature_id))
                .map(|occurrence| (assembly, occurrence))
        })
    }

    /// Returns stable assembly and ordered body-slot identity for one feature.
    #[must_use]
    pub fn assembly_feature_instance(
        &self,
        feature_id: FeatureId,
    ) -> Option<crate::assembly::AssemblyFeatureInstance> {
        self.assemblies.iter().find_map(|assembly| {
            assembly.occurrences.iter().find_map(|occurrence| {
                occurrence
                    .feature_ids
                    .iter()
                    .position(|candidate| *candidate == feature_id)
                    .map(|body_slot| crate::assembly::AssemblyFeatureInstance {
                        assembly_id: assembly.id,
                        definition_id: occurrence.definition_id,
                        occurrence_id: occurrence.id,
                        body_slot,
                    })
            })
        })
    }

    /// Builds a feature-id lookup for every materialized assembly body.
    #[must_use]
    pub fn assembly_feature_instances(
        &self,
    ) -> HashMap<FeatureId, crate::assembly::AssemblyFeatureInstance> {
        self.assemblies
            .iter()
            .flat_map(|assembly| {
                assembly.occurrences.iter().flat_map(move |occurrence| {
                    occurrence
                        .feature_ids
                        .iter()
                        .enumerate()
                        .map(move |(body_slot, feature_id)| {
                            (
                                *feature_id,
                                crate::assembly::AssemblyFeatureInstance {
                                    assembly_id: assembly.id,
                                    definition_id: occurrence.definition_id,
                                    occurrence_id: occurrence.id,
                                    body_slot,
                                },
                            )
                        })
                })
            })
            .collect()
    }

    /// Returns every materialized body excluded by direct or inherited
    /// occurrence suppression.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError`] if a persisted occurrence hierarchy cannot be
    /// resolved.
    pub fn suppressed_assembly_feature_ids(&self) -> Result<HashSet<FeatureId>, DocumentError> {
        suppressed_assembly_feature_ids(&self.assemblies).map_err(Into::into)
    }

    #[must_use]
    pub const fn next_feature_id(&self) -> FeatureId {
        self.next_id
    }

    /// Builds and validates the document's dependency graph.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError`] when a dependency is missing, incompatible,
    /// or part of a cycle.
    pub fn feature_graph(&self) -> Result<FeatureGraph, DocumentError> {
        FeatureGraph::build(self)
    }

    pub fn dependents(&self, id: FeatureId) -> impl Iterator<Item = &Feature> {
        self.features
            .iter()
            .filter(move |feature| feature.primitive.dependencies().contains(&id))
    }

    /// Applies one validated command to this document.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError`] when a referenced feature is absent, a
    /// parameter is invalid, or the feature id space is exhausted.
    pub fn apply(&mut self, command: ModelCommand) -> Result<Option<FeatureId>, DocumentError> {
        match command {
            ModelCommand::CreateBox {
                name,
                size,
                position,
            } => self.create(
                &name,
                Primitive::Box {
                    size: Vec3::from_array(size),
                },
                Vec3::from_array(position),
            ),
            ModelCommand::CreateCylinder {
                name,
                radius,
                height,
                position,
            } => self.create(
                &name,
                Primitive::Cylinder { radius, height },
                Vec3::from_array(position),
            ),
            ModelCommand::CreateSphere {
                name,
                radius,
                position,
            } => self.create(
                &name,
                Primitive::Sphere { radius },
                Vec3::from_array(position),
            ),
            ModelCommand::CreateCone {
                name,
                bottom_radius,
                top_radius,
                height,
                position,
            } => self.create(
                &name,
                Primitive::Cone {
                    bottom_radius,
                    top_radius,
                    height,
                },
                Vec3::from_array(position),
            ),
            ModelCommand::CreateTorus {
                name,
                major_radius,
                minor_radius,
                position,
            } => self.create(
                &name,
                Primitive::Torus {
                    major_radius,
                    minor_radius,
                },
                Vec3::from_array(position),
            ),
            ModelCommand::CreateExtrusion {
                name,
                profile,
                height,
                position,
            } => self.create(
                &name,
                Primitive::Extrusion { profile, height },
                Vec3::from_array(position),
            ),
            ModelCommand::CreateSketch {
                name,
                plane,
                profile,
                holes,
                constraints,
                position,
            } => {
                self.validate_sketch_plane_source(&plane)?;
                self.create(
                    &name,
                    Primitive::Sketch {
                        plane,
                        region: SketchRegion2D::from_polygons(profile, holes),
                        construction: Vec::new(),
                        constraints,
                    },
                    Vec3::from_array(position),
                )
            }
            ModelCommand::CreateSketchRegion {
                name,
                plane,
                region,
                construction,
                constraints,
                position,
            } => {
                self.validate_sketch_plane_source(&plane)?;
                self.create(
                    &name,
                    Primitive::Sketch {
                        plane,
                        region,
                        construction,
                        constraints,
                    },
                    Vec3::from_array(position),
                )
            }
            ModelCommand::CreateExtrusionFromSketch {
                name,
                sketch_id,
                height,
                position,
            } => {
                let sketch = self
                    .feature(sketch_id)
                    .ok_or(DocumentError::FeatureNotFound(sketch_id))?;
                let Primitive::Sketch {
                    region,
                    construction,
                    constraints,
                    ..
                } = &sketch.primitive
                else {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: sketch_id,
                        expected: "sketch",
                    });
                };
                let solved_region = solve_sketch_region(region, construction, constraints)?;
                self.create(
                    &name,
                    Primitive::ExtrusionFromSketch {
                        sketch_id,
                        region: solved_region,
                        height,
                    },
                    Vec3::from_array(position),
                )
            }
            ModelCommand::CreateRevolveFromSketch {
                name,
                sketch_id,
                axis_origin,
                axis_direction,
                angle,
                position,
            } => {
                let sketch = self
                    .feature(sketch_id)
                    .ok_or(DocumentError::FeatureNotFound(sketch_id))?;
                let Primitive::Sketch {
                    region,
                    construction,
                    constraints,
                    ..
                } = &sketch.primitive
                else {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: sketch_id,
                        expected: "sketch",
                    });
                };
                if !region.holes.is_empty() {
                    return Err(DocumentError::InvalidParameter(
                        "revolve does not support sketch hole loops; use an extrusion or a separate boolean workflow"
                            .into(),
                    ));
                }
                let solved_region = solve_sketch_region(region, construction, constraints)?;
                self.create(
                    &name,
                    Primitive::RevolveFromSketch {
                        sketch_id,
                        profile: solved_region.profile,
                        axis_origin,
                        axis_direction,
                        angle,
                    },
                    Vec3::from_array(position),
                )
            }
            ModelCommand::CreateLoftFromSketches {
                name,
                sketch_ids,
                position,
            } => {
                let mut profiles = Vec::with_capacity(sketch_ids.len());
                for sketch_id in &sketch_ids {
                    let sketch = self
                        .feature(*sketch_id)
                        .ok_or(DocumentError::FeatureNotFound(*sketch_id))?;
                    let Primitive::Sketch {
                        region,
                        construction,
                        constraints,
                        ..
                    } = &sketch.primitive
                    else {
                        return Err(DocumentError::PrimitiveMismatch {
                            id: *sketch_id,
                            expected: "sketch",
                        });
                    };
                    if !region.holes.is_empty() {
                        return Err(DocumentError::InvalidParameter(format!(
                            "loft source sketch {sketch_id} has hole loops; ruled loft currently requires one outer loop per section"
                        )));
                    }
                    profiles.push(solve_sketch_region(region, construction, constraints)?.profile);
                }
                self.create(
                    &name,
                    Primitive::LoftFromSketches {
                        sketch_ids,
                        profiles,
                    },
                    Vec3::from_array(position),
                )
            }
            ModelCommand::ImportStep {
                name,
                source,
                data_section,
                shell_id,
                void_shells,
                length_unit,
                color,
                position,
            } => {
                if color.is_some_and(|color| !color_is_valid(&color)) {
                    return Err(DocumentError::InvalidParameter(
                        "color must contain finite values between zero and one".into(),
                    ));
                }
                let created = self.create(
                    &name,
                    Primitive::ImportedStep {
                        source,
                        data_section,
                        shell_id,
                        void_shells,
                        length_unit,
                    },
                    Vec3::from_array(position),
                )?;
                if let (Some(id), Some(color)) = (created, color) {
                    self.feature_mut(id)?.color = color;
                }
                Ok(created)
            }
            ModelCommand::CreateBoolean {
                name,
                operation,
                left,
                right,
            } => {
                if left == right {
                    return Err(DocumentError::InvalidParameter(
                        "boolean operands must be distinct features".into(),
                    ));
                }
                for id in [left, right] {
                    let feature = self.feature(id).ok_or(DocumentError::FeatureNotFound(id))?;
                    if feature.primitive.is_reference_geometry() {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "solid feature",
                        });
                    }
                }
                let created = self.create(
                    &name,
                    Primitive::Boolean {
                        operation,
                        left,
                        right,
                    },
                    Vec3::ZERO,
                )?;
                self.feature_mut(left)?.visible = false;
                self.feature_mut(right)?.visible = false;
                Ok(created)
            }
            ModelCommand::CreateChamfer {
                name,
                mut edges,
                distance,
            } => {
                canonicalize_edge_refs(&mut edges)?;
                let source_id = edges[0].feature_id;
                let source = self
                    .feature(source_id)
                    .ok_or(DocumentError::FeatureNotFound(source_id))?;
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: source_id,
                        expected: "solid feature",
                    });
                }
                let created =
                    self.create(&name, Primitive::Chamfer { edges, distance }, Vec3::ZERO)?;
                self.feature_mut(source_id)?.visible = false;
                Ok(created)
            }
            ModelCommand::CreateFillet {
                name,
                mut edges,
                radius,
            } => {
                canonicalize_edge_refs(&mut edges)?;
                let source_id = edges[0].feature_id;
                let source = self
                    .feature(source_id)
                    .ok_or(DocumentError::FeatureNotFound(source_id))?;
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: source_id,
                        expected: "solid feature",
                    });
                }
                let created =
                    self.create(&name, Primitive::Fillet { edges, radius }, Vec3::ZERO)?;
                self.feature_mut(source_id)?.visible = false;
                Ok(created)
            }
            ModelCommand::CreateDatumPlane { name, face, offset } => {
                let source = self
                    .feature(face.feature_id)
                    .ok_or(DocumentError::FeatureNotFound(face.feature_id))?;
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: face.feature_id,
                        expected: "solid feature",
                    });
                }
                self.create(&name, Primitive::DatumPlane { face, offset }, Vec3::ZERO)
            }
            ModelCommand::CreateDatumPoint {
                name,
                vertex,
                offset,
            } => {
                let source = self
                    .feature(vertex.feature_id)
                    .ok_or(DocumentError::FeatureNotFound(vertex.feature_id))?;
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: vertex.feature_id,
                        expected: "solid feature",
                    });
                }
                self.create(
                    &name,
                    Primitive::DatumPoint {
                        vertex,
                        offset: Vec3::from_array(offset),
                    },
                    Vec3::ZERO,
                )
            }
            ModelCommand::Duplicate { id, name, position } => {
                self.duplicate(id, &name, Vec3::from_array(position))
            }
            ModelCommand::Move { id, position } => {
                self.ensure_feature_transform_is_editable(id)?;
                let position = Vec3::from_array(position);
                if !position.is_finite() {
                    return Err(DocumentError::InvalidParameter(
                        "translation must contain finite values".into(),
                    ));
                }
                self.feature_mut(id)?.translation = position;
                Ok(None)
            }
            ModelCommand::Rotate { id, rotation } => {
                self.ensure_feature_transform_is_editable(id)?;
                let rotation = Vec3::from_array(rotation);
                if !rotation.is_finite() {
                    return Err(DocumentError::InvalidParameter(
                        "rotation must contain finite values".into(),
                    ));
                }
                if matches!(
                    self.feature(id).map(|feature| &feature.primitive),
                    Some(Primitive::Sketch { .. })
                ) && (rotation.x.abs() > f64::EPSILON || rotation.y.abs() > f64::EPSILON)
                {
                    return Err(DocumentError::InvalidParameter(
                        "sketch rotation is limited to the local plane normal (Z)".into(),
                    ));
                }
                self.feature_mut(id)?.rotation = rotation;
                Ok(None)
            }
            ModelCommand::ResizeBox { id, size } => {
                let primitive = Primitive::Box {
                    size: Vec3::from_array(size),
                };
                primitive.validate()?;
                let feature = self.feature_mut(id)?;
                if !matches!(feature.primitive, Primitive::Box { .. }) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "box",
                    });
                }
                feature.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeCylinder { id, radius, height } => {
                let primitive = Primitive::Cylinder { radius, height };
                primitive.validate()?;
                let feature = self.feature_mut(id)?;
                if !matches!(feature.primitive, Primitive::Cylinder { .. }) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "cylinder",
                    });
                }
                feature.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeSphere { id, radius } => {
                let primitive = Primitive::Sphere { radius };
                primitive.validate()?;
                let feature = self.feature_mut(id)?;
                if !matches!(feature.primitive, Primitive::Sphere { .. }) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "sphere",
                    });
                }
                feature.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeCone {
                id,
                bottom_radius,
                top_radius,
                height,
            } => {
                let primitive = Primitive::Cone {
                    bottom_radius,
                    top_radius,
                    height,
                };
                primitive.validate()?;
                let feature = self.feature_mut(id)?;
                if !matches!(feature.primitive, Primitive::Cone { .. }) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "cone",
                    });
                }
                feature.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeTorus {
                id,
                major_radius,
                minor_radius,
            } => {
                let primitive = Primitive::Torus {
                    major_radius,
                    minor_radius,
                };
                primitive.validate()?;
                let feature = self.feature_mut(id)?;
                if !matches!(feature.primitive, Primitive::Torus { .. }) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "torus",
                    });
                }
                feature.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeExtrusion {
                id,
                profile,
                height,
            } => {
                let linked_sketch = match &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                {
                    Primitive::Extrusion { .. } => None,
                    Primitive::ExtrusionFromSketch {
                        sketch_id, region, ..
                    } => Some((*sketch_id, region.clone())),
                    _ => {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "extrusion",
                        });
                    }
                };
                let primitive = match linked_sketch {
                    Some((sketch_id, region)) => Primitive::ExtrusionFromSketch {
                        sketch_id,
                        region,
                        height,
                    },
                    None => Primitive::Extrusion { profile, height },
                };
                primitive.validate()?;
                let feature = self.feature_mut(id)?;
                if !matches!(
                    feature.primitive,
                    Primitive::Extrusion { .. } | Primitive::ExtrusionFromSketch { .. }
                ) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "extrusion",
                    });
                }
                feature.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeRevolve {
                id,
                axis_origin,
                axis_direction,
                angle,
            } => {
                let Primitive::RevolveFromSketch { sketch_id, .. } = self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                else {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "revolve",
                    });
                };
                let sketch = self
                    .feature(sketch_id)
                    .ok_or(DocumentError::FeatureNotFound(sketch_id))?;
                let Primitive::Sketch {
                    region,
                    construction,
                    constraints,
                    ..
                } = &sketch.primitive
                else {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: sketch_id,
                        expected: "sketch",
                    });
                };
                if !region.holes.is_empty() {
                    return Err(DocumentError::InvalidParameter(
                        "revolve does not support sketch hole loops".into(),
                    ));
                }
                let primitive = Primitive::RevolveFromSketch {
                    sketch_id,
                    profile: solve_sketch_region(region, construction, constraints)?.profile,
                    axis_origin,
                    axis_direction,
                    angle,
                };
                primitive.validate()?;
                self.feature_mut(id)?.primitive = primitive;
                Ok(None)
            }
            ModelCommand::ResizeSketch { id, profile } => {
                let (plane, mut region, construction, constraints) = match &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                {
                    Primitive::Sketch {
                        plane,
                        region,
                        construction,
                        constraints,
                        ..
                    } => (
                        plane.clone(),
                        region.clone(),
                        construction.clone(),
                        constraints.clone(),
                    ),
                    _ => {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "sketch",
                        });
                    }
                };
                region.profile = SketchLoop2D::from_polygon(profile);
                let primitive = Primitive::Sketch {
                    plane,
                    region,
                    construction,
                    constraints,
                };
                self.replace_sketch_and_sync(id, primitive)?;
                Ok(None)
            }
            ModelCommand::SetSketchHoles { id, holes } => {
                if self.features.iter().any(|feature| {
                    matches!(
                        feature.primitive,
                        Primitive::RevolveFromSketch { sketch_id, .. } if sketch_id == id
                    )
                }) && !holes.is_empty()
                {
                    return Err(DocumentError::InvalidParameter(
                        "a sketch with revolve dependents cannot contain hole loops".into(),
                    ));
                }
                let (plane, mut region, construction, constraints) = match &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                {
                    Primitive::Sketch {
                        plane,
                        region,
                        construction,
                        constraints,
                        ..
                    } => (
                        plane.clone(),
                        region.clone(),
                        construction.clone(),
                        constraints.clone(),
                    ),
                    _ => {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "sketch",
                        });
                    }
                };
                region.holes = holes.into_iter().map(SketchLoop2D::from_polygon).collect();
                let primitive = Primitive::Sketch {
                    plane,
                    region,
                    construction,
                    constraints,
                };
                self.replace_sketch_and_sync(id, primitive)?;
                Ok(None)
            }
            ModelCommand::SetSketchRegion { id, region } => {
                if self.features.iter().any(|feature| {
                    matches!(
                        feature.primitive,
                        Primitive::RevolveFromSketch { sketch_id, .. } if sketch_id == id
                    )
                }) && !region.holes.is_empty()
                {
                    return Err(DocumentError::InvalidParameter(
                        "a sketch with revolve dependents cannot contain hole loops".into(),
                    ));
                }
                let (plane, construction, constraints) = match &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                {
                    Primitive::Sketch {
                        plane,
                        construction,
                        constraints,
                        ..
                    } => (plane.clone(), construction.clone(), constraints.clone()),
                    _ => {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "sketch",
                        });
                    }
                };
                let primitive = Primitive::Sketch {
                    plane,
                    region,
                    construction,
                    constraints,
                };
                self.replace_sketch_and_sync(id, primitive)?;
                Ok(None)
            }
            ModelCommand::SetSketchDefinition {
                id,
                region,
                construction,
                constraints,
            } => {
                if self.features.iter().any(|feature| {
                    matches!(
                        feature.primitive,
                        Primitive::RevolveFromSketch { sketch_id, .. } if sketch_id == id
                    )
                }) && !region.holes.is_empty()
                {
                    return Err(DocumentError::InvalidParameter(
                        "a sketch with revolve dependents cannot contain hole loops".into(),
                    ));
                }
                let plane = match &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                {
                    Primitive::Sketch { plane, .. } => plane.clone(),
                    _ => {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "sketch",
                        });
                    }
                };
                self.replace_sketch_and_sync(
                    id,
                    Primitive::Sketch {
                        plane,
                        region,
                        construction,
                        constraints,
                    },
                )?;
                Ok(None)
            }
            ModelCommand::SetSketchConstraints { id, constraints } => {
                let (plane, region, construction) = match &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                {
                    Primitive::Sketch {
                        plane,
                        region,
                        construction,
                        ..
                    } => (plane.clone(), region.clone(), construction.clone()),
                    _ => {
                        return Err(DocumentError::PrimitiveMismatch {
                            id,
                            expected: "sketch",
                        });
                    }
                };
                let primitive = Primitive::Sketch {
                    plane,
                    region,
                    construction,
                    constraints,
                };
                self.replace_sketch_and_sync(id, primitive)?;
                Ok(None)
            }
            ModelCommand::SetSketchPlane { id, plane } => {
                self.validate_sketch_plane_source(&plane)?;
                let Primitive::Sketch {
                    plane: previous, ..
                } = &self
                    .feature(id)
                    .ok_or(DocumentError::FeatureNotFound(id))?
                    .primitive
                else {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "sketch",
                    });
                };
                let previous = previous.clone();
                if let Primitive::Sketch { plane: current, .. } =
                    &mut self.feature_mut(id)?.primitive
                {
                    *current = plane;
                }
                if let Err(error) = self.feature_graph() {
                    if let Primitive::Sketch { plane: current, .. } =
                        &mut self.feature_mut(id)?.primitive
                    {
                        *current = previous;
                    }
                    return Err(error);
                }
                Ok(None)
            }
            ModelCommand::SetDatumPlaneOffset { id, offset } => {
                if !offset.is_finite() {
                    return Err(DocumentError::InvalidParameter(
                        "datum plane offset must be finite".into(),
                    ));
                }
                let feature = self.feature_mut(id)?;
                if let Primitive::DatumPlane {
                    offset: current, ..
                } = &mut feature.primitive
                {
                    *current = offset;
                    Ok(None)
                } else {
                    Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "datum plane",
                    })
                }
            }
            ModelCommand::SetDatumPointOffset { id, offset } => {
                let offset = Vec3::from_array(offset);
                if !offset.is_finite() {
                    return Err(DocumentError::InvalidParameter(
                        "datum point offset must contain finite values".into(),
                    ));
                }
                let feature = self.feature_mut(id)?;
                if let Primitive::DatumPoint {
                    offset: current, ..
                } = &mut feature.primitive
                {
                    *current = offset;
                    Ok(None)
                } else {
                    Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "datum point",
                    })
                }
            }
            ModelCommand::SetChamferDistance { id, distance } => {
                if !distance.is_finite() || distance <= 0.0 {
                    return Err(DocumentError::InvalidParameter(
                        "chamfer distance must be finite and greater than zero".into(),
                    ));
                }
                let feature = self.feature_mut(id)?;
                if let Primitive::Chamfer {
                    distance: current, ..
                } = &mut feature.primitive
                {
                    *current = distance;
                    Ok(None)
                } else {
                    Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "chamfer",
                    })
                }
            }
            ModelCommand::SetFilletRadius { id, radius } => {
                if !radius.is_finite() || radius <= 0.0 {
                    return Err(DocumentError::InvalidParameter(
                        "fillet radius must be finite and greater than zero".into(),
                    ));
                }
                let feature = self.feature_mut(id)?;
                if let Primitive::Fillet {
                    radius: current, ..
                } = &mut feature.primitive
                {
                    *current = radius;
                    Ok(None)
                } else {
                    Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "fillet",
                    })
                }
            }
            ModelCommand::Rename { id, name } => {
                let name = normalized_name(&name, "Feature");
                self.feature_mut(id)?.name = name;
                Ok(None)
            }
            ModelCommand::SetVisibility { id, visible } => {
                self.feature_mut(id)?.visible = visible;
                Ok(None)
            }
            ModelCommand::SetColor { id, color } => {
                if !color_is_valid(&color) {
                    return Err(DocumentError::InvalidParameter(
                        "color must contain finite values between zero and one".into(),
                    ));
                }
                self.feature_mut(id)?.color = color;
                Ok(None)
            }
            ModelCommand::SetMaterial {
                id,
                name,
                density_kg_m3,
            } => {
                let material = Material::validated(&name, density_kg_m3)?;
                let feature = self.feature_mut(id)?;
                if feature.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "solid feature",
                    });
                }
                feature.material = Some(material);
                Ok(None)
            }
            ModelCommand::ClearMaterial { id } => {
                let feature = self.feature_mut(id)?;
                if feature.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id,
                        expected: "solid feature",
                    });
                }
                feature.material = None;
                Ok(None)
            }
            ModelCommand::CreateAssembly {
                name,
                definitions,
                occurrences,
            } => {
                let id = self.next_assembly_id;
                let next_assembly_id = self
                    .next_assembly_id
                    .checked_add(1)
                    .ok_or(DocumentError::IdOverflow)?;
                let assembly = Assembly {
                    id,
                    name: normalized_name(&name, "Assembly"),
                    definitions,
                    occurrences,
                    mates: Vec::new(),
                };
                self.validate_assembly(&assembly)?;
                let mut candidate_assemblies = self.assemblies.clone();
                candidate_assemblies.push(assembly.clone());
                self.validate_suppressed_dependencies(&candidate_assemblies)?;
                self.next_assembly_id = next_assembly_id;
                self.assemblies.push(assembly);
                Ok(None)
            }
            ModelCommand::SetOccurrenceTransform {
                assembly_id,
                occurrence_id,
                position,
                rotation,
            } => {
                self.set_occurrence_transform(assembly_id, occurrence_id, position, rotation)?;
                Ok(None)
            }
            ModelCommand::CreateAssemblyMate { assembly_id, mate } => {
                self.create_assembly_mate(assembly_id, mate)?;
                Ok(None)
            }
            ModelCommand::SetAssemblyMateState {
                assembly_id,
                mate_id,
                state,
            } => {
                self.set_assembly_mate_state(assembly_id, mate_id, state)?;
                Ok(None)
            }
            ModelCommand::DeleteAssemblyMate {
                assembly_id,
                mate_id,
            } => {
                self.delete_assembly_mate(assembly_id, mate_id)?;
                Ok(None)
            }
            ModelCommand::SetOccurrenceSuppressed {
                assembly_id,
                occurrence_id,
                suppressed,
            } => {
                self.set_occurrence_suppressed(assembly_id, occurrence_id, suppressed)?;
                Ok(None)
            }
            ModelCommand::DeleteAssembly { id } => {
                let index = self
                    .assemblies
                    .iter()
                    .position(|assembly| assembly.id == id)
                    .ok_or(DocumentError::AssemblyNotFound(id))?;
                self.assemblies.remove(index);
                Ok(None)
            }
            ModelCommand::Delete { id } => {
                if let Some(dependent) = self.dependents(id).next() {
                    return Err(DocumentError::FeatureInUse {
                        id,
                        dependent: dependent.id,
                    });
                }
                if let Some((assembly, occurrence)) = self.assemblies.iter().find_map(|assembly| {
                    assembly.occurrences.iter().find_map(|occurrence| {
                        occurrence
                            .feature_ids
                            .contains(&id)
                            .then_some((assembly.id, occurrence.id))
                    })
                }) {
                    return Err(DocumentError::FeatureInAssembly {
                        id,
                        assembly,
                        occurrence,
                    });
                }
                let index = self
                    .features
                    .iter()
                    .position(|feature| feature.id == id)
                    .ok_or(DocumentError::FeatureNotFound(id))?;
                self.features.remove(index);
                Ok(None)
            }
        }
    }

    /// Validates and applies a batch as a single atomic document edit.
    ///
    /// # Errors
    ///
    /// Returns the first [`DocumentError`] produced by a command. The original
    /// document is unchanged when any command fails.
    pub fn apply_transaction(
        &mut self,
        commands: impl IntoIterator<Item = ModelCommand>,
    ) -> Result<Vec<FeatureId>, DocumentError> {
        let mut staged = self.clone();
        let mut created = Vec::new();
        for command in commands {
            if let Some(id) = staged.apply(command)? {
                created.push(id);
            }
        }
        *self = staged;
        Ok(created)
    }

    /// Validates all persisted invariants and restores the id allocator.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError`] for duplicate ids, invalid geometry, or
    /// non-finite transforms and colors.
    pub fn validate_and_repair(&mut self) -> Result<(), DocumentError> {
        let mut ids = std::collections::HashSet::with_capacity(self.features.len());
        let mut maximum_id = 0;
        for feature in &self.features {
            if feature.id == 0 || !ids.insert(feature.id) {
                return Err(DocumentError::InvalidFeatureId(feature.id));
            }
            feature.primitive.validate()?;
            if !feature.translation.is_finite() {
                return Err(DocumentError::InvalidParameter(format!(
                    "feature {} translation must contain finite values",
                    feature.id
                )));
            }
            if !feature.rotation.is_finite() {
                return Err(DocumentError::InvalidParameter(format!(
                    "feature {} rotation must contain finite values",
                    feature.id
                )));
            }
            if matches!(feature.primitive, Primitive::Sketch { .. })
                && (feature.rotation.x.abs() > f64::EPSILON
                    || feature.rotation.y.abs() > f64::EPSILON)
            {
                return Err(DocumentError::InvalidParameter(format!(
                    "sketch {} rotation is limited to the local plane normal (Z)",
                    feature.id
                )));
            }
            if !color_is_valid(&feature.color) {
                return Err(DocumentError::InvalidParameter(format!(
                    "feature {} color must contain finite values between zero and one",
                    feature.id
                )));
            }
            if let Some(material) = &feature.material {
                if feature.primitive.is_reference_geometry() {
                    return Err(DocumentError::InvalidParameter(format!(
                        "feature {} cannot assign material to reference geometry",
                        feature.id
                    )));
                }
                material.validate()?;
            }
            maximum_id = maximum_id.max(feature.id);
        }
        self.feature_graph()?;
        let mut assembly_ids = std::collections::BTreeSet::new();
        let mut maximum_assembly_id = 0;
        let available_features = self.features.iter().map(|feature| feature.id).collect();
        let mut owned_features = std::collections::BTreeMap::new();
        for assembly in &self.assemblies {
            if !assembly_ids.insert(assembly.id) {
                return Err(DocumentError::Assembly(AssemblyError::InvalidAssemblyId(
                    assembly.id,
                )));
            }
            assembly.validate(&available_features)?;
            let world_transforms = assembly.world_transforms()?;
            for occurrence in &assembly.occurrences {
                let world_transform = world_transforms
                    .get(&occurrence.id)
                    .copied()
                    .ok_or(AssemblyError::UnresolvableOccurrenceHierarchy)?;
                for feature_id in &occurrence.feature_ids {
                    let feature =
                        self.feature(*feature_id)
                            .ok_or(AssemblyError::MissingFeature {
                                occurrence: occurrence.id,
                                feature: *feature_id,
                            })?;
                    if feature.primitive.is_reference_geometry() {
                        return Err(DocumentError::Assembly(AssemblyError::NonSolidFeature {
                            occurrence: occurrence.id,
                            feature: *feature_id,
                        }));
                    }
                    let feature_transform = AssemblyTransform::from_euler_xyz_degrees(
                        feature.translation.as_array(),
                        feature.rotation.as_array(),
                    );
                    if !feature_transform.approximately_equals(world_transform, 1.0e-8) {
                        return Err(DocumentError::Assembly(
                            AssemblyError::FeatureTransformMismatch {
                                occurrence: occurrence.id,
                                feature: *feature_id,
                            },
                        ));
                    }
                    if let Some((owner_assembly, owner_occurrence)) =
                        owned_features.insert(*feature_id, (assembly.id, occurrence.id))
                    {
                        return Err(DocumentError::FeatureInMultipleAssemblies {
                            id: *feature_id,
                            first_assembly: owner_assembly,
                            first_occurrence: owner_occurrence,
                            second_assembly: assembly.id,
                            second_occurrence: occurrence.id,
                        });
                    }
                }
            }
            maximum_assembly_id = maximum_assembly_id.max(assembly.id);
        }
        self.validate_suppressed_dependencies(&self.assemblies)?;
        self.next_id = maximum_id.checked_add(1).ok_or(DocumentError::IdOverflow)?;
        self.next_assembly_id = maximum_assembly_id
            .checked_add(1)
            .ok_or(DocumentError::IdOverflow)?;
        self.name = normalized_name(&self.name, "Untitled");
        Ok(())
    }

    fn validate_assembly(&self, assembly: &Assembly) -> Result<(), DocumentError> {
        let available_features = self.features.iter().map(|feature| feature.id).collect();
        assembly.validate(&available_features)?;
        let world_transforms = assembly.world_transforms()?;
        for occurrence in &assembly.occurrences {
            let world_transform = world_transforms
                .get(&occurrence.id)
                .copied()
                .ok_or(AssemblyError::UnresolvableOccurrenceHierarchy)?;
            for feature_id in &occurrence.feature_ids {
                let feature = self
                    .feature(*feature_id)
                    .ok_or(AssemblyError::MissingFeature {
                        occurrence: occurrence.id,
                        feature: *feature_id,
                    })?;
                if feature.primitive.is_reference_geometry() {
                    return Err(DocumentError::Assembly(AssemblyError::NonSolidFeature {
                        occurrence: occurrence.id,
                        feature: *feature_id,
                    }));
                }
                let feature_transform = AssemblyTransform::from_euler_xyz_degrees(
                    feature.translation.as_array(),
                    feature.rotation.as_array(),
                );
                if !feature_transform.approximately_equals(world_transform, 1.0e-8) {
                    return Err(DocumentError::Assembly(
                        AssemblyError::FeatureTransformMismatch {
                            occurrence: occurrence.id,
                            feature: *feature_id,
                        },
                    ));
                }
                if let Some((owner_assembly, owner_occurrence)) =
                    self.assemblies.iter().find_map(|existing| {
                        existing.occurrences.iter().find_map(|existing_occurrence| {
                            existing_occurrence
                                .feature_ids
                                .contains(feature_id)
                                .then_some((existing.id, existing_occurrence.id))
                        })
                    })
                {
                    return Err(DocumentError::FeatureInMultipleAssemblies {
                        id: *feature_id,
                        first_assembly: owner_assembly,
                        first_occurrence: owner_occurrence,
                        second_assembly: assembly.id,
                        second_occurrence: occurrence.id,
                    });
                }
            }
        }
        Ok(())
    }

    fn set_occurrence_transform(
        &mut self,
        assembly_id: AssemblyId,
        occurrence_id: u64,
        position: [f64; 3],
        rotation: [f64; 3],
    ) -> Result<(), DocumentError> {
        let assembly_index = self
            .assemblies
            .iter()
            .position(|assembly| assembly.id == assembly_id)
            .ok_or(DocumentError::AssemblyNotFound(assembly_id))?;
        let mut assembly = self.assemblies[assembly_index].clone();
        if let Some(mate) = assembly.mate_for_child(occurrence_id) {
            return Err(DocumentError::MateDrivenOccurrence {
                assembly: assembly_id,
                occurrence: occurrence_id,
                mate: mate.id,
            });
        }
        let occurrence = assembly
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.id == occurrence_id)
            .ok_or(DocumentError::OccurrenceNotFound {
                assembly: assembly_id,
                occurrence: occurrence_id,
            })?;
        occurrence.transform = AssemblyTransform::from_euler_xyz_degrees(position, rotation);

        self.replace_assembly_with_feature_transforms(assembly_index, assembly)
    }

    fn create_assembly_mate(
        &mut self,
        assembly_id: AssemblyId,
        mate: AssemblyMate,
    ) -> Result<(), DocumentError> {
        let assembly_index = self
            .assemblies
            .iter()
            .position(|assembly| assembly.id == assembly_id)
            .ok_or(DocumentError::AssemblyNotFound(assembly_id))?;
        let mut assembly = self.assemblies[assembly_index].clone();
        let child_id = mate.child_occurrence_id;
        let local_transform = mate.local_transform();
        assembly.mates.push(mate);
        if let Some(child) = assembly
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.id == child_id)
        {
            child.transform = local_transform;
        }
        self.replace_assembly_with_feature_transforms(assembly_index, assembly)
    }

    fn set_assembly_mate_state(
        &mut self,
        assembly_id: AssemblyId,
        mate_id: AssemblyMateId,
        state: f64,
    ) -> Result<(), DocumentError> {
        let assembly_index = self
            .assemblies
            .iter()
            .position(|assembly| assembly.id == assembly_id)
            .ok_or(DocumentError::AssemblyNotFound(assembly_id))?;
        let mut assembly = self.assemblies[assembly_index].clone();
        let mate = assembly
            .mates
            .iter_mut()
            .find(|mate| mate.id == mate_id)
            .ok_or(DocumentError::AssemblyMateNotFound {
                assembly: assembly_id,
                mate: mate_id,
            })?;
        mate.state = state;
        let child_id = mate.child_occurrence_id;
        let local_transform = mate.local_transform();
        let child = assembly
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.id == child_id)
            .ok_or(AssemblyError::MateOccurrenceNotFound {
                mate: mate_id,
                occurrence: child_id,
            })?;
        child.transform = local_transform;
        self.replace_assembly_with_feature_transforms(assembly_index, assembly)
    }

    fn delete_assembly_mate(
        &mut self,
        assembly_id: AssemblyId,
        mate_id: AssemblyMateId,
    ) -> Result<(), DocumentError> {
        let assembly_index = self
            .assemblies
            .iter()
            .position(|assembly| assembly.id == assembly_id)
            .ok_or(DocumentError::AssemblyNotFound(assembly_id))?;
        let mut assembly = self.assemblies[assembly_index].clone();
        let mate_index = assembly
            .mates
            .iter()
            .position(|mate| mate.id == mate_id)
            .ok_or(DocumentError::AssemblyMateNotFound {
                assembly: assembly_id,
                mate: mate_id,
            })?;
        assembly.mates.remove(mate_index);
        self.replace_assembly_with_feature_transforms(assembly_index, assembly)
    }

    fn replace_assembly_with_feature_transforms(
        &mut self,
        assembly_index: usize,
        assembly: Assembly,
    ) -> Result<(), DocumentError> {
        let available_features = self.features.iter().map(|feature| feature.id).collect();
        assembly.validate(&available_features)?;
        let world_transforms = assembly.world_transforms()?;
        let mut updates = Vec::new();
        for occurrence in &assembly.occurrences {
            let world_transform = world_transforms
                .get(&occurrence.id)
                .copied()
                .ok_or(AssemblyError::UnresolvableOccurrenceHierarchy)?;
            let feature_rotation = world_transform.euler_xyz_degrees();
            let reconstructed = AssemblyTransform::from_euler_xyz_degrees(
                world_transform.translation,
                feature_rotation,
            );
            if !world_transform.approximately_equals(reconstructed, 1.0e-8) {
                return Err(DocumentError::Assembly(
                    AssemblyError::UnrepresentableFeatureTransform {
                        occurrence: occurrence.id,
                    },
                ));
            }
            for feature_id in &occurrence.feature_ids {
                let feature_index = self
                    .features
                    .iter()
                    .position(|feature| feature.id == *feature_id)
                    .ok_or(AssemblyError::MissingFeature {
                        occurrence: occurrence.id,
                        feature: *feature_id,
                    })?;
                if self.features[feature_index]
                    .primitive
                    .is_reference_geometry()
                {
                    return Err(DocumentError::Assembly(AssemblyError::NonSolidFeature {
                        occurrence: occurrence.id,
                        feature: *feature_id,
                    }));
                }
                updates.push((feature_index, world_transform.translation, feature_rotation));
            }
        }

        self.assemblies[assembly_index] = assembly;
        for (feature_index, translation, rotation) in updates {
            self.features[feature_index].translation = Vec3::from_array(translation);
            self.features[feature_index].rotation = Vec3::from_array(rotation);
        }
        Ok(())
    }

    fn set_occurrence_suppressed(
        &mut self,
        assembly_id: AssemblyId,
        occurrence_id: u64,
        suppressed: bool,
    ) -> Result<(), DocumentError> {
        let assembly_index = self
            .assemblies
            .iter()
            .position(|assembly| assembly.id == assembly_id)
            .ok_or(DocumentError::AssemblyNotFound(assembly_id))?;
        let mut candidate_assemblies = self.assemblies.clone();
        let occurrence = candidate_assemblies[assembly_index]
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.id == occurrence_id)
            .ok_or(DocumentError::OccurrenceNotFound {
                assembly: assembly_id,
                occurrence: occurrence_id,
            })?;
        occurrence.suppressed = suppressed;
        self.validate_suppressed_dependencies(&candidate_assemblies)?;
        self.assemblies = candidate_assemblies;
        Ok(())
    }

    fn validate_suppressed_dependencies(
        &self,
        assemblies: &[Assembly],
    ) -> Result<(), DocumentError> {
        let suppressed = suppressed_assembly_feature_ids(assemblies)?;
        for feature in &self.features {
            if suppressed.contains(&feature.id) {
                continue;
            }
            if let Some(dependency) = feature
                .primitive
                .dependencies()
                .into_iter()
                .find(|dependency| suppressed.contains(dependency))
            {
                return Err(DocumentError::SuppressedFeatureDependency {
                    feature: feature.id,
                    dependency,
                });
            }
        }
        Ok(())
    }

    fn validate_primitive_suppressed_dependencies(
        &self,
        feature: FeatureId,
        primitive: &Primitive,
    ) -> Result<(), DocumentError> {
        let suppressed = self.suppressed_assembly_feature_ids()?;
        if let Some(dependency) = primitive
            .dependencies()
            .into_iter()
            .find(|dependency| suppressed.contains(dependency))
        {
            return Err(DocumentError::SuppressedFeatureDependency {
                feature,
                dependency,
            });
        }
        Ok(())
    }

    fn ensure_feature_transform_is_editable(&self, id: FeatureId) -> Result<(), DocumentError> {
        if let Some((assembly, occurrence)) = self.assembly_occurrence_for_feature(id) {
            return Err(DocumentError::FeatureInAssembly {
                id,
                assembly: assembly.id,
                occurrence: occurrence.id,
            });
        }
        Ok(())
    }

    fn validate_sketch_plane_source(&self, plane: &SketchPlane) -> Result<(), DocumentError> {
        match plane {
            SketchPlane::DatumPlane { datum_id } => {
                let source = self
                    .feature(*datum_id)
                    .ok_or(DocumentError::FeatureNotFound(*datum_id))?;
                if !matches!(source.primitive, Primitive::DatumPlane { .. }) {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: *datum_id,
                        expected: "datum plane",
                    });
                }
            }
            SketchPlane::PlanarFace { face } => {
                let source = self
                    .feature(face.feature_id)
                    .ok_or(DocumentError::FeatureNotFound(face.feature_id))?;
                if source.primitive.is_reference_geometry() {
                    return Err(DocumentError::PrimitiveMismatch {
                        id: face.feature_id,
                        expected: "solid feature",
                    });
                }
            }
            SketchPlane::WorldXy | SketchPlane::WorldXz | SketchPlane::WorldYz => {}
        }
        Ok(())
    }

    fn create(
        &mut self,
        name: &str,
        primitive: Primitive,
        translation: Vec3,
    ) -> Result<Option<FeatureId>, DocumentError> {
        primitive.validate()?;
        self.validate_primitive_suppressed_dependencies(self.next_id, &primitive)?;
        if !translation.is_finite() {
            return Err(DocumentError::InvalidParameter(
                "translation must contain finite values".into(),
            ));
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(DocumentError::IdOverflow)?;
        let color = palette_color(id);
        self.features.push(Feature {
            id,
            name: normalized_name(name, primitive.label()),
            primitive,
            translation,
            rotation: Vec3::ZERO,
            visible: true,
            color,
            material: None,
        });
        Ok(Some(id))
    }

    fn duplicate(
        &mut self,
        source_id: FeatureId,
        name: &str,
        translation: Vec3,
    ) -> Result<Option<FeatureId>, DocumentError> {
        if !translation.is_finite() {
            return Err(DocumentError::InvalidParameter(
                "translation must contain finite values".into(),
            ));
        }
        let source = self
            .feature(source_id)
            .cloned()
            .ok_or(DocumentError::FeatureNotFound(source_id))?;
        let new_name = if name.trim().is_empty() {
            format!("{} copy", source.name)
        } else {
            name.to_owned()
        };
        let id = self
            .create(&new_name, source.primitive.clone(), translation)?
            .ok_or(DocumentError::IdOverflow)?;
        let feature = self.feature_mut(id)?;
        feature.rotation = source.rotation;
        feature.visible = source.visible;
        feature.material = source.material;
        Ok(Some(id))
    }

    fn feature_mut(&mut self, id: FeatureId) -> Result<&mut Feature, DocumentError> {
        self.features
            .iter_mut()
            .find(|feature| feature.id == id)
            .ok_or(DocumentError::FeatureNotFound(id))
    }

    fn replace_sketch_and_sync(
        &mut self,
        sketch_id: FeatureId,
        primitive: Primitive,
    ) -> Result<(), DocumentError> {
        let Primitive::Sketch {
            region,
            construction,
            constraints,
            ..
        } = &primitive
        else {
            return Err(DocumentError::PrimitiveMismatch {
                id: sketch_id,
                expected: "sketch",
            });
        };
        primitive.validate()?;
        self.validate_primitive_suppressed_dependencies(sketch_id, &primitive)?;
        let solved_region = solve_sketch_region(region, construction, constraints)?;

        // Validate every derived primitive before changing any feature so a
        // profile edit cannot leave a linked revolve or extrusion invalid.
        for feature in &self.features {
            let candidate = match &feature.primitive {
                Primitive::ExtrusionFromSketch {
                    sketch_id: source,
                    height,
                    ..
                } if *source == sketch_id => Some(Primitive::ExtrusionFromSketch {
                    sketch_id,
                    region: solved_region.clone(),
                    height: *height,
                }),
                Primitive::RevolveFromSketch {
                    sketch_id: source,
                    axis_origin,
                    axis_direction,
                    angle,
                    ..
                } if *source == sketch_id => {
                    if !solved_region.holes.is_empty() {
                        return Err(DocumentError::InvalidParameter(
                            "a sketch with revolve dependents cannot contain hole loops".into(),
                        ));
                    }
                    Some(Primitive::RevolveFromSketch {
                        sketch_id,
                        profile: solved_region.profile.clone(),
                        axis_origin: *axis_origin,
                        axis_direction: *axis_direction,
                        angle: *angle,
                    })
                }
                Primitive::LoftFromSketches {
                    sketch_ids,
                    profiles,
                } if sketch_ids.contains(&sketch_id) => {
                    if !solved_region.holes.is_empty() {
                        return Err(DocumentError::InvalidParameter(
                            "a sketch with loft dependents cannot contain hole loops".into(),
                        ));
                    }
                    let mut updated = profiles.clone();
                    let section = sketch_ids
                        .iter()
                        .position(|source| *source == sketch_id)
                        .ok_or(DocumentError::FeatureNotFound(sketch_id))?;
                    updated[section] = solved_region.profile.clone();
                    Some(Primitive::LoftFromSketches {
                        sketch_ids: sketch_ids.clone(),
                        profiles: updated,
                    })
                }
                _ => None,
            };
            if let Some(candidate) = candidate {
                candidate.validate()?;
            }
        }

        self.feature_mut(sketch_id)?.primitive = primitive;
        for feature in &mut self.features {
            if let Primitive::ExtrusionFromSketch {
                sketch_id: source,
                region: cached,
                ..
            } = &mut feature.primitive
                && *source == sketch_id
            {
                cached.clone_from(&solved_region);
            } else if let Primitive::RevolveFromSketch {
                sketch_id: source,
                profile: cached,
                ..
            } = &mut feature.primitive
                && *source == sketch_id
            {
                cached.clone_from(&solved_region.profile);
            } else if let Primitive::LoftFromSketches {
                sketch_ids,
                profiles,
            } = &mut feature.primitive
                && let Some(section) = sketch_ids.iter().position(|source| *source == sketch_id)
            {
                profiles[section].clone_from(&solved_region.profile);
            }
        }
        Ok(())
    }
}

fn suppressed_assembly_feature_ids(
    assemblies: &[Assembly],
) -> Result<HashSet<FeatureId>, AssemblyError> {
    let mut features = HashSet::new();
    for assembly in assemblies {
        let effective = assembly.effective_suppression()?;
        for occurrence in &assembly.occurrences {
            if effective.get(&occurrence.id) == Some(&true) {
                features.extend(occurrence.feature_ids.iter().copied());
            }
        }
    }
    Ok(features)
}

fn normalized_name(name: &str, fallback: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        fallback.into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn profile_is_valid(profile: &[[f64; 2]]) -> bool {
    if profile.len() < 3 || profile.len() > 128 {
        return false;
    }
    if profile
        .iter()
        .any(|point| !point[0].is_finite() || !point[1].is_finite())
    {
        return false;
    }
    for index in 0..profile.len() {
        let next = (index + 1) % profile.len();
        let dx = profile[index][0] - profile[next][0];
        let dy = profile[index][1] - profile[next][1];
        if dx.mul_add(dx, dy * dy) <= 1.0e-18 {
            return false;
        }
        for other in (index + 1)..profile.len() {
            let other_next = (other + 1) % profile.len();
            if other == index || other == next || other_next == index {
                continue;
            }
            if segments_intersect(
                profile[index],
                profile[next],
                profile[other],
                profile[other_next],
            ) {
                return false;
            }
        }
    }
    let area = profile
        .iter()
        .zip(profile.iter().cycle().skip(1))
        .take(profile.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        .abs();
    area > 1.0e-9
}

fn solve_sketch_region(
    region: &SketchRegion2D,
    construction: &[SketchSegment2D],
    constraints: &[Constraint],
) -> Result<SketchRegion2D, DocumentError> {
    solve_sketch(region, construction, constraints, SolverConfig::default())
        .map(|solved| solved.region)
        .map_err(sketch_document_error)
}

fn sketch_document_error(error: SketchError) -> DocumentError {
    let detail = error.to_string();
    let (reason, constraint_indices, iterations, residual) = match error {
        SketchError::ConstraintConflict {
            iterations,
            residual,
            constraints,
        } => (
            SketchConstraintFailureReason::Conflict,
            constraints,
            iterations,
            residual,
        ),
        SketchError::NotConverged {
            iterations,
            residual,
        } => (
            SketchConstraintFailureReason::NonConvergence,
            Vec::new(),
            iterations,
            residual,
        ),
        _ => {
            return DocumentError::InvalidParameter(format!(
                "sketch definition is invalid: {detail}"
            ));
        }
    };
    DocumentError::SketchConstraint(SketchConstraintDiagnostic {
        reason,
        constraint_indices,
        iterations,
        residual,
        detail,
    })
}

fn revolve_is_valid(
    profile: &SketchLoop2D,
    axis_origin: [f64; 2],
    axis_direction: [f64; 2],
    angle: f64,
) -> bool {
    if !(profile.validate().is_ok()
        && axis_origin.iter().all(|value| value.is_finite())
        && axis_direction.iter().all(|value| value.is_finite())
        && angle.is_finite()
        && 0.0 < angle
        && angle <= 360.0)
    {
        return false;
    }
    profile
        .signed_distance_range_to_line(axis_origin, axis_direction)
        .is_some_and(|[minimum, maximum]| minimum > 1.0e-9 || maximum < -1.0e-9)
}

fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
        (b[0] - a[0]).mul_add(c[1] - a[1], -(b[1] - a[1]) * (c[0] - a[0]))
    }

    const EPSILON: f64 = 1.0e-12;
    let [ab_c, ab_d, cd_a, cd_b] = [
        cross(a, b, c),
        cross(a, b, d),
        cross(c, d, a),
        cross(c, d, b),
    ];
    if (ab_c > EPSILON && ab_d < -EPSILON || ab_c < -EPSILON && ab_d > EPSILON)
        && (cd_a > EPSILON && cd_b < -EPSILON || cd_a < -EPSILON && cd_b > EPSILON)
    {
        return true;
    }
    for (cross, point, start, end) in [
        (ab_c, c, a, b),
        (ab_d, d, a, b),
        (cd_a, a, c, d),
        (cd_b, b, c, d),
    ] {
        if cross.abs() <= EPSILON
            && point[0] >= start[0].min(end[0]) - EPSILON
            && point[0] <= start[0].max(end[0]) + EPSILON
            && point[1] >= start[1].min(end[1]) - EPSILON
            && point[1] <= start[1].max(end[1]) + EPSILON
        {
            return true;
        }
    }
    false
}

fn palette_color(id: FeatureId) -> [f32; 4] {
    const COLORS: [[f32; 4]; 5] = [
        [0.22, 0.68, 0.65, 1.0],
        [0.91, 0.52, 0.23, 1.0],
        [0.51, 0.62, 0.82, 1.0],
        [0.76, 0.72, 0.35, 1.0],
        [0.66, 0.48, 0.70, 1.0],
    ];
    match id.saturating_sub(1) % COLORS.len() as u64 {
        0 => COLORS[0],
        1 => COLORS[1],
        2 => COLORS[2],
        3 => COLORS[3],
        _ => COLORS[4],
    }
}

fn color_is_valid(color: &[f32; 4]) -> bool {
    color
        .iter()
        .all(|component| component.is_finite() && (0.0..=1.0).contains(component))
}

fn validate_step_boundaries(
    outer_shell_id: u64,
    void_shells: &[StepShellBoundary],
) -> Result<(), DocumentError> {
    if void_shells.len() > MAX_STEP_VOID_SHELLS {
        return Err(DocumentError::InvalidParameter(format!(
            "imported STEP solid exceeds the limit of {MAX_STEP_VOID_SHELLS} void shells"
        )));
    }
    let mut shell_ids = std::collections::BTreeSet::from([outer_shell_id]);
    for boundary in void_shells {
        if boundary.shell_id == 0 || !shell_ids.insert(boundary.shell_id) {
            return Err(DocumentError::InvalidParameter(
                "imported STEP boundary shell ids must be non-zero and unique".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ModelCommand {
    CreateBox {
        name: String,
        size: [f64; 3],
        #[serde(default)]
        position: [f64; 3],
    },
    CreateCylinder {
        name: String,
        radius: f64,
        height: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateSphere {
        name: String,
        radius: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateCone {
        name: String,
        bottom_radius: f64,
        top_radius: f64,
        height: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateTorus {
        name: String,
        major_radius: f64,
        minor_radius: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateExtrusion {
        name: String,
        profile: Vec<[f64; 2]>,
        height: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateSketch {
        name: String,
        #[serde(default)]
        plane: SketchPlane,
        profile: Vec<[f64; 2]>,
        #[serde(default)]
        holes: Vec<Vec<[f64; 2]>>,
        #[serde(default)]
        constraints: Vec<Constraint>,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateSketchRegion {
        name: String,
        #[serde(default)]
        plane: SketchPlane,
        #[serde(flatten)]
        region: SketchRegion2D,
        #[serde(default)]
        construction: Vec<SketchSegment2D>,
        #[serde(default)]
        constraints: Vec<Constraint>,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateExtrusionFromSketch {
        name: String,
        sketch_id: FeatureId,
        height: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateRevolveFromSketch {
        name: String,
        sketch_id: FeatureId,
        axis_origin: [f64; 2],
        axis_direction: [f64; 2],
        angle: f64,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateLoftFromSketches {
        name: String,
        sketch_ids: Vec<FeatureId>,
        #[serde(default)]
        position: [f64; 3],
    },
    ImportStep {
        name: String,
        source: String,
        #[serde(default)]
        data_section: usize,
        shell_id: u64,
        #[serde(default)]
        void_shells: Vec<StepShellBoundary>,
        #[serde(default)]
        length_unit: StepLengthUnit,
        #[serde(default)]
        color: Option<[f32; 4]>,
        #[serde(default)]
        position: [f64; 3],
    },
    CreateBoolean {
        name: String,
        operation: BooleanOperation,
        left: FeatureId,
        right: FeatureId,
    },
    CreateChamfer {
        name: String,
        #[serde(alias = "edge", deserialize_with = "deserialize_edge_refs")]
        edges: Vec<EdgeRef>,
        distance: f64,
    },
    CreateFillet {
        name: String,
        #[serde(alias = "edge", deserialize_with = "deserialize_edge_refs")]
        edges: Vec<EdgeRef>,
        radius: f64,
    },
    CreateDatumPlane {
        name: String,
        face: FaceRef,
        #[serde(default)]
        offset: f64,
    },
    CreateDatumPoint {
        name: String,
        vertex: VertexRef,
        #[serde(default)]
        offset: [f64; 3],
    },
    Duplicate {
        id: FeatureId,
        #[serde(default)]
        name: String,
        #[serde(default)]
        position: [f64; 3],
    },
    Move {
        id: FeatureId,
        position: [f64; 3],
    },
    Rotate {
        id: FeatureId,
        /// Absolute Euler rotation in degrees around X, Y, and Z.
        rotation: [f64; 3],
    },
    ResizeBox {
        id: FeatureId,
        size: [f64; 3],
    },
    ResizeCylinder {
        id: FeatureId,
        radius: f64,
        height: f64,
    },
    ResizeSphere {
        id: FeatureId,
        radius: f64,
    },
    ResizeCone {
        id: FeatureId,
        bottom_radius: f64,
        top_radius: f64,
        height: f64,
    },
    ResizeTorus {
        id: FeatureId,
        major_radius: f64,
        minor_radius: f64,
    },
    ResizeExtrusion {
        id: FeatureId,
        profile: Vec<[f64; 2]>,
        height: f64,
    },
    ResizeRevolve {
        id: FeatureId,
        axis_origin: [f64; 2],
        axis_direction: [f64; 2],
        angle: f64,
    },
    ResizeSketch {
        id: FeatureId,
        profile: Vec<[f64; 2]>,
    },
    SetSketchHoles {
        id: FeatureId,
        holes: Vec<Vec<[f64; 2]>>,
    },
    SetSketchRegion {
        id: FeatureId,
        #[serde(flatten)]
        region: SketchRegion2D,
    },
    SetSketchDefinition {
        id: FeatureId,
        #[serde(flatten)]
        region: SketchRegion2D,
        #[serde(default)]
        construction: Vec<SketchSegment2D>,
        #[serde(default)]
        constraints: Vec<Constraint>,
    },
    SetSketchConstraints {
        id: FeatureId,
        constraints: Vec<Constraint>,
    },
    SetSketchPlane {
        id: FeatureId,
        plane: SketchPlane,
    },
    SetDatumPlaneOffset {
        id: FeatureId,
        offset: f64,
    },
    SetDatumPointOffset {
        id: FeatureId,
        offset: [f64; 3],
    },
    SetChamferDistance {
        id: FeatureId,
        distance: f64,
    },
    SetFilletRadius {
        id: FeatureId,
        radius: f64,
    },
    Rename {
        id: FeatureId,
        name: String,
    },
    SetVisibility {
        id: FeatureId,
        visible: bool,
    },
    SetColor {
        id: FeatureId,
        color: [f32; 4],
    },
    SetMaterial {
        id: FeatureId,
        name: String,
        density_kg_m3: f64,
    },
    ClearMaterial {
        id: FeatureId,
    },
    CreateAssembly {
        name: String,
        definitions: Vec<ComponentDefinition>,
        occurrences: Vec<ComponentOccurrence>,
    },
    SetOccurrenceTransform {
        assembly_id: AssemblyId,
        occurrence_id: u64,
        /// Absolute local translation in the parent occurrence's coordinates.
        position: [f64; 3],
        /// Absolute local XYZ Euler rotation in degrees.
        rotation: [f64; 3],
    },
    CreateAssemblyMate {
        assembly_id: AssemblyId,
        mate: AssemblyMate,
    },
    SetAssemblyMateState {
        assembly_id: AssemblyId,
        mate_id: AssemblyMateId,
        state: f64,
    },
    DeleteAssemblyMate {
        assembly_id: AssemblyId,
        mate_id: AssemblyMateId,
    },
    SetOccurrenceSuppressed {
        assembly_id: AssemblyId,
        occurrence_id: u64,
        suppressed: bool,
    },
    DeleteAssembly {
        id: AssemblyId,
    },
    Delete {
        id: FeatureId,
    },
}

impl ModelCommand {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::CreateBox { .. } => "Create box",
            Self::CreateCylinder { .. } => "Create cylinder",
            Self::CreateSphere { .. } => "Create sphere",
            Self::CreateCone { .. } => "Create cone",
            Self::CreateTorus { .. } => "Create torus",
            Self::CreateExtrusion { .. } => "Create extrusion",
            Self::CreateSketch { .. } => "Create sketch",
            Self::CreateSketchRegion { .. } => "Create curved sketch",
            Self::CreateExtrusionFromSketch { .. } => "Create extrusion from sketch",
            Self::CreateRevolveFromSketch { .. } => "Create revolve from sketch",
            Self::CreateLoftFromSketches { .. } => "Create loft from sketches",
            Self::ImportStep { .. } => "Import STEP solid",
            Self::CreateBoolean { operation, .. } => operation.label(),
            Self::CreateChamfer { .. } => "Create chamfer",
            Self::CreateFillet { .. } => "Create fillet",
            Self::CreateDatumPlane { .. } => "Create datum plane",
            Self::CreateDatumPoint { .. } => "Create datum point",
            Self::Duplicate { .. } => "Duplicate feature",
            Self::Move { .. } => "Move feature",
            Self::Rotate { .. } => "Rotate feature",
            Self::ResizeBox { .. } => "Resize box",
            Self::ResizeCylinder { .. } => "Resize cylinder",
            Self::ResizeSphere { .. } => "Resize sphere",
            Self::ResizeCone { .. } => "Resize cone",
            Self::ResizeTorus { .. } => "Resize torus",
            Self::ResizeExtrusion { .. } => "Resize extrusion",
            Self::ResizeRevolve { .. } => "Resize revolve",
            Self::ResizeSketch { .. } => "Resize sketch",
            Self::SetSketchHoles { .. } => "Set sketch holes",
            Self::SetSketchRegion { .. } => "Set sketch region",
            Self::SetSketchDefinition { .. } => "Set sketch definition",
            Self::SetSketchConstraints { .. } => "Set sketch constraints",
            Self::SetSketchPlane { .. } => "Set sketch plane",
            Self::SetDatumPlaneOffset { .. } => "Move datum plane",
            Self::SetDatumPointOffset { .. } => "Move datum point",
            Self::SetChamferDistance { .. } => "Resize chamfer",
            Self::SetFilletRadius { .. } => "Resize fillet",
            Self::Rename { .. } => "Rename feature",
            Self::SetVisibility { .. } => "Change visibility",
            Self::SetColor { .. } => "Change color",
            Self::SetMaterial { .. } => "Set material",
            Self::ClearMaterial { .. } => "Clear material",
            Self::CreateAssembly { .. } => "Create assembly",
            Self::SetOccurrenceTransform { .. } => "Move assembly occurrence",
            Self::CreateAssemblyMate { .. } => "Create assembly mate",
            Self::SetAssemblyMateState { .. } => "Move assembly mate",
            Self::DeleteAssemblyMate { .. } => "Delete assembly mate",
            Self::SetOccurrenceSuppressed { .. } => "Suppress assembly occurrence",
            Self::DeleteAssembly { .. } => "Delete assembly",
            Self::Delete { .. } => "Delete feature",
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DocumentError {
    #[error("feature {0} does not exist")]
    FeatureNotFound(FeatureId),
    #[error("assembly {0} does not exist")]
    AssemblyNotFound(AssemblyId),
    #[error("assembly {assembly} occurrence {occurrence} does not exist")]
    OccurrenceNotFound {
        assembly: AssemblyId,
        occurrence: u64,
    },
    #[error("assembly {assembly} mate {mate} does not exist")]
    AssemblyMateNotFound {
        assembly: AssemblyId,
        mate: AssemblyMateId,
    },
    #[error("assembly {assembly} occurrence {occurrence} is driven by mate {mate}")]
    MateDrivenOccurrence {
        assembly: AssemblyId,
        occurrence: ComponentOccurrenceId,
        mate: AssemblyMateId,
    },
    #[error("invalid assembly: {0}")]
    Assembly(#[from] AssemblyError),
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("{0}")]
    SketchConstraint(#[from] SketchConstraintDiagnostic),
    #[error("feature {id} is not a {expected}")]
    PrimitiveMismatch {
        id: FeatureId,
        expected: &'static str,
    },
    #[error("feature id space exhausted")]
    IdOverflow,
    #[error("feature id {0} is zero or duplicated")]
    InvalidFeatureId(FeatureId),
    #[error(
        "feature {feature} depends on missing or incompatible feature {dependency}; expected {expected}"
    )]
    InvalidDependency {
        feature: FeatureId,
        dependency: FeatureId,
        expected: &'static str,
    },
    #[error("active feature {feature} depends on suppressed assembly feature {dependency}")]
    SuppressedFeatureDependency {
        feature: FeatureId,
        dependency: FeatureId,
    },
    #[error("feature dependency graph contains a cycle: {cycle:?}")]
    DependencyCycle { cycle: Vec<FeatureId> },
    #[error("feature {id} is used by dependent feature {dependent}")]
    FeatureInUse { id: FeatureId, dependent: FeatureId },
    #[error("feature {id} belongs to assembly {assembly} occurrence {occurrence}")]
    FeatureInAssembly {
        id: FeatureId,
        assembly: AssemblyId,
        occurrence: u64,
    },
    #[error(
        "feature {id} belongs to assembly {first_assembly} occurrence {first_occurrence} and assembly {second_assembly} occurrence {second_occurrence}"
    )]
    FeatureInMultipleAssemblies {
        id: FeatureId,
        first_assembly: AssemblyId,
        first_occurrence: u64,
        second_assembly: AssemblyId,
        second_occurrence: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_circle_loop(center: [f64; 2], radius: f64) -> SketchLoop2D {
        let right = [center[0] + radius, center[1]];
        let left = [center[0] - radius, center[1]];
        SketchLoop2D {
            segments: vec![
                SketchSegment2D::Arc {
                    start: right,
                    end: left,
                    center,
                    ccw: true,
                },
                SketchSegment2D::Arc {
                    start: left,
                    end: right,
                    center,
                    ccw: true,
                },
            ],
        }
    }

    #[test]
    fn transactions_are_atomic() {
        let mut document = CadDocument::default();
        let commands = vec![
            ModelCommand::CreateBox {
                name: "valid".into(),
                size: [1.0, 2.0, 3.0],
                position: [0.0; 3],
            },
            ModelCommand::CreateCylinder {
                name: "invalid".into(),
                radius: -1.0,
                height: 3.0,
                position: [0.0; 3],
            },
        ];

        assert!(document.apply_transaction(commands).is_err());
        assert!(document.features.is_empty());
    }

    #[test]
    fn default_name_and_color_are_stable() {
        let mut document = CadDocument::default();
        let id = document
            .apply(ModelCommand::CreateBox {
                name: "   ".into(),
                size: [1.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let feature = document.feature(id).unwrap();
        assert_eq!(feature.name, "Box");
        let expected = [0.22, 0.68, 0.65, 1.0];
        assert!(
            feature
                .color
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn validates_and_repairs_loaded_document_ids() {
        let mut document = CadDocument::default();
        document
            .apply(ModelCommand::CreateSphere {
                name: "ball".into(),
                radius: 4.0,
                position: [1.0, 2.0, 3.0],
            })
            .unwrap();
        document.next_id = 1;

        document.validate_and_repair().unwrap();
        let id = document
            .apply(ModelCommand::CreateCone {
                name: "frustum".into(),
                bottom_radius: 5.0,
                top_radius: 2.0,
                height: 8.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert_eq!(id, 2);
    }

    #[test]
    fn rotation_is_an_absolute_finite_parameter() {
        let mut document = CadDocument::default();
        let id = document
            .apply(ModelCommand::CreateBox {
                name: "rotor".into(),
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
        assert_eq!(
            document.feature(id).unwrap().rotation,
            Vec3::new(0.0, 0.0, 90.0)
        );
        assert!(
            document
                .apply(ModelCommand::Rotate {
                    id,
                    rotation: [f64::NAN, 0.0, 0.0],
                })
                .is_err()
        );
    }

    #[test]
    fn torus_requires_a_non_self_intersecting_profile() {
        let mut document = CadDocument::default();
        assert!(
            document
                .apply(ModelCommand::CreateTorus {
                    name: "invalid torus".into(),
                    major_radius: 4.0,
                    minor_radius: 4.0,
                    position: [0.0; 3],
                })
                .is_err()
        );
        let id = document
            .apply(ModelCommand::CreateTorus {
                name: "seal".into(),
                major_radius: 12.0,
                minor_radius: 3.0,
                position: [1.0, 2.0, 3.0],
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            document.feature(id).unwrap().primitive,
            Primitive::Torus { .. }
        ));
    }

    #[test]
    fn duplicate_allocates_a_new_id_and_inherits_transform() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "source".into(),
                size: [2.0; 3],
                position: [1.0, 2.0, 3.0],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::Rotate {
                id: source,
                rotation: [10.0, 20.0, 30.0],
            })
            .unwrap();
        document
            .apply(ModelCommand::SetMaterial {
                id: source,
                name: " Aluminum 6061 ".into(),
                density_kg_m3: 2_700.0,
            })
            .unwrap();
        let copy = document
            .apply(ModelCommand::Duplicate {
                id: source,
                name: String::new(),
                position: [8.0, 9.0, 10.0],
            })
            .unwrap()
            .unwrap();
        assert_ne!(source, copy);
        let feature = document.feature(copy).unwrap();
        assert_eq!(feature.name, "source copy");
        assert_eq!(feature.translation, Vec3::new(8.0, 9.0, 10.0));
        assert_eq!(feature.rotation, Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(
            feature.material,
            Some(Material {
                name: "Aluminum 6061".into(),
                density_kg_m3: 2_700.0,
            })
        );
    }

    #[test]
    fn material_commands_validate_assignments_and_clear_metadata() {
        let mut document = CadDocument::default();
        let solid = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [2.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::SetMaterial {
                id: solid,
                name: "Steel".into(),
                density_kg_m3: 7_850.0,
            })
            .unwrap();
        assert_eq!(
            document
                .feature(solid)
                .unwrap()
                .material
                .as_ref()
                .unwrap()
                .name,
            "Steel"
        );
        for (name, density_kg_m3) in [("", 1_000.0), ("Steel", 0.0), ("Steel", f64::NAN)] {
            assert!(
                document
                    .apply(ModelCommand::SetMaterial {
                        id: solid,
                        name: name.into(),
                        density_kg_m3,
                    })
                    .is_err()
            );
        }
        document
            .apply(ModelCommand::ClearMaterial { id: solid })
            .unwrap();
        assert!(document.feature(solid).unwrap().material.is_none());

        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "profile".into(),
                profile: vec![[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::SetMaterial {
                id: sketch,
                name: "Steel".into(),
                density_kg_m3: 7_850.0,
            }),
            Err(DocumentError::PrimitiveMismatch {
                expected: "solid feature",
                ..
            })
        ));
    }

    #[test]
    fn color_changes_are_validated() {
        let mut document = CadDocument::default();
        let id = document
            .apply(ModelCommand::CreateSphere {
                name: "colored".into(),
                radius: 2.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        document
            .apply(ModelCommand::SetColor {
                id,
                color: [0.1, 0.2, 0.3, 1.0],
            })
            .unwrap();
        let actual = document.feature(id).unwrap().color;
        assert!(
            actual
                .iter()
                .zip([0.1, 0.2, 0.3, 1.0])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
        assert!(
            document
                .apply(ModelCommand::SetColor {
                    id,
                    color: [1.2, 0.0, 0.0, 1.0],
                })
                .is_err()
        );
    }

    #[test]
    fn extrusion_profiles_reject_degenerate_and_self_intersecting_polygons() {
        let mut document = CadDocument::default();
        let invalid_profiles = [
            vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
            vec![[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]],
        ];
        for profile in invalid_profiles {
            assert!(
                document
                    .apply(ModelCommand::CreateExtrusion {
                        name: "invalid".into(),
                        profile,
                        height: 5.0,
                        position: [0.0; 3],
                    })
                    .is_err()
            );
        }
        let id = document
            .apply(ModelCommand::CreateExtrusion {
                name: "plate".into(),
                profile: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]],
                height: 8.0,
                position: [1.0, 2.0, 3.0],
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            document.feature(id).unwrap().primitive,
            Primitive::Extrusion { .. }
        ));
    }

    #[test]
    fn extrusion_from_sketch_keeps_a_parametric_dependency() {
        let mut document = CadDocument::default();
        let sketch_id = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "outline".into(),
                profile: vec![[0.0, 0.0], [12.0, 0.0], [12.0, 8.0], [0.0, 8.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion_id = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "pad".into(),
                sketch_id,
                height: 6.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            document.feature(extrusion_id).unwrap().primitive,
            Primitive::ExtrusionFromSketch {
                sketch_id: id,
                ..
            } if id == sketch_id
        ));
        document
            .apply(ModelCommand::ResizeExtrusion {
                id: extrusion_id,
                profile: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 8.0], [0.0, 8.0]],
                height: 10.0,
            })
            .unwrap();
        assert!(matches!(
            document.feature(extrusion_id).unwrap().primitive,
            Primitive::ExtrusionFromSketch {
                sketch_id: id,
                ..
            } if id == sketch_id
        ));
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: sketch_id }),
            Err(DocumentError::FeatureInUse {
                id,
                dependent
            }) if id == sketch_id && dependent == extrusion_id
        ));
        document
            .apply(ModelCommand::Delete { id: extrusion_id })
            .unwrap();
        document
            .apply(ModelCommand::Delete { id: sketch_id })
            .unwrap();
    }

    #[test]
    fn sketch_holes_are_parametric_and_synchronize_linked_extrusions() {
        let mut document = CadDocument::default();
        let initial_hole = vec![[6.0, 4.0], [10.0, 4.0], [10.0, 8.0], [6.0, 8.0]];
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::WorldXy,
                name: "window profile".into(),
                profile: vec![[0.0, 0.0], [16.0, 0.0], [16.0, 12.0], [0.0, 12.0]],
                holes: vec![initial_hole.clone()],
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "window plate".into(),
                sketch_id: sketch,
                height: 3.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            &document.feature(extrusion).unwrap().primitive,
            Primitive::ExtrusionFromSketch { region, .. }
                if region.holes.len() == 1 && region.holes[0].vertices() == initial_hole
        ));

        let updated_hole = vec![[5.0, 3.0], [11.0, 3.0], [11.0, 9.0], [5.0, 9.0]];
        document
            .apply(ModelCommand::SetSketchHoles {
                id: sketch,
                holes: vec![updated_hole.clone()],
            })
            .unwrap();
        assert!(matches!(
            &document.feature(extrusion).unwrap().primitive,
            Primitive::ExtrusionFromSketch { region, .. }
                if region.holes.len() == 1 && region.holes[0].vertices() == updated_hole
        ));
        document
            .apply(ModelCommand::ResizeSketch {
                id: sketch,
                profile: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 14.0], [0.0, 14.0]],
            })
            .unwrap();
        assert!(matches!(
            &document.feature(sketch).unwrap().primitive,
            Primitive::Sketch { region, .. } if region.holes.len() == 1
        ));
        assert!(matches!(
            document.apply(ModelCommand::CreateRevolveFromSketch {
                name: "unsupported turn".into(),
                sketch_id: sketch,
                axis_origin: [25.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
                position: [0.0; 3],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));

        document
            .apply(ModelCommand::SetSketchHoles {
                id: sketch,
                holes: Vec::new(),
            })
            .unwrap();
        document
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "turn".into(),
                sketch_id: sketch,
                axis_origin: [25.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
                position: [0.0; 3],
            })
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchHoles {
                id: sketch,
                holes: vec![vec![[5.0, 3.0], [11.0, 3.0], [11.0, 9.0], [5.0, 9.0]]],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert!(matches!(
            &document.feature(sketch).unwrap().primitive,
            Primitive::Sketch { region, .. } if region.holes.is_empty()
        ));
    }

    #[test]
    fn sketch_holes_reject_invalid_region_topology() {
        let profile = vec![[0.0, 0.0], [20.0, 0.0], [20.0, 16.0], [0.0, 16.0]];
        let invalid_holes = [
            vec![vec![[21.0, 2.0], [24.0, 2.0], [24.0, 5.0], [21.0, 5.0]]],
            vec![vec![[0.0, 2.0], [4.0, 2.0], [4.0, 6.0], [0.0, 6.0]]],
            vec![
                vec![[3.0, 3.0], [11.0, 3.0], [11.0, 10.0], [3.0, 10.0]],
                vec![[8.0, 6.0], [16.0, 6.0], [16.0, 13.0], [8.0, 13.0]],
            ],
            vec![
                vec![[3.0, 3.0], [17.0, 3.0], [17.0, 13.0], [3.0, 13.0]],
                vec![[6.0, 6.0], [10.0, 6.0], [10.0, 9.0], [6.0, 9.0]],
            ],
            vec![vec![[4.0, 4.0], [12.0, 12.0], [4.0, 12.0], [12.0, 4.0]]],
        ];
        for holes in invalid_holes {
            let mut document = CadDocument::default();
            assert!(matches!(
                document.apply(ModelCommand::CreateSketch {
                    plane: SketchPlane::WorldXy,
                    name: "invalid holes".into(),
                    profile: profile.clone(),
                    holes,
                    constraints: Vec::new(),
                    position: [0.0; 3],
                }),
                Err(DocumentError::InvalidParameter(_))
            ));
            assert!(document.features.is_empty());
        }
    }

    #[test]
    fn sketch_hole_edits_fail_atomically_when_outer_profile_becomes_invalid() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::WorldXy,
                name: "constrained window profile".into(),
                profile: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 16.0], [0.0, 16.0]],
                holes: vec![vec![[6.0, 5.0], [14.0, 5.0], [14.0, 11.0], [6.0, 11.0]]],
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let before_resize = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::ResizeSketch {
                id: sketch,
                profile: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before_resize);

        let before_constraints = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchConstraints {
                id: sketch,
                constraints: vec![Constraint::Fixed {
                    point: 0,
                    x: 8.0,
                    y: 7.0,
                }],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before_constraints);
    }

    #[test]
    fn sketch_hole_edits_enforce_loop_and_total_point_limits_atomically() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::WorldXy,
                name: "bounded profile".into(),
                profile: vec![
                    [-500.0, -100.0],
                    [500.0, -100.0],
                    [500.0, 100.0],
                    [-500.0, 100.0],
                ],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let before = document.clone();

        let too_many_holes = (0..33)
            .map(|index| {
                let x = f64::from(index) * 2.0 - 40.0;
                vec![[x, -1.0], [x + 1.0, -1.0], [x + 1.0, 0.0], [x, 0.0]]
            })
            .collect();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchHoles {
                id: sketch,
                holes: too_many_holes,
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before);

        let too_many_points = (0..8)
            .map(|hole| {
                let center_x = f64::from(hole) * 100.0 - 350.0;
                (0..128)
                    .map(|point| {
                        let angle = f64::from(point) * std::f64::consts::TAU / 128.0;
                        [center_x + angle.cos() * 10.0, angle.sin() * 10.0]
                    })
                    .collect()
            })
            .collect();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchHoles {
                id: sketch,
                holes: too_many_points,
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before);
    }

    #[test]
    fn constrained_sketches_solve_and_update_linked_extrusions() {
        let mut document = CadDocument::default();
        let sketch_id = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "constrained outline".into(),
                profile: vec![[0.0, 0.0], [9.0, 1.0], [10.0, 6.0], [1.0, 5.0]],
                holes: Vec::new(),
                constraints: vec![
                    Constraint::Fixed {
                        point: 0,
                        x: 0.0,
                        y: 0.0,
                    },
                    Constraint::Horizontal { segment: 0 },
                    Constraint::Vertical { segment: 1 },
                ],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion_id = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "pad".into(),
                sketch_id,
                height: 4.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let sketch = document.feature(sketch_id).unwrap();
        let Primitive::Sketch {
            region,
            constraints,
            ..
        } = &sketch.primitive
        else {
            panic!("expected sketch");
        };
        let profile = region.profile.vertices();
        assert_eq!(constraints.len(), 3);
        assert!((profile[0][1] - profile[1][1]).abs() > 1.0e-4);
        document
            .apply(ModelCommand::SetSketchConstraints {
                id: sketch_id,
                constraints: vec![
                    Constraint::Fixed {
                        point: 0,
                        x: 2.0,
                        y: 3.0,
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
            })
            .unwrap();
        let Primitive::ExtrusionFromSketch { region, .. } =
            &document.feature(extrusion_id).unwrap().primitive
        else {
            panic!("expected linked extrusion");
        };
        let profile = region.profile.vertices();
        assert!((profile[0][0] - 2.0).abs() < 1.0e-7);
        assert!((profile[0][1] - 3.0).abs() < 1.0e-7);
        let direction = |segment: usize| {
            let next = (segment + 1) % profile.len();
            let vector = [
                profile[next][0] - profile[segment][0],
                profile[next][1] - profile[segment][1],
            ];
            let length = vector[0].hypot(vector[1]);
            ([vector[0] / length, vector[1] / length], length)
        };
        let directions = (0..4).map(direction).collect::<Vec<_>>();
        assert!((directions[0].1 - 10.0).abs() < 1.0e-8);
        assert!((directions[1].1 - 5.0).abs() < 1.0e-8);
        assert!((directions[0].1 - directions[2].1).abs() < 1.0e-8);
        assert!((directions[1].1 - directions[3].1).abs() < 1.0e-8);
        assert!(
            directions[0]
                .0
                .iter()
                .zip(directions[2].0)
                .map(|(first, second)| first * second)
                .sum::<f64>()
                .abs()
                > 1.0 - 1.0e-8
        );
        assert!(
            directions[0]
                .0
                .iter()
                .zip(directions[1].0)
                .map(|(first, second)| first * second)
                .sum::<f64>()
                .abs()
                < 1.0e-8
        );

        let before_conflict = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchConstraints {
                id: sketch_id,
                constraints: vec![
                    Constraint::Length {
                        segment: 0,
                        length: 10.0,
                    },
                    Constraint::Length {
                        segment: 0,
                        length: 11.0,
                    },
                ],
            }),
            Err(DocumentError::SketchConstraint(diagnostic))
                if diagnostic.reason == SketchConstraintFailureReason::Conflict
                    && diagnostic.constraint_indices == vec![0, 1]
        ));
        assert_eq!(document, before_conflict);
    }

    #[test]
    fn construction_point_relationships_update_dependents_atomically() {
        let mut document = CadDocument::default();
        let source_region = SketchRegion2D::from_polygons(
            vec![[0.0, 1.0], [9.0, 1.0], [10.0, 6.0], [0.0, 6.0]],
            Vec::new(),
        );
        let sketch_id = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "construction driven outline".into(),
                region: source_region.clone(),
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion_id = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "construction driven pad".into(),
                sketch_id,
                height: 3.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let construction = vec![SketchSegment2D::Line {
            start: [-10.0, 0.0],
            end: [20.0, 0.0],
        }];
        let constraints = vec![
            Constraint::Fixed {
                point: 4,
                x: -10.0,
                y: 0.0,
            },
            Constraint::Fixed {
                point: 5,
                x: 20.0,
                y: 0.0,
            },
            Constraint::PointOnCurve {
                point: 0,
                segment: 4,
            },
        ];
        document
            .apply(ModelCommand::SetSketchDefinition {
                id: sketch_id,
                region: source_region.clone(),
                construction: construction.clone(),
                constraints: constraints.clone(),
            })
            .unwrap();

        assert!(matches!(
            &document.feature(sketch_id).unwrap().primitive,
            Primitive::Sketch {
                construction: stored,
                constraints: stored_constraints,
                ..
            } if stored == &construction && stored_constraints == &constraints
        ));
        let Primitive::ExtrusionFromSketch { region: cached, .. } =
            &document.feature(extrusion_id).unwrap().primitive
        else {
            panic!("expected linked extrusion");
        };
        assert!(cached.profile.vertices()[0][1].abs() < 1.0e-8);

        let before_conflict = document.clone();
        let mut conflicting = constraints;
        conflicting.push(Constraint::Fixed {
            point: 0,
            x: 0.0,
            y: 2.0,
        });
        assert!(matches!(
            document.apply(ModelCommand::SetSketchDefinition {
                id: sketch_id,
                region: source_region,
                construction,
                constraints: conflicting,
            }),
            Err(DocumentError::SketchConstraint(diagnostic))
                if diagnostic.reason == SketchConstraintFailureReason::Conflict
        ));
        assert_eq!(document, before_conflict);
    }

    #[test]
    fn revolve_from_sketch_is_parametric_and_axis_validated() {
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
        let revolve_id = document
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "turned body".into(),
                sketch_id,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            document.feature_graph().unwrap().dependencies(revolve_id),
            Some(&[sketch_id][..])
        );
        document
            .apply(ModelCommand::ResizeRevolve {
                id: revolve_id,
                axis_origin: [1.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 180.0,
            })
            .unwrap();
        assert!(matches!(
            document.feature(revolve_id).unwrap().primitive,
            Primitive::RevolveFromSketch { angle, .. } if (angle - 180.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            document.apply(ModelCommand::ResizeRevolve {
                id: revolve_id,
                axis_origin: [5.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
    }

    #[test]
    fn loft_from_sketches_tracks_ordered_dependencies_and_updates_atomically() {
        let mut document = CadDocument::default();
        let mut sketch_ids = Vec::new();
        for (index, z) in [0.0, 10.0, 20.0].into_iter().enumerate() {
            sketch_ids.push(
                document
                    .apply(ModelCommand::CreateSketch {
                        name: format!("section {index}"),
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
        let loft = document
            .apply(ModelCommand::CreateLoftFromSketches {
                name: "three section loft".into(),
                sketch_ids: sketch_ids.clone(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            document.feature_graph().unwrap().dependencies(loft),
            Some(sketch_ids.as_slice())
        );
        assert!(matches!(
            &document.feature(loft).unwrap().primitive,
            Primitive::LoftFromSketches { profiles, .. } if profiles.len() == 3
        ));

        let updated = vec![[-4.0, -6.0], [4.0, -6.0], [4.0, 6.0], [-4.0, 6.0]];
        document
            .apply(ModelCommand::ResizeSketch {
                id: sketch_ids[1],
                profile: updated.clone(),
            })
            .unwrap();
        assert!(matches!(
            &document.feature(loft).unwrap().primitive,
            Primitive::LoftFromSketches { profiles, .. }
                if profiles[1].vertices() == updated
        ));

        let before = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::ResizeSketch {
                id: sketch_ids[1],
                profile: vec![[-4.0, -4.0], [4.0, -4.0], [0.0, 4.0]],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before);
        assert!(
            document
                .apply(ModelCommand::Delete { id: sketch_ids[0] })
                .is_err()
        );
        assert!(
            document
                .apply(ModelCommand::CreateLoftFromSketches {
                    name: "duplicate section".into(),
                    sketch_ids: vec![sketch_ids[0], sketch_ids[0]],
                    position: [0.0; 3],
                })
                .is_err()
        );
    }

    #[test]
    fn exact_arc_revolve_rejects_axis_tangency_atomically() {
        let mut document = CadDocument::default();
        let sketch_id = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::default(),
                name: "tangent circle".into(),
                region: SketchRegion2D {
                    profile: SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::Arc {
                                start: [10.0, 0.0],
                                end: [0.0, 0.0],
                                center: [5.0, 0.0],
                                ccw: true,
                            },
                            SketchSegment2D::Arc {
                                start: [0.0, 0.0],
                                end: [10.0, 0.0],
                                center: [5.0, 0.0],
                                ccw: true,
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
        let before = document.clone();

        assert!(matches!(
            document.apply(ModelCommand::CreateRevolveFromSketch {
                name: "invalid tangent revolve".into(),
                sketch_id,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 360.0,
                position: [0.0; 3],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before);
    }

    #[test]
    fn exact_region_edits_synchronize_dependents_and_fail_atomically() {
        let mut document = CadDocument::default();
        let initial_region = SketchRegion2D {
            profile: exact_circle_loop([8.0, 0.0], 2.0),
            holes: Vec::new(),
        };
        let sketch_id = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::default(),
                name: "exact source".into(),
                region: initial_region,
                construction: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let extrusion_id = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "exact pad".into(),
                sketch_id,
                height: 3.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let revolve_id = document
            .apply(ModelCommand::CreateRevolveFromSketch {
                name: "exact turn".into(),
                sketch_id,
                axis_origin: [0.0, 0.0],
                axis_direction: [0.0, 1.0],
                angle: 180.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();

        let updated_region = SketchRegion2D {
            profile: exact_circle_loop([9.0, 1.0], 3.0),
            holes: Vec::new(),
        };
        document
            .apply(ModelCommand::SetSketchRegion {
                id: sketch_id,
                region: updated_region.clone(),
            })
            .unwrap();
        assert!(matches!(
            &document.feature(extrusion_id).unwrap().primitive,
            Primitive::ExtrusionFromSketch { region, .. } if region == &updated_region
        ));
        assert!(matches!(
            &document.feature(revolve_id).unwrap().primitive,
            Primitive::RevolveFromSketch { profile, .. } if profile == &updated_region.profile
        ));

        document
            .apply(ModelCommand::SetSketchConstraints {
                id: sketch_id,
                constraints: vec![
                    Constraint::FixedCenter {
                        segment: 0,
                        x: 10.0,
                        y: 2.0,
                    },
                    Constraint::Radius {
                        segment: 0,
                        radius: 2.5,
                    },
                    Constraint::Concentric {
                        first: 0,
                        second: 1,
                    },
                    Constraint::EqualRadius {
                        first: 0,
                        second: 1,
                    },
                ],
            })
            .unwrap();
        let Primitive::ExtrusionFromSketch {
            region: extrusion_region,
            ..
        } = &document.feature(extrusion_id).unwrap().primitive
        else {
            panic!("expected linked extrusion");
        };
        let Primitive::RevolveFromSketch {
            profile: revolve_profile,
            ..
        } = &document.feature(revolve_id).unwrap().primitive
        else {
            panic!("expected linked revolve");
        };
        assert_eq!(&extrusion_region.profile, revolve_profile);
        for segment in &extrusion_region.profile.segments {
            let SketchSegment2D::Arc { start, center, .. } = segment else {
                panic!("expected exact arc cache");
            };
            assert!((center[0] - 10.0).abs() < 1.0e-8);
            assert!((center[1] - 2.0).abs() < 1.0e-8);
            assert!(((start[0] - center[0]).hypot(start[1] - center[1]) - 2.5).abs() < 1.0e-8);
        }

        let before_constraint = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchConstraints {
                id: sketch_id,
                constraints: vec![
                    Constraint::FixedCenter {
                        segment: 0,
                        x: 0.0,
                        y: 0.0,
                    },
                    Constraint::FixedCenter {
                        segment: 0,
                        x: 10.0,
                        y: 0.0,
                    },
                ],
            }),
            Err(DocumentError::SketchConstraint(diagnostic))
                if diagnostic.reason == SketchConstraintFailureReason::Conflict
                    && diagnostic.constraint_indices == vec![0, 1]
        ));
        assert_eq!(document, before_constraint);

        document
            .apply(ModelCommand::SetSketchConstraints {
                id: sketch_id,
                constraints: Vec::new(),
            })
            .unwrap();

        let before_axis_crossing = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchRegion {
                id: sketch_id,
                region: SketchRegion2D {
                    profile: exact_circle_loop([2.0, 0.0], 3.0),
                    holes: Vec::new(),
                },
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert_eq!(document, before_axis_crossing);
    }

    #[test]
    fn conflicting_curvature_continuity_edit_is_atomic() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketchRegion {
                plane: SketchPlane::WorldXy,
                name: "opposed curvature".into(),
                region: SketchRegion2D {
                    profile: SketchLoop2D {
                        segments: vec![
                            SketchSegment2D::Arc {
                                start: [0.0, 0.0],
                                end: [2.0, 2.0],
                                center: [0.0, 2.0],
                                ccw: true,
                            },
                            SketchSegment2D::Arc {
                                start: [2.0, 2.0],
                                end: [4.0, 4.0],
                                center: [4.0, 2.0],
                                ccw: false,
                            },
                            SketchSegment2D::Line {
                                start: [4.0, 4.0],
                                end: [0.0, 4.0],
                            },
                            SketchSegment2D::Line {
                                start: [0.0, 4.0],
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
        let before = document.clone();

        assert!(matches!(
            document.apply(ModelCommand::SetSketchConstraints {
                id: sketch,
                constraints: vec![Constraint::CurvatureContinuous {
                    first: 0,
                    second: 1,
                }],
            }),
            Err(DocumentError::SketchConstraint(diagnostic))
                if diagnostic.reason == SketchConstraintFailureReason::Conflict
                    && diagnostic.constraint_indices == vec![0]
        ));
        assert_eq!(document, before);
    }

    #[test]
    fn exact_regions_reject_radius_mismatch_and_disconnected_segments() {
        let invalid_profiles = [
            SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Arc {
                        start: [1.0, 0.0],
                        end: [-2.0, 0.0],
                        center: [0.0, 0.0],
                        ccw: true,
                    },
                    SketchSegment2D::Line {
                        start: [-2.0, 0.0],
                        end: [1.0, 0.0],
                    },
                ],
            },
            SketchLoop2D {
                segments: vec![
                    SketchSegment2D::Line {
                        start: [0.0, 0.0],
                        end: [2.0, 0.0],
                    },
                    SketchSegment2D::Line {
                        start: [3.0, 0.0],
                        end: [0.0, 2.0],
                    },
                    SketchSegment2D::Line {
                        start: [0.0, 2.0],
                        end: [0.0, 0.0],
                    },
                ],
            },
        ];

        for profile in invalid_profiles {
            let mut document = CadDocument::default();
            assert!(matches!(
                document.apply(ModelCommand::CreateSketchRegion {
                    plane: SketchPlane::default(),
                    name: "invalid exact region".into(),
                    region: SketchRegion2D {
                        profile,
                        holes: Vec::new(),
                    },
                    construction: Vec::new(),
                    constraints: Vec::new(),
                    position: [0.0; 3],
                }),
                Err(DocumentError::InvalidParameter(_))
            ));
            assert!(document.features.is_empty());
        }
    }

    #[test]
    fn conflicting_sketch_constraints_are_rejected_atomically() {
        let mut document = CadDocument::default();
        let result = document.apply_transaction([
            ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "invalid".into(),
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]],
                holes: Vec::new(),
                constraints: vec![
                    Constraint::Fixed {
                        point: 0,
                        x: 0.0,
                        y: 0.0,
                    },
                    Constraint::Fixed {
                        point: 0,
                        x: 10.0,
                        y: 10.0,
                    },
                ],
                position: [0.0; 3],
            },
            ModelCommand::CreateBox {
                name: "never committed".into(),
                size: [1.0; 3],
                position: [0.0; 3],
            },
        ]);
        assert!(matches!(
            result,
            Err(DocumentError::SketchConstraint(diagnostic))
                if diagnostic.reason == SketchConstraintFailureReason::Conflict
                    && diagnostic.constraint_indices == vec![0, 1]
        ));
        assert!(document.features.is_empty());
    }

    #[test]
    fn datum_plane_persists_a_face_dependency_and_blocks_source_deletion() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "mounting block".into(),
                size: [20.0, 12.0, 6.0],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let face = FaceRef::primitive(source, crate::topology::PrimitiveFace::BoxZMax);
        let datum = document
            .apply(ModelCommand::CreateDatumPlane {
                name: "top datum".into(),
                face: face.clone(),
                offset: 2.5,
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            document.feature_graph().unwrap().dependencies(datum),
            Some(&[source][..])
        );
        assert!(matches!(
            &document.feature(datum).unwrap().primitive,
            Primitive::DatumPlane { face: actual, offset }
                if actual == &face && (*offset - 2.5).abs() < f64::EPSILON
        ));
        document
            .apply(ModelCommand::SetDatumPlaneOffset {
                id: datum,
                offset: -1.0,
            })
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: source }),
            Err(DocumentError::FeatureInUse { id, dependent }) if id == source && dependent == datum
        ));
    }

    #[test]
    fn datum_attached_sketches_are_parametric_and_reject_dependency_cycles() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "fixture".into(),
                size: [20.0, 12.0, 6.0],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let datum = document
            .apply(ModelCommand::CreateDatumPlane {
                name: "top datum".into(),
                face: FaceRef::primitive(source, crate::topology::PrimitiveFace::BoxZMax),
                offset: 0.0,
            })
            .unwrap()
            .unwrap();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                name: "attached profile".into(),
                plane: SketchPlane::DatumPlane { datum_id: datum },
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [1.0, 2.0, 0.0],
            })
            .unwrap()
            .unwrap();
        let pad = document
            .apply(ModelCommand::CreateExtrusionFromSketch {
                name: "pad".into(),
                sketch_id: sketch,
                height: 5.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let graph = document.feature_graph().unwrap();
        assert_eq!(graph.dependencies(sketch), Some(&[datum][..]));
        assert_eq!(graph.dependencies(pad), Some(&[sketch][..]));
        assert_eq!(graph.transitive_dependents(source), [datum, sketch, pad]);

        let result_datum = document
            .apply(ModelCommand::CreateDatumPlane {
                name: "result datum".into(),
                face: FaceRef::primitive(pad, crate::topology::PrimitiveFace::EndCap),
                offset: 0.0,
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::SetSketchPlane {
                id: sketch,
                plane: SketchPlane::DatumPlane {
                    datum_id: result_datum,
                },
            }),
            Err(DocumentError::DependencyCycle { .. })
        ));
        assert!(matches!(
            document.feature(sketch).unwrap().primitive,
            Primitive::Sketch {
                plane: SketchPlane::DatumPlane { datum_id },
                ..
            } if datum_id == datum
        ));
        assert!(matches!(
            document.apply(ModelCommand::Rotate {
                id: sketch,
                rotation: [10.0, 0.0, 0.0],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
    }

    #[test]
    fn face_attached_sketches_keep_a_direct_solid_dependency() {
        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "housing".into(),
                size: [20.0, 12.0, 6.0],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let face = FaceRef::primitive(source, crate::topology::PrimitiveFace::BoxYMax);
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                name: "side profile".into(),
                plane: SketchPlane::PlanarFace { face: face.clone() },
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            document.feature_graph().unwrap().dependencies(sketch),
            Some(&[source][..])
        );
        assert!(matches!(
            &document.feature(sketch).unwrap().primitive,
            Primitive::Sketch {
                plane: SketchPlane::PlanarFace { face: actual },
                ..
            } if actual == &face
        ));
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: source }),
            Err(DocumentError::FeatureInUse { id, dependent })
                if id == source && dependent == sketch
        ));

        let invalid_face =
            FaceRef::primitive(sketch, crate::topology::PrimitiveFace::Patch { index: 0 });
        assert!(matches!(
            document.apply(ModelCommand::SetSketchPlane {
                id: sketch,
                plane: SketchPlane::PlanarFace { face: invalid_face },
            }),
            Err(DocumentError::PrimitiveMismatch {
                id,
                expected: "solid feature"
            }) if id == sketch
        ));
    }

    #[test]
    fn datum_plane_rejects_sketch_and_invalid_face_references() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "profile".into(),
                profile: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 8.0], [0.0, 8.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let sketch_face =
            FaceRef::primitive(sketch, crate::topology::PrimitiveFace::Patch { index: 0 });
        assert!(matches!(
            document.apply(ModelCommand::CreateDatumPlane {
                name: "invalid".into(),
                face: sketch_face,
                offset: 0.0,
            }),
            Err(DocumentError::PrimitiveMismatch { id, expected: "solid feature" }) if id == sketch
        ));
        assert!(
            document
                .apply(ModelCommand::CreateDatumPlane {
                    name: "missing".into(),
                    face: FaceRef::primitive(
                        99,
                        crate::topology::PrimitiveFace::Patch { index: 0 }
                    ),
                    offset: 0.0,
                })
                .is_err()
        );
    }

    #[test]
    fn datum_point_persists_a_vertex_dependency_and_validates_offsets() {
        use crate::topology::{EdgeRef, PrimitiveFace, VertexRef};

        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "mounting block".into(),
                size: [20.0, 12.0, 6.0],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let vertex = VertexRef::new(
            source,
            vec![EdgeRef::new(
                source,
                FaceRef::primitive(source, PrimitiveFace::BoxXMin),
                FaceRef::primitive(source, PrimitiveFace::BoxYMin),
                0,
            )],
            0,
        );
        let datum = document
            .apply(ModelCommand::CreateDatumPoint {
                name: "setup origin".into(),
                vertex: vertex.clone(),
                offset: [1.0, 2.0, 3.0],
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            document.feature_graph().unwrap().dependencies(datum),
            Some(&[source][..])
        );
        assert!(matches!(
            &document.feature(datum).unwrap().primitive,
            Primitive::DatumPoint { vertex: actual, offset }
                if actual == &vertex && offset.as_array().iter().zip([1.0, 2.0, 3.0])
                    .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
        ));
        document
            .apply(ModelCommand::SetDatumPointOffset {
                id: datum,
                offset: [-1.0, 4.0, 0.5],
            })
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::SetDatumPointOffset {
                id: datum,
                offset: [f64::NAN, 0.0, 0.0],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: source }),
            Err(DocumentError::FeatureInUse { id, dependent }) if id == source && dependent == datum
        ));
        assert!(matches!(
            document.apply(ModelCommand::SetMaterial {
                id: datum,
                name: "Steel".into(),
                density_kg_m3: 7_850.0,
            }),
            Err(DocumentError::PrimitiveMismatch { id, expected: "solid feature" }) if id == datum
        ));
    }

    #[test]
    fn datum_point_rejects_non_canonical_vertex_references() {
        use crate::topology::{EdgeRef, PrimitiveFace, VertexRef};

        let mut document = CadDocument::default();
        let source = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let foreign_edge = EdgeRef::new(
            source + 1,
            FaceRef::primitive(source + 1, PrimitiveFace::BoxXMin),
            FaceRef::primitive(source + 1, PrimitiveFace::BoxYMin),
            0,
        );
        assert!(matches!(
            document.apply(ModelCommand::CreateDatumPoint {
                name: "invalid".into(),
                vertex: VertexRef::new(source, vec![foreign_edge], 0),
                offset: [0.0; 3],
            }),
            Err(DocumentError::InvalidParameter(_))
        ));
    }

    #[test]
    fn feature_graph_orders_dependencies_and_reports_impact() {
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
            .apply(ModelCommand::CreateCylinder {
                name: "right".into(),
                radius: 4.0,
                height: 10.0,
                position: [2.0, 2.0, 0.0],
            })
            .unwrap()
            .unwrap();
        let result = document
            .apply(ModelCommand::CreateBoolean {
                name: "union".into(),
                operation: BooleanOperation::Union,
                left,
                right,
            })
            .unwrap()
            .unwrap();
        let graph = document.feature_graph().unwrap();
        assert_eq!(graph.order(), &[left, right, result]);
        assert_eq!(graph.dependencies(result), Some(&[left, right][..]));
        assert_eq!(graph.dependents(left), &[result]);
        assert_eq!(graph.transitive_dependents(left), vec![result]);
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: left }),
            Err(DocumentError::FeatureInUse { id, dependent }) if id == left && dependent == result
        ));
    }

    #[test]
    fn chamfer_keeps_a_persistent_edge_dependency_and_validates_distance() {
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
            FaceRef::primitive(body, crate::topology::PrimitiveFace::BoxXMax),
            FaceRef::primitive(body, crate::topology::PrimitiveFace::BoxZMax),
            0,
        );
        let chamfer = document
            .apply(ModelCommand::CreateChamfer {
                name: "edge break".into(),
                edges: vec![edge.clone()],
                distance: 1.0,
            })
            .unwrap()
            .unwrap();

        assert!(!document.feature(body).unwrap().visible);
        assert_eq!(
            document.feature_graph().unwrap().dependencies(chamfer),
            Some(&[body][..])
        );
        assert!(matches!(
            &document.feature(chamfer).unwrap().primitive,
            Primitive::Chamfer { edges, distance }
                if edges == &[edge] && (*distance - 1.0).abs() < f64::EPSILON
        ));
        document
            .apply(ModelCommand::SetChamferDistance {
                id: chamfer,
                distance: 2.0,
            })
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: body }),
            Err(DocumentError::FeatureInUse { id, dependent })
                if id == body && dependent == chamfer
        ));
        assert!(
            document
                .apply(ModelCommand::SetChamferDistance {
                    id: chamfer,
                    distance: 0.0,
                })
                .is_err()
        );
    }

    #[test]
    fn fillet_keeps_a_persistent_edge_dependency_and_validates_radius() {
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
            FaceRef::primitive(body, crate::topology::PrimitiveFace::BoxXMax),
            FaceRef::primitive(body, crate::topology::PrimitiveFace::BoxZMax),
            0,
        );
        let fillet = document
            .apply(ModelCommand::CreateFillet {
                name: "round".into(),
                edges: vec![edge.clone()],
                radius: 1.0,
            })
            .unwrap()
            .unwrap();

        assert!(!document.feature(body).unwrap().visible);
        assert_eq!(
            document.feature_graph().unwrap().dependencies(fillet),
            Some(&[body][..])
        );
        assert!(matches!(
            &document.feature(fillet).unwrap().primitive,
            Primitive::Fillet { edges, radius }
                if edges == &[edge] && (*radius - 1.0).abs() < f64::EPSILON
        ));
        document
            .apply(ModelCommand::SetFilletRadius {
                id: fillet,
                radius: 2.0,
            })
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: body }),
            Err(DocumentError::FeatureInUse { id, dependent })
                if id == body && dependent == fillet
        ));
        assert!(
            document
                .apply(ModelCommand::SetFilletRadius {
                    id: fillet,
                    radius: 0.0,
                })
                .is_err()
        );
    }

    #[test]
    fn multi_edge_modifiers_canonicalize_one_source_and_reject_mixed_sources() {
        let mut document = CadDocument::default();
        let bodies = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "first".into(),
                    size: [10.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "second".into(),
                    size: [10.0; 3],
                    position: [20.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        let edge = |body, first, second| {
            EdgeRef::new(
                body,
                FaceRef::primitive(body, first),
                FaceRef::primitive(body, second),
                0,
            )
        };
        let first = edge(
            bodies[0],
            crate::topology::PrimitiveFace::BoxXMax,
            crate::topology::PrimitiveFace::BoxZMax,
        );
        let second = edge(
            bodies[0],
            crate::topology::PrimitiveFace::BoxXMin,
            crate::topology::PrimitiveFace::BoxZMin,
        );
        let chamfer = document
            .apply(ModelCommand::CreateChamfer {
                name: "two edges".into(),
                edges: vec![second.clone(), first.clone(), second.clone()],
                distance: 1.0,
            })
            .unwrap()
            .unwrap();
        let Primitive::Chamfer { edges, .. } = &document.feature(chamfer).unwrap().primitive else {
            panic!("expected chamfer");
        };
        assert_eq!(edges, &[second, first]);
        assert_eq!(
            document.feature_graph().unwrap().dependencies(chamfer),
            Some(&[bodies[0]][..])
        );

        let mixed = vec![
            edge(
                bodies[0],
                crate::topology::PrimitiveFace::BoxXMax,
                crate::topology::PrimitiveFace::BoxYMax,
            ),
            edge(
                bodies[1],
                crate::topology::PrimitiveFace::BoxXMax,
                crate::topology::PrimitiveFace::BoxYMax,
            ),
        ];
        assert!(matches!(
            document.apply(ModelCommand::CreateFillet {
                name: "invalid".into(),
                edges: mixed,
                radius: 1.0,
            }),
            Err(DocumentError::InvalidParameter(message))
                if message.contains("one non-zero source feature")
        ));
        assert!(
            document
                .apply(ModelCommand::CreateFillet {
                    name: "empty".into(),
                    edges: Vec::new(),
                    radius: 1.0,
                })
                .is_err()
        );
    }

    #[test]
    fn feature_graph_rejects_boolean_sketch_operands() {
        let mut document = CadDocument::default();
        let sketch = document
            .apply(ModelCommand::CreateSketch {
                plane: SketchPlane::default(),
                name: "profile".into(),
                profile: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]],
                holes: Vec::new(),
                constraints: Vec::new(),
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let box_id = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [2.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            document.apply(ModelCommand::CreateBoolean {
                name: "invalid".into(),
                operation: BooleanOperation::Intersect,
                left: sketch,
                right: box_id,
            }),
            Err(DocumentError::PrimitiveMismatch { id, expected: "solid feature" }) if id == sketch
        ));
    }

    #[test]
    fn feature_graph_rejects_cycles_from_persisted_data() {
        let mut document = CadDocument {
            name: "cycle".into(),
            features: vec![
                Feature {
                    id: 1,
                    name: "a".into(),
                    primitive: Primitive::Boolean {
                        operation: BooleanOperation::Union,
                        left: 2,
                        right: 2,
                    },
                    translation: Vec3::ZERO,
                    rotation: Vec3::ZERO,
                    visible: true,
                    color: [0.5; 4],
                    material: None,
                },
                Feature {
                    id: 2,
                    name: "b".into(),
                    primitive: Primitive::Boolean {
                        operation: BooleanOperation::Union,
                        left: 1,
                        right: 1,
                    },
                    translation: Vec3::ZERO,
                    rotation: Vec3::ZERO,
                    visible: true,
                    color: [0.5; 4],
                    material: None,
                },
            ],
            assemblies: Vec::new(),
            next_id: 3,
            next_assembly_id: 1,
        };
        assert!(matches!(
            document.validate_and_repair(),
            Err(DocumentError::InvalidParameter(_))
        ));

        document.features[0].primitive = Primitive::Boolean {
            operation: BooleanOperation::Union,
            left: 2,
            right: 1,
        };
        document.features[1].primitive = Primitive::Boolean {
            operation: BooleanOperation::Union,
            left: 1,
            right: 2,
        };
        assert!(matches!(
            document.validate_and_repair(),
            Err(DocumentError::DependencyCycle { cycle }) if cycle == vec![1, 2]
        ));
    }

    #[test]
    fn imported_step_units_are_validated_before_commit() {
        let mut document = CadDocument::default();
        let command = |length_unit| ModelCommand::ImportStep {
            name: "supplier body".into(),
            source: "ISO-10303-21; DATA; #1=CLOSED_SHELL('',(#2)); ENDSEC; END-ISO-10303-21;"
                .into(),
            data_section: 0,
            shell_id: 1,
            void_shells: Vec::new(),
            length_unit,
            color: None,
            position: [0.0; 3],
        };
        assert!(
            document
                .apply(command(StepLengthUnit {
                    name: "inch".into(),
                    millimeters_per_unit: f64::NAN,
                    declared: true,
                }))
                .is_err()
        );
        assert!(
            document
                .apply(command(StepLengthUnit {
                    name: String::new(),
                    millimeters_per_unit: 25.4,
                    declared: true,
                }))
                .is_err()
        );
        assert!(
            document
                .apply(command(StepLengthUnit {
                    name: "inch".into(),
                    millimeters_per_unit: 25.4,
                    declared: false,
                }))
                .is_err()
        );
        assert!(document.features.is_empty());
    }

    #[test]
    fn imported_step_boundary_ids_are_validated_before_commit() {
        let command = |void_shells| ModelCommand::ImportStep {
            name: "hollow supplier body".into(),
            source: "ISO-10303-21; DATA; #1=CLOSED_SHELL('',(#2)); ENDSEC; END-ISO-10303-21;"
                .into(),
            data_section: 0,
            shell_id: 1,
            void_shells,
            length_unit: StepLengthUnit::millimeter(),
            color: None,
            position: [0.0; 3],
        };
        let mut document = CadDocument::default();
        for void_shells in [
            vec![StepShellBoundary {
                shell_id: 0,
                orientation: true,
            }],
            vec![StepShellBoundary {
                shell_id: 1,
                orientation: false,
            }],
            vec![
                StepShellBoundary {
                    shell_id: 2,
                    orientation: true,
                },
                StepShellBoundary {
                    shell_id: 2,
                    orientation: false,
                },
            ],
        ] {
            assert!(document.apply(command(void_shells)).is_err());
            assert!(document.features.is_empty());
        }
    }

    #[test]
    fn imported_step_color_is_applied_atomically() {
        let command = |color| ModelCommand::ImportStep {
            name: "painted supplier body".into(),
            source: "ISO-10303-21; DATA; #1=CLOSED_SHELL('',(#2)); ENDSEC; END-ISO-10303-21;"
                .into(),
            data_section: 0,
            shell_id: 1,
            void_shells: Vec::new(),
            length_unit: StepLengthUnit::millimeter(),
            color,
            position: [0.0; 3],
        };
        let mut document = CadDocument::default();
        assert!(document.apply(command(Some([1.1, 0.2, 0.3, 1.0]))).is_err());
        assert!(document.features.is_empty());

        let color = [0.1, 0.2, 0.8, 0.75];
        let id = document.apply(command(Some(color))).unwrap().unwrap();
        assert!(
            document
                .feature(id)
                .unwrap()
                .color
                .into_iter()
                .zip(color)
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn assembly_occurrences_reuse_definitions_and_own_concrete_features() {
        use crate::assembly::{
            AssemblyTransform, ComponentDefinition, ComponentKind, ComponentOccurrence,
        };

        let mut document = CadDocument::default();
        let feature_ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "left fastener".into(),
                    size: [2.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "right fastener".into(),
                    size: [2.0; 3],
                    position: [10.0, 0.0, 0.0],
                },
            ])
            .unwrap();
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
                        name: "fastener".into(),
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
                        name: "fastener 1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: vec![feature_ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "fastener 2".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [10.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![feature_ids[1]],
                        source: None,
                    },
                ],
            })
            .unwrap();

        let assembly = document.assembly(1).unwrap();
        assert_eq!(assembly.definitions.len(), 2);
        assert_eq!(assembly.children(1).count(), 2);
        assert_eq!(
            document.assembly_feature_instance(feature_ids[1]),
            Some(crate::assembly::AssemblyFeatureInstance {
                assembly_id: 1,
                definition_id: 2,
                occurrence_id: 3,
                body_slot: 0,
            })
        );
        assert_eq!(document.assembly_feature_instance(999), None);
        let instances = document.assembly_feature_instances();
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[&feature_ids[0]].occurrence_id, 2);
        for command in [
            ModelCommand::Move {
                id: feature_ids[0],
                position: [1.0, 2.0, 3.0],
            },
            ModelCommand::Rotate {
                id: feature_ids[0],
                rotation: [10.0, 20.0, 30.0],
            },
        ] {
            assert!(matches!(
                document.apply(command),
                Err(DocumentError::FeatureInAssembly {
                    assembly: 1,
                    occurrence: 2,
                    ..
                })
            ));
        }
        assert!(matches!(
            document.apply(ModelCommand::Delete { id: feature_ids[0] }),
            Err(DocumentError::FeatureInAssembly {
                assembly: 1,
                occurrence: 2,
                ..
            })
        ));
        document
            .apply(ModelCommand::DeleteAssembly { id: 1 })
            .unwrap();
        document
            .apply(ModelCommand::Delete { id: feature_ids[0] })
            .unwrap();
    }

    #[test]
    fn occurrence_transform_updates_descendant_bodies_atomically() {
        use crate::assembly::{
            AssemblyTransform, ComponentDefinition, ComponentKind, ComponentOccurrence,
        };

        let mut document = CadDocument::default();
        let feature_ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "subassembly body".into(),
                    size: [2.0; 3],
                    position: [10.0, 0.0, 0.0],
                },
                ModelCommand::CreateBox {
                    name: "nested body".into(),
                    size: [1.0; 3],
                    position: [12.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "nested fixture".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "fixture".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "carriage".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 3,
                        name: "pin".into(),
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
                        name: "carriage:1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [10.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![feature_ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "pin:1".into(),
                        definition_id: 3,
                        parent_id: Some(2),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [2.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![feature_ids[1]],
                        source: None,
                    },
                ],
            })
            .unwrap();

        document
            .apply(ModelCommand::SetOccurrenceTransform {
                assembly_id: 1,
                occurrence_id: 2,
                position: [20.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 90.0],
            })
            .unwrap();
        for (feature_id, expected_position) in [
            (feature_ids[0], [20.0, 0.0, 0.0]),
            (feature_ids[1], [20.0, 2.0, 0.0]),
        ] {
            let feature = document.feature(feature_id).unwrap();
            assert!(
                feature
                    .translation
                    .as_array()
                    .into_iter()
                    .zip(expected_position)
                    .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
            );
            assert!((feature.rotation.z - 90.0).abs() < 1.0e-9);
        }
        let expected_local =
            AssemblyTransform::from_euler_xyz_degrees([20.0, 0.0, 0.0], [0.0, 0.0, 90.0]);
        assert!(
            document
                .assembly(1)
                .unwrap()
                .occurrence(2)
                .unwrap()
                .transform
                .approximately_equals(expected_local, 1.0e-9)
        );

        let committed = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::SetOccurrenceTransform {
                assembly_id: 1,
                occurrence_id: 2,
                position: [f64::NAN, 0.0, 0.0],
                rotation: [0.0; 3],
            }),
            Err(DocumentError::Assembly(AssemblyError::NonFiniteTransform))
        ));
        assert_eq!(document, committed);
        assert!(matches!(
            document.apply(ModelCommand::SetOccurrenceTransform {
                assembly_id: 1,
                occurrence_id: 99,
                position: [0.0; 3],
                rotation: [0.0; 3],
            }),
            Err(DocumentError::OccurrenceNotFound {
                assembly: 1,
                occurrence: 99
            })
        ));
        assert_eq!(document, committed);
    }

    #[test]
    fn assembly_mate_commands_drive_descendants_and_fail_atomically() {
        use crate::assembly::{
            AssemblyMate, AssemblyMateKind, AssemblyMateLimits, AssemblyTransform,
            ComponentDefinition, ComponentKind, ComponentOccurrence,
        };

        let mut document = CadDocument::default();
        let feature_ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "base".into(),
                    size: [4.0; 3],
                    position: [5.0, 0.0, 0.0],
                },
                ModelCommand::CreateBox {
                    name: "arm".into(),
                    size: [3.0; 3],
                    position: [15.0, 0.0, 0.0],
                },
                ModelCommand::CreateBox {
                    name: "tool".into(),
                    size: [2.0; 3],
                    position: [17.0, 0.0, 0.0],
                },
            ])
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "robot".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "base".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "arm".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 3,
                        name: "tool".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "base:1".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [5.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![feature_ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "arm:1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [10.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![feature_ids[1]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "tool:1".into(),
                        definition_id: 3,
                        parent_id: Some(2),
                        suppressed: true,
                        transform: AssemblyTransform {
                            translation: [2.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![feature_ids[2]],
                        source: None,
                    },
                ],
            })
            .unwrap();

        document
            .apply(ModelCommand::CreateAssemblyMate {
                assembly_id: 1,
                mate: AssemblyMate {
                    id: 1,
                    name: "shoulder".into(),
                    parent_occurrence_id: 1,
                    child_occurrence_id: 2,
                    parent_frame: AssemblyTransform {
                        translation: [10.0, 0.0, 0.0],
                        ..AssemblyTransform::IDENTITY
                    },
                    child_frame: AssemblyTransform::IDENTITY,
                    kind: AssemblyMateKind::Revolute {
                        axis: [0.0, 0.0, 1.0],
                        limits_deg: Some(AssemblyMateLimits {
                            min: -90.0,
                            max: 90.0,
                        }),
                    },
                    state: 0.0,
                },
            })
            .unwrap();
        document
            .apply(ModelCommand::SetAssemblyMateState {
                assembly_id: 1,
                mate_id: 1,
                state: 90.0,
            })
            .unwrap();

        let arm = document.feature(feature_ids[1]).unwrap().clone();
        let tool = document.feature(feature_ids[2]).unwrap().clone();
        assert!(
            arm.translation
                .as_array()
                .into_iter()
                .zip([15.0, 0.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        assert!((arm.rotation.z - 90.0).abs() < 1.0e-9);
        assert!(
            tool.translation
                .as_array()
                .into_iter()
                .zip([15.0, 2.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        );
        assert!((tool.rotation.z - 90.0).abs() < 1.0e-9);
        assert!(
            document
                .assembly(1)
                .unwrap()
                .occurrence(3)
                .unwrap()
                .suppressed
        );

        let committed = document.clone();
        assert!(matches!(
            document.apply(ModelCommand::SetOccurrenceTransform {
                assembly_id: 1,
                occurrence_id: 2,
                position: [0.0; 3],
                rotation: [0.0; 3],
            }),
            Err(DocumentError::MateDrivenOccurrence {
                assembly: 1,
                occurrence: 2,
                mate: 1
            })
        ));
        assert_eq!(document, committed);
        assert!(matches!(
            document.apply(ModelCommand::SetAssemblyMateState {
                assembly_id: 1,
                mate_id: 1,
                state: 91.0,
            }),
            Err(DocumentError::Assembly(
                AssemblyError::MateStateOutsideLimits { mate: 1 }
            ))
        ));
        assert_eq!(document, committed);

        document
            .apply(ModelCommand::DeleteAssemblyMate {
                assembly_id: 1,
                mate_id: 1,
            })
            .unwrap();
        assert!(document.assembly(1).unwrap().mates.is_empty());
        assert_eq!(document.feature(feature_ids[1]).unwrap(), &arm);
        assert_eq!(document.feature(feature_ids[2]).unwrap(), &tool);
        document
            .apply(ModelCommand::SetOccurrenceTransform {
                assembly_id: 1,
                occurrence_id: 2,
                position: [20.0, 0.0, 0.0],
                rotation: [0.0; 3],
            })
            .unwrap();
    }

    #[test]
    fn occurrence_suppression_is_inherited_and_dependency_safe() {
        use crate::assembly::{
            AssemblyTransform, ComponentDefinition, ComponentKind, ComponentOccurrence,
        };

        let mut document = CadDocument::default();
        let ids = document
            .apply_transaction([
                ModelCommand::CreateBox {
                    name: "carriage".into(),
                    size: [4.0; 3],
                    position: [0.0; 3],
                },
                ModelCommand::CreateBox {
                    name: "pin".into(),
                    size: [2.0; 3],
                    position: [6.0, 0.0, 0.0],
                },
                ModelCommand::CreateBox {
                    name: "tool".into(),
                    size: [2.0; 3],
                    position: [8.0, 0.0, 0.0],
                },
            ])
            .unwrap();
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
                        name: "carriage".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 3,
                        name: "pin".into(),
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
                        name: "carriage:1".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: vec![ids[0]],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "pin:1".into(),
                        definition_id: 3,
                        parent_id: Some(2),
                        suppressed: false,
                        transform: AssemblyTransform {
                            translation: [6.0, 0.0, 0.0],
                            ..AssemblyTransform::IDENTITY
                        },
                        feature_ids: vec![ids[1]],
                        source: None,
                    },
                ],
            })
            .unwrap();
        let dependent = document
            .apply(ModelCommand::CreateBoolean {
                name: "dependent".into(),
                operation: BooleanOperation::Union,
                left: ids[1],
                right: ids[2],
            })
            .unwrap()
            .unwrap();

        let committed = document.clone();
        assert_eq!(
            document.apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 2,
                suppressed: true,
            }),
            Err(DocumentError::SuppressedFeatureDependency {
                feature: dependent,
                dependency: ids[1],
            })
        );
        assert_eq!(document, committed);

        document
            .apply(ModelCommand::Delete { id: dependent })
            .unwrap();
        let visibility = document
            .features
            .iter()
            .map(|feature| (feature.id, feature.visible))
            .collect::<Vec<_>>();
        document
            .apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 2,
                suppressed: true,
            })
            .unwrap();
        let assembly = document.assembly(1).unwrap();
        assert!(assembly.occurrence(2).unwrap().suppressed);
        assert!(!assembly.occurrence(3).unwrap().suppressed);
        assert_eq!(
            assembly.effective_suppression().unwrap(),
            [(1, false), (2, true), (3, true)].into_iter().collect()
        );
        assert_eq!(
            document.suppressed_assembly_feature_ids().unwrap(),
            [ids[0], ids[1]].into_iter().collect()
        );

        let suppressed_document = document.clone();
        let next_id = document.next_feature_id();
        assert_eq!(
            document.apply(ModelCommand::CreateBoolean {
                name: "invalid active dependent".into(),
                operation: BooleanOperation::Union,
                left: ids[0],
                right: ids[2],
            }),
            Err(DocumentError::SuppressedFeatureDependency {
                feature: next_id,
                dependency: ids[0],
            })
        );
        assert_eq!(document, suppressed_document);

        document
            .apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 3,
                suppressed: true,
            })
            .unwrap();
        document
            .apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 2,
                suppressed: false,
            })
            .unwrap();
        assert_eq!(
            document.suppressed_assembly_feature_ids().unwrap(),
            [ids[1]].into_iter().collect()
        );
        assert!(
            document
                .assembly(1)
                .unwrap()
                .occurrence(3)
                .unwrap()
                .suppressed
        );
        document
            .apply(ModelCommand::SetOccurrenceSuppressed {
                assembly_id: 1,
                occurrence_id: 3,
                suppressed: false,
            })
            .unwrap();
        assert!(
            document
                .suppressed_assembly_feature_ids()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            document
                .features
                .iter()
                .map(|feature| (feature.id, feature.visible))
                .collect::<Vec<_>>(),
            visibility
        );
    }

    #[test]
    fn invalid_assembly_hierarchy_is_rejected_atomically() {
        use crate::assembly::{
            AssemblyTransform, ComponentDefinition, ComponentKind, ComponentOccurrence,
        };

        let mut document = CadDocument::default();
        let body = document
            .apply(ModelCommand::CreateBox {
                name: "body".into(),
                size: [2.0; 3],
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let result = document.apply(ModelCommand::CreateAssembly {
            name: "cycle".into(),
            definitions: vec![ComponentDefinition {
                id: 1,
                name: "cycle".into(),
                kind: ComponentKind::Assembly,
                source: None,
            }],
            occurrences: vec![
                ComponentOccurrence {
                    id: 1,
                    name: "one".into(),
                    definition_id: 1,
                    parent_id: Some(2),
                    suppressed: false,
                    transform: AssemblyTransform::IDENTITY,
                    feature_ids: vec![body],
                    source: None,
                },
                ComponentOccurrence {
                    id: 2,
                    name: "two".into(),
                    definition_id: 1,
                    parent_id: Some(1),
                    suppressed: false,
                    transform: AssemblyTransform::IDENTITY,
                    feature_ids: Vec::new(),
                    source: None,
                },
            ],
        });
        assert!(matches!(
            result,
            Err(DocumentError::Assembly(
                AssemblyError::MissingRootOccurrence
            ))
        ));
        assert!(document.assemblies.is_empty());

        document
            .apply(ModelCommand::CreateAssembly {
                name: "valid".into(),
                definitions: vec![ComponentDefinition {
                    id: 1,
                    name: "body".into(),
                    kind: ComponentKind::Part,
                    source: None,
                }],
                occurrences: vec![ComponentOccurrence {
                    id: 1,
                    name: "body".into(),
                    definition_id: 1,
                    parent_id: None,
                    suppressed: false,
                    transform: AssemblyTransform::IDENTITY,
                    feature_ids: vec![body],
                    source: None,
                }],
            })
            .unwrap();
        assert!(document.assembly(1).is_some());
    }
}
