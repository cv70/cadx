//! System prompt text and the read-only prompt payload sent to the provider.

use std::collections::BTreeSet;

use serde::Serialize;

use cadx_analysis::{MeasurementResult, SceneAnalysis};
use cadx_core::{
    diagnostics::{BooleanDiagnostic, EdgeModifierDiagnostic, SketchConstraintDiagnostic},
    domain::{FeatureId, Primitive},
    kernel::{
        CadKernelCapabilities, InterferenceAnalysis, InterferencePairAnalysis,
        InterferencePairOutcome, SketchSolveDiagnostic,
    },
};

use crate::{AiContext, AiError, AiRequest, AiSketchDimension};

use super::document_view::planning_document_context;

pub(super) const SYSTEM_PROMPT: &str = r"You are the modeling agent inside CADX, a parametric 3D CAD application.
Convert the user's request into safe, explicit modeling commands by calling apply_cad_plan exactly once.
All dimensions and positions are in millimeters. Preserve existing features unless the user asks to change them.
Use the feature ids in the supplied document when modifying existing geometry. Keep the plan concise.
When the user asks to optimize, compare tradeoffs, or explore approaches and
there are genuinely distinct solutions, include up to two independent
alternatives. Every alternative must be a complete command batch against the
same current document, not an incremental edit after another candidate. Do not
invent volume, mass, clearance, interference, or performance claims; CADX will
kernel-evaluate and measure every candidate locally before showing it.
Sketch profiles may include ordered Coincident, Horizontal, Vertical, Fixed,
Distance, HorizontalDistance, VerticalDistance, PointLineDistance,
LineThroughCenter, Length, EqualLength, Parallel, Perpendicular, and Angle constraints;
use zero-based point and segment ids. Length references one Line. EqualLength,
Parallel, Perpendicular, and Angle reference two distinct Line segments.
angle_degrees is the directed angle from the first segment to the second
segment in the inclusive range [-180, 180].
HorizontalDistance and VerticalDistance constrain the signed coordinate delta
from first point to second point and therefore accept finite negative, zero, or
positive dimensions. PointLineDistance is the non-negative perpendicular
distance to the infinite support of one Line and retains the point's initial
side. LineThroughCenter requires one Line support to pass through one Arc center.
Sketches may also include interior hole loops. Each hole must be a simple closed
profile strictly inside the outer profile and disjoint from every other hole;
constraints never address hole loops. Hole loops can drive extrusion but are
not supported by revolve.
Use create_sketch_region or set_sketch_region for exact Line, Arc, rational
quadratic, and cubic Bezier geometry. Each
loop is an ordered segment array: every segment end must equal the next segment
start, including the last-to-first closure. An Arc's start and end must have the
same positive radius from its center. Represent a complete circle as exactly two
connected semicircular Arc segments; never approximate a circle with polygon
points. A rational quadratic owns one segment-local control and a positive
weight; a cubic Bezier owns two segment-local controls. Exact regions also support Radius and FixedCenter on Arc segments,
EqualRadius and Concentric between two Arc segments, and Tangent between
adjacent segments when at least one is curved. All Line relationship constraints
may only reference Line segments. CurvatureContinuous requires two adjacent
curved segments and enforces both the same traversal tangent and the same signed
curvature.
Curve-specific constraints require create_sketch_region
or set_sketch_constraints on an exact region; they cannot be used with a point-only
create_sketch profile.
An exact sketch may include up to 128 independent construction exact-curve
segments. Construction participates in solving and display but never in the
closed region, extrusion, revolve, mass properties, or exported solid. For a
profile with N segments, profile ids remain P0..P(N-1) and S0..S(N-1).
Construction segment i is S(N+i), with independent endpoint ids P(N+2i) and
P(N+2i+1). PointOnCurve constrains a point to the finite exact curve segment,
not its infinite support curve. Midpoint constrains a point other than the
target endpoints to the midpoint of one Line. Symmetric constrains two distinct
points about one Line axis. Use create_sketch_region or atomic
set_sketch_definition when construction or these point relationships are needed.
Constraint-driven sketches can drive either an extrusion or a revolve. Revolve
axes use a 2D origin and direction in the sketch profile plane, with angles in
degrees from 0 to 360. Sketches default to world XY; use a world plane, an
existing datum-plane feature id, or a supplied persistent planar face when the
requested design intent requires it.
Ruled loft consumes 2 to 32 ordered existing sketch ids. Every section must
have one outer loop, the same exact segment count and traversal direction, and
no holes. Order the ids monotonically through the intended body; do not use a
loft for folded or branching section sequences.
The user will review the proposed command batch before CADX applies it.
Use the computed geometric context for spatial reasoning when present. Treat
it as read-only evidence. Interaction selection is nested under
interaction.selection; viewport focus, retrieved feature graph entries, spatial
neighbors, and every omitted count are under interaction. Selected faces,
edges, and vertices use persistent kernel-neutral references. A selected face
or vertex may be used for reference-geometry
commands. Follow kernel_capabilities when deciding whether chamfer or fillet is available and which
multi-edge, support-surface, convexity, and shared-vertex restrictions apply. When capabilities are
absent, use the conservative contract of one convex linear edge between two planar faces. Do not
propose edge modifiers outside the declared contract or variable radii. Material density is always in kg/m^3;
only assign one when the user supplies it or explicitly names a known material.
Measurements, sketch solve/failure diagnostics, boolean and edge-modifier failure diagnostics, mass, center of mass, inertia, and product interference reports in the computed context are read-only results.
Assembly occurrences must be repositioned with set_occurrence_transform using
the persisted assembly_id and occurrence_id, local position in millimeters, and
local XYZ Euler rotation in degrees. A non-root occurrence may instead have one
driving assembly mate to its actual hierarchy parent. Create it with a unique
mate id, full rigid parent and child anchor frames, and fixed, revolute, or
slider kind. Mate axes are normalized and expressed in the parent anchor frame;
revolute state and limits use degrees, slider state and limits use millimeters,
and fixed state is zero. Change only the scalar state with set_assembly_mate_state.
Delete the mate before directly repositioning a driven occurrence. Use
set_occurrence_suppressed to suppress or restore an occurrence and its complete
descendant subtree. Suppression preserves mate state and is distinct from
feature visibility. It is rejected when an active feature depends on a body in
the suppressed subtree. Never use move or rotate on an assembly-owned feature;
occurrence and mate commands update the product structure atomically.
Use selected_sketch_diagnostic degrees_of_freedom, rank, and redundant_constraints
and selected_sketch_dimensions zero-based constraint indices, kinds, and values
to refine a sketch without treating redundancy as inconsistency. For a rejected
edit, branch on last_sketch_failure.reason and its zero-based constraint_indices.
Use diagnostic stage and reason codes for corrective planning; never infer behavior by parsing a diagnostic detail string.
Do not invent tolerance data that is not supplied.
The document context is relevance-retrieved and bounded. Never guess an omitted
feature, occurrence, mate, or identifier. Use only detailed objects supplied in
the current request; if the requested object is omitted or ambiguous, return no
speculative edit.
Never claim support for operations that are absent from the tool schema.";

