use std::{fmt, ops::Range};

use serde::{Deserialize, Serialize};

use crate::domain::FeatureId;

/// A stable semantic role assigned by the feature that generated a face.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum PrimitiveFace {
    BoxXMin,
    BoxXMax,
    BoxYMin,
    BoxYMax,
    BoxZMin,
    BoxZMax,
    StartCap,
    StartCapPatch { patch: u32 },
    EndCap,
    EndCapPatch { patch: u32 },
    Lateral,
    LateralPatch { patch: u32 },
    ProfileSide { segment: u32 },
    ProfileSidePatch { segment: u32, patch: u32 },
    HoleSide { hole: u32, segment: u32 },
    HoleSidePatch { hole: u32, segment: u32, patch: u32 },
    LoftSide { transition: u32, segment: u32 },
    Patch { index: u32 },
}

/// The persistent name of a face relative to its owning feature.
///
/// Boolean faces retain the reference of the upstream face that generated
/// them. `fragment` distinguishes pieces when an operation splits one source
/// face into multiple result faces.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum FaceName {
    Primitive {
        face: PrimitiveFace,
    },
    Derived {
        sources: Vec<FaceRef>,
        fragment: u32,
    },
}

/// A kernel-neutral, serializable reference to a topological face.
///
/// The reference never contains a kernel object id. It is resolved against a
/// freshly evaluated scene, so it remains usable after save/load and rebuilds
/// while the referenced generating topology still exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaceRef {
    pub feature_id: FeatureId,
    pub name: FaceName,
}

impl FaceRef {
    #[must_use]
    pub const fn primitive(feature_id: FeatureId, face: PrimitiveFace) -> Self {
        Self {
            feature_id,
            name: FaceName::Primitive { face },
        }
    }

    #[must_use]
    pub fn derived(feature_id: FeatureId, source: Self, fragment: u32) -> Self {
        Self::derived_from(feature_id, vec![source], fragment)
    }

    #[must_use]
    pub fn derived_from(feature_id: FeatureId, mut sources: Vec<Self>, fragment: u32) -> Self {
        sources.sort_unstable();
        sources.dedup();
        Self {
            feature_id,
            name: FaceName::Derived { sources, fragment },
        }
    }
}

impl fmt::Display for FaceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "feature:{}/", self.feature_id)?;
        match &self.name {
            FaceName::Primitive { face } => write!(formatter, "{face:?}"),
            FaceName::Derived { sources, fragment } => {
                write!(formatter, "from:(")?;
                for (index, source) in sources.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{source}")?;
                }
                write!(formatter, ")/fragment:{fragment}")
            }
        }
    }
}

/// A persistent edge reference derived from its two adjacent faces.
///
/// `fragment` distinguishes multiple edges shared by the same face pair. The
/// adjacent references are always stored in canonical order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EdgeRef {
    pub feature_id: FeatureId,
    pub adjacent_faces: [FaceRef; 2],
    pub fragment: u32,
}

impl EdgeRef {
    #[must_use]
    pub fn new(feature_id: FeatureId, first: FaceRef, second: FaceRef, fragment: u32) -> Self {
        let adjacent_faces = if first <= second {
            [first, second]
        } else {
            [second, first]
        };
        Self {
            feature_id,
            adjacent_faces,
            fragment,
        }
    }
}

impl fmt::Display for EdgeRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "feature:{}/between:({},{})/fragment:{}",
            self.feature_id, self.adjacent_faces[0], self.adjacent_faces[1], self.fragment
        )
    }
}

/// A persistent vertex reference derived from all incident persistent edges.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VertexRef {
    pub feature_id: FeatureId,
    pub incident_edges: Vec<EdgeRef>,
    pub fragment: u32,
}

impl VertexRef {
    #[must_use]
    pub fn new(feature_id: FeatureId, mut incident_edges: Vec<EdgeRef>, fragment: u32) -> Self {
        incident_edges.sort_unstable();
        incident_edges.dedup();
        Self {
            feature_id,
            incident_edges,
            fragment,
        }
    }
}

