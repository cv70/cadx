use crate::assembly::{AssemblyDefinitionBody, AssemblyTransform};
use crate::diagnostics::{BooleanDiagnostic, EdgeModifierDiagnostic, EdgeModifierOperation};
use crate::domain::{CadDocument, DocumentError, FeatureId, Material};
use crate::topology::{
    EdgeRef, EvaluatedEdge, EvaluatedFace, EvaluatedVertex, FaceRef, TopologyResolution, VertexRef,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use cadx_sketch::{
    SketchAnnotationGeometry2D, SketchConstraintAnnotation2D, SketchSolveDiagnostic,
    constraint_annotations,
};

/// Support level for selections whose edges meet at a vertex.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedVertexSupport {
    #[default]
    Unsupported,
    ConvexPolyhedralSource,
    Supported,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeCountSupport {
    #[default]
    Unsupported,
    Single,
    Multiple,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFeatureScope {
    #[default]
    Single,
    Multiple,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeCurveSupport {
    #[default]
    LinearOnly,
    Any,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportSurfaceSupport {
    #[default]
    PlanarOnly,
    Any,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeConvexitySupport {
    #[default]
    ConvexOnly,
    Any,
}

/// Kernel-declared contract for one edge-modifier operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeModifierCapability {
    pub edge_count: EdgeCountSupport,
    pub source_scope: SourceFeatureScope,
    pub edge_curves: EdgeCurveSupport,
    pub support_surfaces: SupportSurfaceSupport,
    pub edge_convexity: EdgeConvexitySupport,
    pub shared_vertex_support: SharedVertexSupport,
}

/// Kernel-neutral capabilities needed by application, UI, and AI consumers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CadKernelCapabilities {
    pub chamfer: EdgeModifierCapability,
    pub fillet: EdgeModifierCapability,
    /// Exact B-Rep intersection analysis with kernel-declared volume precision.
    #[serde(default)]
    pub interference_analysis: bool,
}

impl CadKernelCapabilities {
    #[must_use]
    pub const fn edge_modifier(self, operation: EdgeModifierOperation) -> EdgeModifierCapability {
        match operation {
            EdgeModifierOperation::Chamfer => self.chamfer,
            EdgeModifierOperation::Fillet => self.fillet,
        }
    }
}

/// Precision evidence attached to interference-volume results.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InterferenceVolumePrecision {
    /// Volume integrated from a tessellation of the exact B-Rep intersection.
    Tessellated { chord_tolerance_mm: f64 },
}

/// Method that established the exact intersection topology for one pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterferenceIntersectionMethod {
    BrepBoolean,
    NonCrossingContact,
    BoundaryClassifiedContainment,
}

/// Stable phase at which one candidate-pair analysis failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterferenceFailureStage {
    KernelOperation,
    ResultValidation,
    VolumeIntegration,
}

/// Stable reason code for one candidate-pair analysis failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterferenceFailureReason {
    KernelRejected,
    KernelPanic,
    InvalidResultTopology,
    EmptyMesh,
    NonFiniteVolume,
}

/// Exact-intersection result for one pair that survived broad-phase culling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InterferencePairOutcome {
    Clear {
        volume_mm3: f64,
        precision: InterferenceVolumePrecision,
        method: InterferenceIntersectionMethod,
    },
    Interfering {
        volume_mm3: f64,
        precision: InterferenceVolumePrecision,
        method: InterferenceIntersectionMethod,
    },
    Failed {
        stage: InterferenceFailureStage,
        reason: InterferenceFailureReason,
        detail: String,
    },
}

/// Evidence for one deterministic broad-phase candidate pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterferencePairAnalysis {
    pub feature_ids: [FeatureId; 2],
    pub bounds: [crate::diagnostics::AxisAlignedBounds; 2],
    pub outcome: InterferencePairOutcome,
}

/// Read-only product-level interference report for one document revision.
///
/// `pairs` contains only AABB-overlapping candidates. `clear_pair_count` also
/// includes pairs proven clear by disjoint bounds, keeping report memory
/// proportional to broad-phase hits instead of every possible pair.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InterferenceAnalysis {
    pub candidate_feature_ids: Vec<FeatureId>,
    pub total_pair_count: u64,
    pub broad_phase_pair_count: u64,
    pub clear_pair_count: u64,
    pub interfering_pair_count: u64,
    pub failed_pair_count: u64,
    /// Volumes at or below this modeling-scale threshold are classified clear.
    pub volume_tolerance_mm3: f64,
    pub pairs: Vec<InterferencePairAnalysis>,
}