pub(super) const DOMAIN_SYSTEM_PROMPT: &str = r"You are the domain tool router inside CADX, an industrial CAD application.
Choose exactly one of the offered tools that best satisfies the user's request and call it exactly once.
Use the supplied document context for entity references and dimensions. Do not invent tool names or fields.
Arguments use the plain JSON types declared by the selected tool schema. Omit optional values only when the
schema permits it. Never emit CAD commands directly; the selected domain pack validates and executes the call.";

const MAX_PROMPT_INTERFERENCE_FEATURES: usize = 64;

const MAX_PROMPT_INTERFERENCE_PAIRS: usize = 32;

#[derive(Serialize)]
struct PlanningAiContext<'a> {
    interaction: &'a crate::context::ContextSnapshot,
    kernel_capabilities: &'a CadKernelCapabilities,
    measurement: &'a Option<MeasurementResult>,
    last_boolean_failure: &'a Option<BooleanDiagnostic>,
    last_edge_modifier_failure: &'a Option<EdgeModifierDiagnostic>,
    last_sketch_failure: &'a Option<SketchConstraintDiagnostic>,
    selected_sketch_diagnostic: &'a Option<SketchSolveDiagnostic>,
    selected_sketch_dimensions: &'a [AiSketchDimension],
    scene_analysis: &'a SceneAnalysis,
    interference_analysis: Option<PlanningInterferenceContext<'a>>,
}