impl fmt::Display for VertexRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "feature:{}/incident:(", self.feature_id)?;
        for (index, edge) in self.incident_edges.iter().enumerate() {
            if index > 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{edge}")?;
        }
        write!(formatter, ")/fragment:{}", self.fragment)
    }
}

/// Explicit result of resolving a persistent topology reference.
///
/// Consumers must only act on `Resolved`; ambiguity and loss are not silently
/// converted to a nearest geometric entity.
#[derive(Debug, Clone, PartialEq)]
pub enum TopologyResolution<'a, T> {
    Resolved(&'a T),
    Ambiguous(Vec<&'a T>),
    Lost,
}

impl<'a, T> TopologyResolution<'a, T> {
    #[must_use]
    pub fn unique(self) -> Option<&'a T> {
        match self {
            Self::Resolved(value) => Some(value),
            Self::Ambiguous(_) | Self::Lost => None,
        }
    }
}

/// Kernel-neutral classification of the supporting geometry for a face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Swept,
}

/// Kernel-neutral equation of an analytic plane in model coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneGeometry {
    pub origin: [f64; 3],
    /// Unit in-plane direction derived from the supporting surface U axis.
    pub x_direction: [f64; 3],
    /// Unit in-plane direction completing a right-handed orthonormal frame.
    pub y_direction: [f64; 3],
    /// Unit normal. Its sign follows the supporting surface, not face winding.
    pub normal: [f64; 3],
}

/// Evaluated geometric properties of one topological face.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceGeometry {
    pub surface: SurfaceKind,
    /// Present only when the kernel can expose an analytic plane equation.
    pub plane: Option<PlaneGeometry>,
    pub area: f64,
    pub centroid: [f64; 3],
    /// Area-weighted mean normal. Curved closed faces may have a zero vector.
    pub mean_normal: [f64; 3],
}

/// Topology metadata for a contiguous range of triangles in an evaluated part.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedFace {
    pub reference: FaceRef,
    pub geometry: FaceGeometry,
    /// Triangle ordinals in `EvaluatedPart::mesh`, not raw index offsets.
    pub triangles: Range<u32>,
}

/// Kernel-neutral classification of a B-Rep edge's carrying curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveKind {
    Line,
    BSpline,
    Nurbs,
    Intersection,
}

/// Evaluated geometric properties of one topological edge.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeGeometry {
    pub curve: CurveKind,
    pub endpoints: [[f64; 3]; 2],
    pub midpoint: [f64; 3],
    pub length: f64,
    /// Estimated absolute integration error in model units. Zero denotes an
    /// analytic line length; `None` means only the display polyline was usable.
    pub length_error_estimate: Option<f64>,
    /// Ordered curve samples used for viewport display and picking.
    pub polyline: Vec<[f64; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedEdge {
    pub reference: EdgeRef,
    pub geometry: EdgeGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexGeometry {
    pub position: [f64; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedVertex {
    pub reference: VertexRef,
    pub geometry: VertexGeometry,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_face_reference_round_trips_without_kernel_ids() {
        let reference = FaceRef::derived(
            8,
            FaceRef::primitive(3, PrimitiveFace::ProfileSide { segment: 2 }),
            1,
        );
        let json = serde_json::to_string(&reference).unwrap();
        let decoded: FaceRef = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, reference);
        assert!(!json.contains("object_id"));
        assert!(!json.contains("pointer"));
    }

    #[test]
    fn edge_and_vertex_references_are_canonical_and_serializable() {
        let left = FaceRef::primitive(4, PrimitiveFace::BoxXMin);
        let top = FaceRef::primitive(4, PrimitiveFace::BoxZMax);
        let edge = EdgeRef::new(4, top.clone(), left.clone(), 0);
        assert_eq!(edge.adjacent_faces, [left, top]);
        let vertex = VertexRef::new(4, vec![edge.clone(), edge.clone()], 0);
        assert_eq!(vertex.incident_edges, vec![edge]);

        let json = serde_json::to_string(&vertex).unwrap();
        let decoded: VertexRef = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, vertex);
        assert!(!json.contains("object_id"));
        assert!(!json.contains("pointer"));
    }
}