impl InterferenceAnalysis {
    #[must_use]
    pub const fn has_interference(&self) -> bool {
        self.interfering_pair_count > 0
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.failed_pair_count == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriangleMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl TriangleMesh {
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedPart {
    pub feature_id: FeatureId,
    pub name: String,
    pub color: [f32; 4],
    pub material: Option<Material>,
    pub mesh: TriangleMesh,
    pub faces: Vec<EvaluatedFace>,
    pub edges: Vec<EvaluatedEdge>,
    pub vertices: Vec<EvaluatedVertex>,
}

/// One component-local triangle mesh reusable by multiple render instances.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedMeshDefinition {
    pub key: AssemblyDefinitionBody,
    pub mesh: TriangleMesh,
}

/// One placed use of an evaluated reusable render mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvaluatedMeshInstance {
    pub feature_id: FeatureId,
    pub definition: AssemblyDefinitionBody,
    pub transform: AssemblyTransform,
}

/// A resolved, non-solid datum plane ready for visualization and downstream use.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedDatumPlane {
    pub feature_id: FeatureId,
    pub name: String,
    pub color: [f32; 4],
    pub face: FaceRef,
    pub origin: [f64; 3],
    /// Unit local X direction inherited from the analytic supporting plane.
    pub x_direction: [f64; 3],
    /// Unit local Y direction; X cross Y equals the oriented face normal.
    pub y_direction: [f64; 3],
    /// Unit normal whose sign follows the oriented source face.
    pub normal: [f64; 3],
}

/// A resolved, non-solid datum point ready for visualization and downstream use.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedDatumPoint {
    pub feature_id: FeatureId,
    pub name: String,
    pub color: [f32; 4],
    pub vertex: VertexRef,
    pub position: [f64; 3],
}

/// A solved, non-solid sketch profile mapped into model coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedSketch {
    pub feature_id: FeatureId,
    pub name: String,
    pub color: [f32; 4],
    /// Ordered display metadata derived from the committed solved geometry.
    /// It contains no screen coordinates or UI state.
    pub constraint_annotations: Vec<SketchConstraintAnnotation2D>,
    /// Closed display polyline sampled from the exact profile; the first point
    /// is not repeated. Persistent geometry remains in the document.
    pub profile: Vec<[f64; 3]>,
    /// Sampled interior display loops; first points are not repeated.
    pub holes: Vec<Vec<[f64; 3]>>,
    /// Open display polylines sampled from non-solid construction segments.
    pub construction: Vec<Vec<[f64; 3]>>,
    pub origin: [f64; 3],
    pub x_direction: [f64; 3],
    pub y_direction: [f64; 3],
    pub normal: [f64; 3],
}

/// Read-only constraint-system analysis for one evaluated sketch feature.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedSketchDiagnostic {
    pub feature_id: FeatureId,
    pub solve: SketchSolveDiagnostic,
}

impl EvaluatedPart {
    #[must_use]
    pub fn resolve_face(&self, reference: &FaceRef) -> TopologyResolution<'_, EvaluatedFace> {
        resolve_unique(
            self.faces
                .iter()
                .filter(|face| &face.reference == reference),
        )
    }

    #[must_use]
    pub fn face(&self, reference: &FaceRef) -> Option<&EvaluatedFace> {
        self.resolve_face(reference).unique()
    }

    #[must_use]
    pub fn resolve_edge(&self, reference: &EdgeRef) -> TopologyResolution<'_, EvaluatedEdge> {
        resolve_unique(
            self.edges
                .iter()
                .filter(|edge| &edge.reference == reference),
        )
    }

    #[must_use]
    pub fn edge(&self, reference: &EdgeRef) -> Option<&EvaluatedEdge> {
        self.resolve_edge(reference).unique()
    }

    #[must_use]
    pub fn resolve_vertex(&self, reference: &VertexRef) -> TopologyResolution<'_, EvaluatedVertex> {
        resolve_unique(
            self.vertices
                .iter()
                .filter(|vertex| &vertex.reference == reference),
        )
    }

    #[must_use]
    pub fn vertex(&self, reference: &VertexRef) -> Option<&EvaluatedVertex> {
        self.resolve_vertex(reference).unique()
    }
}