#[derive(Serialize)]
struct PlanningInterferenceContext<'a> {
    total_candidate_feature_count: usize,
    candidate_feature_ids: Vec<FeatureId>,
    omitted_candidate_feature_count: usize,
    total_pair_count: u64,
    broad_phase_pair_count: u64,
    clear_pair_count: u64,
    interfering_pair_count: u64,
    failed_pair_count: u64,
    volume_tolerance_mm3: f64,
    total_pair_detail_count: usize,
    pairs: Vec<&'a InterferencePairAnalysis>,
    omitted_pair_detail_count: usize,
}

pub(super) fn build_prompt(request: &AiRequest) -> Result<String, AiError> {
    let mut document_context = planning_document_context(request);
    for feature in &mut document_context.features {
        if let Primitive::ImportedStep { source, .. } = &mut feature.primitive {
            *source = format!("<redacted embedded STEP source: {} bytes>", source.len());
        }
    }
    let document = serde_json::to_string_pretty(&document_context)
        .map_err(|error| AiError::InvalidPlan(error.to_string()))?;
    let context = request
        .context
        .as_ref()
        .map(planning_ai_context)
        .map(|context| serde_json::to_string_pretty(&context))
        .transpose()
        .map_err(|error| AiError::InvalidPlan(error.to_string()))?
        .unwrap_or_else(|| "{}".into());
    Ok(format!(
        "Retrieved CAD document context (read-only, bounded):\n{document}\n\nComputed geometric and interaction context (read-only, bounded):\n{context}\n\nUser request:\n{}",
        request.prompt
    ))
}

fn planning_ai_context(context: &AiContext) -> PlanningAiContext<'_> {
    let relevant_feature_ids = context
        .interaction
        .relevant_features
        .iter()
        .map(|feature| feature.feature_id)
        .chain(
            context
                .interaction
                .spatial_entities
                .iter()
                .map(|entity| entity.feature_id),
        )
        .chain(context.interaction.selection.selected_feature_id)
        .chain(
            context
                .interaction
                .selection
                .selected_face
                .as_ref()
                .map(|face| face.feature_id),
        )
        .chain(
            context
                .interaction
                .selection
                .selected_edges
                .iter()
                .map(|edge| edge.feature_id),
        )
        .chain(
            context
                .interaction
                .selection
                .selected_vertex
                .as_ref()
                .map(|vertex| vertex.feature_id),
        )
        .collect::<BTreeSet<_>>();
    PlanningAiContext {
        interaction: &context.interaction,
        kernel_capabilities: &context.kernel_capabilities,
        measurement: &context.measurement,
        last_boolean_failure: &context.last_boolean_failure,
        last_edge_modifier_failure: &context.last_edge_modifier_failure,
        last_sketch_failure: &context.last_sketch_failure,
        selected_sketch_diagnostic: &context.selected_sketch_diagnostic,
        selected_sketch_dimensions: &context.selected_sketch_dimensions,
        scene_analysis: &context.scene_analysis,
        interference_analysis: context
            .interference_analysis
            .as_ref()
            .map(|analysis| planning_interference_context(analysis, &relevant_feature_ids)),
    }
}

fn planning_interference_context<'a>(
    analysis: &'a InterferenceAnalysis,
    relevant_feature_ids: &BTreeSet<FeatureId>,
) -> PlanningInterferenceContext<'a> {
    let mut ranked_pairs = analysis.pairs.iter().enumerate().collect::<Vec<_>>();
    ranked_pairs.sort_by_key(|(index, pair)| {
        let related = pair
            .feature_ids
            .iter()
            .any(|feature_id| relevant_feature_ids.contains(feature_id));
        let outcome_priority = match pair.outcome {
            InterferencePairOutcome::Interfering { .. } => 0,
            InterferencePairOutcome::Failed { .. } => 1,
            InterferencePairOutcome::Clear { .. } => 2,
        };
        (!related, outcome_priority, *index)
    });
    ranked_pairs.truncate(MAX_PROMPT_INTERFERENCE_PAIRS);
    let pairs = ranked_pairs
        .into_iter()
        .map(|(_, pair)| pair)
        .collect::<Vec<_>>();

    let available_feature_ids = analysis
        .candidate_feature_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut seen_feature_ids = BTreeSet::new();
    let candidate_feature_ids = analysis
        .candidate_feature_ids
        .iter()
        .copied()
        .filter(|feature_id| relevant_feature_ids.contains(feature_id))
        .chain(
            pairs
                .iter()
                .flat_map(|pair| pair.feature_ids)
                .filter(|feature_id| available_feature_ids.contains(feature_id)),
        )
        .chain(analysis.candidate_feature_ids.iter().copied())
        .filter(|feature_id| seen_feature_ids.insert(*feature_id))
        .take(MAX_PROMPT_INTERFERENCE_FEATURES)
        .collect::<Vec<_>>();

    PlanningInterferenceContext {
        total_candidate_feature_count: analysis.candidate_feature_ids.len(),
        omitted_candidate_feature_count: analysis
            .candidate_feature_ids
            .len()
            .saturating_sub(candidate_feature_ids.len()),
        candidate_feature_ids,
        total_pair_count: analysis.total_pair_count,
        broad_phase_pair_count: analysis.broad_phase_pair_count,
        clear_pair_count: analysis.clear_pair_count,
        interfering_pair_count: analysis.interfering_pair_count,
        failed_pair_count: analysis.failed_pair_count,
        volume_tolerance_mm3: analysis.volume_tolerance_mm3,
        total_pair_detail_count: analysis.pairs.len(),
        omitted_pair_detail_count: analysis.pairs.len().saturating_sub(pairs.len()),
        pairs,
    }
}

#[cfg(test)]
mod tests {
    use crate::AiPlan;
    use cadx_core::domain::{Constraint, ModelCommand, SketchSegment2D};
    use serde_json::json;

    use super::*;

    #[test]
    fn planning_prompt_contains_read_only_scene_context() {
        let prompt = build_prompt(&AiRequest {
            prompt: "inspect this part".into(),
            document: cadx_core::domain::CadDocument::default(),
            context: Some(AiContext {
                interaction: crate::context::ContextSnapshot {
                    selection: crate::context::ContextSelection {
                        selected_feature_id: Some(4),
                        ..crate::context::ContextSelection::default()
                    },
                    ..crate::context::ContextSnapshot::default()
                },
                kernel_capabilities: cadx_core::kernel::CadKernelCapabilities::default(),
                measurement: None,
                last_boolean_failure: None,
                last_edge_modifier_failure: None,
                last_sketch_failure: None,
                selected_sketch_diagnostic: None,
                selected_sketch_dimensions: Vec::new(),
                scene_analysis: SceneAnalysis {
                    total_volume_mm3: 128.0,
                    total_mass_kg: Some(0.42),
                    center_of_mass_mm: Some([1.0, 2.0, 3.0]),
                    ..SceneAnalysis::default()
                },
                interference_analysis: Some(cadx_core::kernel::InterferenceAnalysis {
                    candidate_feature_ids: vec![4, 5],
                    total_pair_count: 1,
                    interfering_pair_count: 1,
                    ..cadx_core::kernel::InterferenceAnalysis::default()
                }),
            }),
        })
        .unwrap();
        assert!(prompt.contains("total_volume_mm3"));
        assert!(prompt.contains("128.0"));
        assert!(prompt.contains("total_mass_kg"));
        assert!(prompt.contains("center_of_mass_mm"));
        assert!(prompt.contains("kernel_capabilities"));
        assert!(prompt.contains("interference_analysis"));
        assert!(prompt.contains("interfering_pair_count"));
        assert!(prompt.contains("read-only"));
    }