fn resolve_unique<'a, T>(mut candidates: impl Iterator<Item = &'a T>) -> TopologyResolution<'a, T> {
    let Some(first) = candidates.next() else {
        return TopologyResolution::Lost;
    };
    let Some(second) = candidates.next() else {
        return TopologyResolution::Resolved(first);
    };
    let mut ambiguous = vec![first, second];
    ambiguous.extend(candidates);
    TopologyResolution::Ambiguous(ambiguous)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvaluatedScene {
    pub parts: Vec<EvaluatedPart>,
    /// Optional renderer optimization; engineering consumers use `parts`.
    pub mesh_definitions: Vec<EvaluatedMeshDefinition>,
    /// Visible placed uses of `mesh_definitions`.
    pub mesh_instances: Vec<EvaluatedMeshInstance>,
    pub sketches: Vec<EvaluatedSketch>,
    pub sketch_diagnostics: Vec<EvaluatedSketchDiagnostic>,
    pub datum_planes: Vec<EvaluatedDatumPlane>,
    pub datum_points: Vec<EvaluatedDatumPoint>,
}

impl EvaluatedScene {
    #[must_use]
    pub fn mesh_definition(&self, key: AssemblyDefinitionBody) -> Option<&EvaluatedMeshDefinition> {
        self.mesh_definitions
            .iter()
            .find(|definition| definition.key == key)
    }

    #[must_use]
    pub fn mesh_instance(&self, feature_id: FeatureId) -> Option<&EvaluatedMeshInstance> {
        self.mesh_instances
            .iter()
            .find(|instance| instance.feature_id == feature_id)
    }

    #[must_use]
    pub fn sketch_diagnostic(&self, feature_id: FeatureId) -> Option<&SketchSolveDiagnostic> {
        self.sketch_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.feature_id == feature_id)
            .map(|diagnostic| &diagnostic.solve)
    }

    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.parts
            .iter()
            .map(|part| part.mesh.triangle_count())
            .sum()
    }

    #[must_use]
    pub fn resolve_face(&self, reference: &FaceRef) -> TopologyResolution<'_, EvaluatedFace> {
        resolve_unique(
            self.parts
                .iter()
                .flat_map(|part| &part.faces)
                .filter(|face| {
                    face.reference.feature_id == reference.feature_id
                        && &face.reference == reference
                }),
        )
    }

    #[must_use]
    pub fn face(&self, reference: &FaceRef) -> Option<&EvaluatedFace> {
        self.resolve_face(reference).unique()
    }

    #[must_use]
    pub fn resolve_edge(&self, reference: &EdgeRef) -> TopologyResolution<'_, EvaluatedEdge> {
        resolve_unique(
            self.parts
                .iter()
                .flat_map(|part| &part.edges)
                .filter(|edge| {
                    edge.reference.feature_id == reference.feature_id
                        && &edge.reference == reference
                }),
        )
    }

    #[must_use]
    pub fn edge(&self, reference: &EdgeRef) -> Option<&EvaluatedEdge> {
        self.resolve_edge(reference).unique()
    }

    #[must_use]
    pub fn resolve_vertex(&self, reference: &VertexRef) -> TopologyResolution<'_, EvaluatedVertex> {
        resolve_unique(
            self.parts
                .iter()
                .flat_map(|part| &part.vertices)
                .filter(|vertex| {
                    vertex.reference.feature_id == reference.feature_id
                        && &vertex.reference == reference
                }),
        )
    }

    #[must_use]
    pub fn vertex(&self, reference: &VertexRef) -> Option<&EvaluatedVertex> {
        self.resolve_vertex(reference).unique()
    }
}

/// Backend-neutral boundary between the parametric document and a CAD kernel.
///
/// Kernel-native B-Rep objects stay inside an implementation. The application
/// consumes only evaluated render meshes, so another kernel can replace Truck
/// without changing document, AI, or UI code.
pub trait CadKernel: Send + Sync {
    fn name(&self) -> &'static str;

    /// Advertises backend behavior that affects command availability.
    /// Implementations must opt in; the default is deliberately conservative.
    fn capabilities(&self) -> CadKernelCapabilities {
        CadKernelCapabilities::default()
    }

    /// Evaluates the declarative document into renderable triangle meshes.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when the backend cannot construct or tessellate
    /// one of the document features.
    fn evaluate(&self, document: &CadDocument) -> Result<EvaluatedScene, KernelError>;

    /// Performs product-level interference analysis without exposing native
    /// B-Rep objects. Implementations must opt in through `capabilities()`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when the capability is unsupported or the
    /// document cannot be materialized. Individual pair failures are retained
    /// as structured report evidence instead of aborting the full analysis.
    fn analyze_interference(
        &self,
        _document: &CadDocument,
    ) -> Result<InterferenceAnalysis, KernelError> {
        Err(KernelError::Unsupported {
            operation: "interference analysis",
        })
    }
}

/// Optional CAD-kernel capability for exact B-Rep exchange.
///
/// The port is owned by the kernel-neutral core. Concrete kernels implement
/// it without exposing their native topology types to application code.
pub trait ExchangeKernel: CadKernel {
    /// Encodes all visible solid features as STEP AP214 data.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when evaluation or exact encoding fails.
    fn encode_step(&self, document: &CadDocument, file_name: &str) -> Result<String, KernelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_resolution_reports_unique_ambiguous_and_lost_candidates() {
        let values = [1, 2];
        assert!(matches!(
            resolve_unique(values[..1].iter()),
            TopologyResolution::Resolved(1)
        ));
        assert!(matches!(
            resolve_unique(values.iter()),
            TopologyResolution::Ambiguous(candidates) if candidates.len() == 2
        ));
        assert!(matches!(
            resolve_unique(values[..0].iter()),
            TopologyResolution::Lost
        ));
    }

    #[test]
    fn kernel_capabilities_default_closed_and_serialize_stably() {
        let default = CadKernelCapabilities::default();
        assert_eq!(default.chamfer.edge_count, EdgeCountSupport::Unsupported);
        assert_eq!(default.fillet.edge_count, EdgeCountSupport::Unsupported);
        assert!(!default.interference_analysis);

        let capabilities = CadKernelCapabilities {
            chamfer: EdgeModifierCapability {
                edge_count: EdgeCountSupport::Multiple,
                shared_vertex_support: SharedVertexSupport::ConvexPolyhedralSource,
                ..EdgeModifierCapability::default()
            },
            ..CadKernelCapabilities::default()
        };
        let value = serde_json::to_value(capabilities).unwrap();
        assert_eq!(
            value["chamfer"]["shared_vertex_support"],
            "convex_polyhedral_source"
        );
        assert_eq!(
            capabilities
                .edge_modifier(EdgeModifierOperation::Chamfer)
                .shared_vertex_support,
            SharedVertexSupport::ConvexPolyhedralSource
        );
        assert_eq!(value["interference_analysis"], false);
    }

    #[test]
    fn interference_report_serializes_typed_precision_and_pair_outcomes() {
        let report = InterferenceAnalysis {
            candidate_feature_ids: vec![2, 7],
            total_pair_count: 1,
            broad_phase_pair_count: 1,
            interfering_pair_count: 1,
            volume_tolerance_mm3: 0.001,
            pairs: vec![InterferencePairAnalysis {
                feature_ids: [2, 7],
                bounds: [
                    crate::diagnostics::AxisAlignedBounds {
                        min: [0.0; 3],
                        max: [10.0; 3],
                    },
                    crate::diagnostics::AxisAlignedBounds {
                        min: [5.0; 3],
                        max: [15.0; 3],
                    },
                ],
                outcome: InterferencePairOutcome::Interfering {
                    volume_mm3: 125.0,
                    precision: InterferenceVolumePrecision::Tessellated {
                        chord_tolerance_mm: 0.05,
                    },
                    method: InterferenceIntersectionMethod::BrepBoolean,
                },
            }],
            ..InterferenceAnalysis::default()
        };
        assert!(report.has_interference());
        assert!(report.is_complete());

        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["pairs"][0]["outcome"]["status"], "interfering");
        assert_eq!(
            value["pairs"][0]["outcome"]["precision"]["kind"],
            "tessellated"
        );
        assert_eq!(value["pairs"][0]["outcome"]["method"], "brep_boolean");
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum KernelError {
    #[error("document dependency graph is invalid: {0}")]
    InvalidDocument(#[from] DocumentError),
    #[error("feature {feature_id}: {message}")]
    Evaluation {
        feature_id: FeatureId,
        message: String,
    },
    #[error("feature {feature_id} topology naming failed: {message}")]
    TopologyNaming {
        feature_id: FeatureId,
        message: String,
    },
    #[error("{0}")]
    Boolean(Box<BooleanDiagnostic>),
    #[error("{0}")]
    EdgeModifier(Box<EdgeModifierDiagnostic>),
    #[error("mesh is too large for 32-bit indices")]
    MeshTooLarge,
    #[error("could not encode {format} exchange data: {message}")]
    Exchange {
        format: &'static str,
        message: String,
    },
    #[error("CAD analysis failed: {message}")]
    Analysis { message: String },
    #[error("CAD kernel does not support {operation}")]
    Unsupported { operation: &'static str },
}

impl From<BooleanDiagnostic> for KernelError {
    fn from(diagnostic: BooleanDiagnostic) -> Self {
        Self::Boolean(Box::new(diagnostic))
    }
}

impl From<EdgeModifierDiagnostic> for KernelError {
    fn from(diagnostic: EdgeModifierDiagnostic) -> Self {
        Self::EdgeModifier(Box::new(diagnostic))
    }
}