    #[test]
    fn planning_interference_context_is_relevance_ranked_and_bounded() {
        let bounds = cadx_core::diagnostics::AxisAlignedBounds {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        let pairs = (0..40)
            .map(|index| cadx_core::kernel::InterferencePairAnalysis {
                feature_ids: [index * 2 + 1, index * 2 + 2],
                bounds: [bounds; 2],
                outcome: cadx_core::kernel::InterferencePairOutcome::Clear {
                    volume_mm3: 0.0,
                    precision: cadx_core::kernel::InterferenceVolumePrecision::Tessellated {
                        chord_tolerance_mm: 0.01,
                    },
                    method: cadx_core::kernel::InterferenceIntersectionMethod::BrepBoolean,
                },
            })
            .collect::<Vec<_>>();
        let analysis = cadx_core::kernel::InterferenceAnalysis {
            candidate_feature_ids: (1..=80).collect(),
            total_pair_count: 3_160,
            broad_phase_pair_count: 40,
            clear_pair_count: 3_160,
            interfering_pair_count: 0,
            failed_pair_count: 0,
            volume_tolerance_mm3: 1.0e-6,
            pairs,
        };

        let planning = planning_interference_context(&analysis, &BTreeSet::from([80]));

        assert_eq!(planning.pairs.len(), MAX_PROMPT_INTERFERENCE_PAIRS);
        assert_eq!(planning.omitted_pair_detail_count, 8);
        assert_eq!(planning.pairs[0].feature_ids, [79, 80]);
        assert_eq!(
            planning.candidate_feature_ids.len(),
            MAX_PROMPT_INTERFERENCE_FEATURES
        );
        assert_eq!(planning.candidate_feature_ids[0], 80);
        assert_eq!(planning.omitted_candidate_feature_count, 16);
        assert_eq!(planning.total_pair_count, 3_160);
    }

    #[test]
    fn planning_prompt_redacts_embedded_step_source() {
        let mut document = cadx_core::domain::CadDocument::default();
        document
            .apply(ModelCommand::ImportStep {
                name: "supplier part".into(),
                source: "sensitive-step-source".into(),
                data_section: 0,
                shell_id: 42,
                void_shells: Vec::new(),
                length_unit: cadx_core::domain::StepLengthUnit::millimeter(),
                color: None,
                position: [0.0; 3],
            })
            .unwrap();
        let prompt = build_prompt(&AiRequest {
            prompt: "move the supplier part".into(),
            document,
            context: None,
        })
        .unwrap();
        assert!(!prompt.contains("sensitive-step-source"));
        assert!(prompt.contains("redacted embedded STEP source: 21 bytes"));
        assert!(prompt.contains("\"shell_id\": 42"));
    }

    #[test]
    fn advanced_line_constraint_plan_deserializes_and_executes() {
        let value = json!({
            "summary": "Create a dimensioned rectangular pad",
            "commands": [
                {
                    "op": "create_sketch",
                    "name": "Advanced outline",
                    "profile": [[0.0, 0.0], [9.0, 1.0], [10.0, 6.0], [1.0, 5.0]],
                    "constraints": [
                        { "type": "fixed", "point": 0, "x": 0.0, "y": 0.0 },
                        { "type": "length", "segment": 0, "length": 10.0 },
                        { "type": "length", "segment": 1, "length": 5.0 },
                        { "type": "equal_length", "first": 0, "second": 2 },
                        { "type": "parallel", "first": 0, "second": 2 },
                        { "type": "perpendicular", "first": 0, "second": 1 },
                        { "type": "equal_length", "first": 1, "second": 3 },
                        { "type": "angle", "first": 0, "second": 3, "angle_degrees": -90.0 }
                    ],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "create_extrusion_from_sketch",
                    "name": "Advanced pad",
                    "sketch_id": 1,
                    "height": 4.0,
                    "position": [0.0, 0.0, 0.0]
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        let ModelCommand::CreateSketch { constraints, .. } = &plan.commands[0] else {
            panic!("expected advanced sketch command");
        };
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::Length { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::EqualLength { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::Parallel { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::Perpendicular { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::Angle { .. }))
        );

        let mut document = cadx_core::domain::CadDocument::default();
        document.apply_transaction(plan.commands).unwrap();
        let Primitive::ExtrusionFromSketch { region, .. } = &document.feature(2).unwrap().primitive
        else {
            panic!("expected advanced constrained extrusion");
        };
        assert!((region.profile.signed_area().abs() - 50.0).abs() < 1.0e-6);
        assert!(SYSTEM_PROMPT.contains("inclusive range [-180, 180]"));
        assert!(SYSTEM_PROMPT.contains("two distinct"));
    }

    #[test]
    fn construction_point_relationship_plan_deserializes_and_executes() {
        let value = json!({
            "summary": "Create a construction-driven rectangular pad",
            "commands": [
                {
                    "op": "create_sketch_region",
                    "name": "Construction outline",
                    "plane": { "type": "world_xy" },
                    "profile": [
                        { "type": "line", "start": [0.0, 0.0], "end": [10.0, 0.0] },
                        { "type": "line", "start": [10.0, 0.0], "end": [10.0, 8.0] },
                        { "type": "line", "start": [10.0, 8.0], "end": [0.0, 8.0] },
                        { "type": "line", "start": [0.0, 8.0], "end": [0.0, 0.0] }
                    ],
                    "holes": [],
                    "construction": [
                        { "type": "line", "start": [0.0, -5.0], "end": [0.0, 5.0] },
                        { "type": "line", "start": [-3.0, 2.0], "end": [-5.0, 2.0] },
                        { "type": "line", "start": [3.0, 2.0], "end": [5.0, 2.0] },
                        { "type": "line", "start": [-5.0, 0.0], "end": [-5.0, 4.0] },
                        { "type": "line", "start": [5.0, 0.0], "end": [5.0, 4.0] }
                    ],
                    "constraints": [
                        { "type": "symmetric", "first": 6, "second": 8, "axis": 4 },
                        { "type": "midpoint", "point": 7, "segment": 7 },
                        { "type": "point_on_curve", "point": 9, "segment": 8 }
                    ],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "create_extrusion_from_sketch",
                    "name": "Construction pad",
                    "sketch_id": 1,
                    "height": 4.0,
                    "position": [0.0, 0.0, 0.0]
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        let ModelCommand::CreateSketchRegion {
            construction,
            constraints,
            ..
        } = &plan.commands[0]
        else {
            panic!("expected construction sketch command");
        };
        assert_eq!(construction.len(), 5);
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::PointOnCurve { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::Midpoint { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::Symmetric { .. }))
        );

        let mut document = cadx_core::domain::CadDocument::default();
        document.apply_transaction(plan.commands).unwrap();
        let Primitive::ExtrusionFromSketch { region, .. } = &document.feature(2).unwrap().primitive
        else {
            panic!("expected construction constrained extrusion");
        };
        assert!((region.profile.signed_area().abs() - 80.0).abs() < 1.0e-8);
        assert!(SYSTEM_PROMPT.contains("finite exact curve segment"));
        assert!(SYSTEM_PROMPT.contains("P(N+2i)"));
        assert!(SYSTEM_PROMPT.contains("mass properties, or exported solid"));
    }

    #[test]
    fn point_dimension_and_center_plan_deserializes_and_executes() {
        let value = json!({
            "summary": "Dimension construction references",
            "commands": [
                {
                    "op": "create_sketch_region",
                    "name": "Dimensioned references",
                    "plane": { "type": "world_xy" },
                    "profile": [
                        { "type": "line", "start": [0.0, 0.0], "end": [10.0, 0.0] },
                        { "type": "line", "start": [10.0, 0.0], "end": [10.0, 8.0] },
                        { "type": "line", "start": [10.0, 8.0], "end": [0.0, 8.0] },
                        { "type": "line", "start": [0.0, 8.0], "end": [0.0, 0.0] }
                    ],
                    "holes": [],
                    "construction": [
                        { "type": "line", "start": [0.0, 0.0], "end": [4.0, 0.0] },
                        {
                            "type": "arc",
                            "start": [8.0, 2.0],
                            "end": [4.0, 2.0],
                            "center": [6.0, 0.0],
                            "ccw": true
                        }
                    ],
                    "constraints": [
                        { "type": "horizontal_distance", "first": 4, "second": 5, "distance": 4.0 },
                        { "type": "vertical_distance", "first": 4, "second": 5, "distance": 0.0 },
                        { "type": "point_line_distance", "point": 6, "line": 4, "distance": 2.0 },
                        { "type": "line_through_center", "line": 4, "arc": 5 }
                    ],
                    "position": [0.0, 0.0, 0.0]
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        let ModelCommand::CreateSketchRegion { constraints, .. } = &plan.commands[0] else {
            panic!("expected dimensioned sketch command");
        };
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::HorizontalDistance { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::VerticalDistance { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::PointLineDistance { .. }))
        );
        assert!(
            constraints
                .iter()
                .any(|constraint| matches!(constraint, Constraint::LineThroughCenter { .. }))
        );

        let mut document = cadx_core::domain::CadDocument::default();
        document.apply_transaction(plan.commands).unwrap();
        assert!(matches!(
            &document.feature(1).unwrap().primitive,
            Primitive::Sketch { constraints, .. } if constraints.len() == 4
        ));
        assert!(SYSTEM_PROMPT.contains("signed coordinate delta"));
        assert!(SYSTEM_PROMPT.contains("selected_sketch_diagnostic"));
        assert!(SYSTEM_PROMPT.contains("selected_sketch_dimensions"));
    }

    #[test]
    fn constrained_exact_circle_commands_deserialize_and_execute_for_ai_plans() {
        let circle = json!([
            {
                "type": "arc",
                "start": [5.0, -2.0],
                "end": [-3.0, -2.0],
                "center": [1.0, -2.0],
                "ccw": true
            },
            {
                "type": "arc",
                "start": [-3.0, -2.0],
                "end": [5.0, -2.0],
                "center": [1.0, -2.0],
                "ccw": true
            }
        ]);
        let value = json!({
            "summary": "Create and constrain an exact circle",
            "commands": [
                {
                    "op": "create_sketch_region",
                    "name": "Exact circle",
                    "plane": { "type": "world_xy" },
                    "profile": circle.clone(),
                    "holes": [],
                    "constraints": [
                        { "type": "fixed_center", "segment": 0, "x": 3.0, "y": 5.0 },
                        { "type": "radius", "segment": 0, "radius": 6.0 },
                        { "type": "concentric", "first": 0, "second": 1 },
                        { "type": "equal_radius", "first": 0, "second": 1 },
                        { "type": "tangent", "first": 0, "second": 1 },
                        { "type": "curvature_continuous", "first": 0, "second": 1 }
                    ],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "set_sketch_region",
                    "id": 1,
                    "profile": circle,
                    "holes": []
                },
                {
                    "op": "create_extrusion_from_sketch",
                    "name": "Constrained cylinder",
                    "sketch_id": 1,
                    "height": 4.0,
                    "position": [0.0, 0.0, 0.0]
                }
            ]
        });

        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateSketchRegion { region, constraints, .. }
                if constraints.len() == 6
                    && region.profile.segments.len() == 2
                    && region.profile.segments.iter().all(SketchSegment2D::is_arc)
        ));
        assert!(matches!(
            &plan.commands[1],
            ModelCommand::SetSketchRegion { id: 1, region }
                if region.profile.segments.len() == 2
        ));
        assert!(matches!(
            &plan.commands[2],
            ModelCommand::CreateExtrusionFromSketch { sketch_id: 1, .. }
        ));
        assert!(SYSTEM_PROMPT.contains("exactly two"));
        assert!(SYSTEM_PROMPT.contains("same positive radius"));
        assert!(SYSTEM_PROMPT.contains("EqualRadius and Concentric"));
        assert!(SYSTEM_PROMPT.contains("Tangent between"));
        assert!(SYSTEM_PROMPT.contains("CurvatureContinuous requires"));
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateSketchRegion { constraints, .. }
                if matches!(constraints.last(), Some(Constraint::CurvatureContinuous {
                    first: 0,
                    second: 1,
                }))
        ));

        let mut document = cadx_core::domain::CadDocument::default();
        document.apply_transaction(plan.commands).unwrap();
        let Primitive::ExtrusionFromSketch { region, .. } = &document.feature(2).unwrap().primitive
        else {
            panic!("expected constrained extrusion");
        };
        assert!((region.profile.signed_area() - std::f64::consts::PI * 36.0).abs() < 1.0e-6);
        for segment in &region.profile.segments {
            let SketchSegment2D::Arc { start, center, .. } = segment else {
                panic!("expected exact arc");
            };
            assert!((center[0] - 3.0).abs() < 1.0e-8);
            assert!((center[1] - 5.0).abs() < 1.0e-8);
            assert!(((start[0] - center[0]).hypot(start[1] - center[1]) - 6.0).abs() < 1.0e-8);
        }
    }
}
