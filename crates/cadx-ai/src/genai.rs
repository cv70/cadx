use genai::{
    Client, ModelIden, ServiceTarget, WebConfig,
    adapter::AdapterKind,
    chat::{ChatMessage, ChatRequest, Tool},
    resolver::{AuthData, Endpoint, ServiceTargetResolver},
};
use serde_json::json;
use std::time::Duration;

use cadx_config::ProviderConfig;
use cadx_core::domain::Primitive;

use super::{AiAssistant, AiError, AiFuture, AiRequest};

const SYSTEM_PROMPT: &str = r"You are the modeling agent inside CADX, a parametric 3D CAD application.
Convert the user's request into safe, explicit modeling commands by calling apply_cad_plan exactly once.
All dimensions and positions are in millimeters. Preserve existing features unless the user asks to change them.
Use the feature ids in the supplied document when modifying existing geometry. Keep the plan concise.
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
it as read-only evidence. Selected faces, edges, and vertices use persistent
kernel-neutral references. A selected face or vertex may be used for reference-geometry
commands. Follow kernel_capabilities when deciding whether chamfer or fillet is available and which
multi-edge, support-surface, convexity, and shared-vertex restrictions apply. When capabilities are
absent, use the conservative contract of one convex linear edge between two planar faces. Do not
propose edge modifiers outside the declared contract or variable radii. Material density is always in kg/m^3;
only assign one when the user supplies it or explicitly names a known material.
Measurements, sketch solve/failure diagnostics, boolean and edge-modifier failure diagnostics, mass, center of mass, and inertia in the computed context are read-only results.
Use selected_sketch_diagnostic degrees_of_freedom, rank, and redundant_constraints
and selected_sketch_dimensions zero-based constraint indices, kinds, and values
to refine a sketch without treating redundancy as inconsistency. For a rejected
edit, branch on last_sketch_failure.reason and its zero-based constraint_indices.
Use diagnostic stage and reason codes for corrective planning; never infer behavior by parsing a diagnostic detail string.
Do not invent tolerance data that is not supplied.
Never claim support for operations that are absent from the tool schema.";

#[derive(Debug, Clone)]
pub struct GenAiAssistant {
    client: Client,
    model: String,
}

impl GenAiAssistant {
    /// Creates an assistant with an explicit model and no environment-backed
    /// credentials. Prefer [`Self::from_provider_config`] for production use.
    ///
    /// # Panics
    ///
    /// Panics only if the built-in provider configuration is internally
    /// inconsistent, which indicates a programming error.
    pub fn new(model: impl Into<String>) -> Self {
        let config = ProviderConfig {
            model: model.into(),
            ..ProviderConfig::default()
        };
        Self::from_provider_config(&config).expect("built-in AI configuration must be valid")
    }

    /// Creates an assistant from the validated CADX provider configuration.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Configuration`] when the configured adapter name is
    /// unsupported.
    pub fn from_provider_config(config: &ProviderConfig) -> Result<Self, AiError> {
        let model = config.model.trim().to_owned();
        let adapter = match config.adapter.as_deref() {
            Some(name) => {
                AdapterKind::from_lower_str(&name.to_ascii_lowercase()).ok_or_else(|| {
                    AiError::Configuration(format!("unsupported provider.adapter '{name}'"))
                })?
            }
            None if config.endpoint.is_some() => AdapterKind::OpenAI,
            None => AdapterKind::from_model(&model)
                .map_err(|error| AiError::Configuration(error.to_string()))?,
        };
        let endpoint = config.endpoint.clone();
        let api_key = config.api_key.clone();
        let resolver = ServiceTargetResolver::from_resolver_fn(
            move |target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
                Ok(ServiceTarget {
                    endpoint: endpoint
                        .clone()
                        .map_or(target.endpoint, Endpoint::from_owned),
                    auth: api_key
                        .clone()
                        .map_or(AuthData::None, AuthData::from_single),
                    model: ModelIden::new(adapter, target.model.model_name),
                })
            },
        );
        let client = Client::builder()
            .with_adapter_kind(adapter)
            .with_service_target_resolver(resolver)
            .with_web_config(
                WebConfig::default().with_timeout(Duration::from_secs(config.timeout_seconds)),
            )
            .build();
        Ok(Self { client, model })
    }
}

impl AiAssistant for GenAiAssistant {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn plan(&self, request: AiRequest) -> AiFuture {
        let client = self.client.clone();
        let model = self.model.clone();
        Box::pin(async move {
            let prompt = build_prompt(&request)?;
            let chat_request = ChatRequest::new(vec![
                ChatMessage::system(SYSTEM_PROMPT),
                ChatMessage::user(prompt),
            ])
            .with_tools([cad_plan_tool()]);

            let response = client
                .exec_chat(&model, chat_request, None)
                .await
                .map_err(|error| AiError::Request(error.to_string()))?;
            let fallback_text = response
                .first_text()
                .unwrap_or("no response text")
                .to_owned();
            let tool_call = response
                .tool_calls()
                .into_iter()
                .find(|call| call.fn_name == "apply_cad_plan")
                .ok_or(AiError::MissingToolCall(fallback_text))?;

            serde_json::from_value(tool_call.fn_arguments.clone())
                .map_err(|error| AiError::InvalidPlan(error.to_string()))
        })
    }
}

fn build_prompt(request: &AiRequest) -> Result<String, AiError> {
    let mut document_context = request.document.clone();
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
        .map(serde_json::to_string_pretty)
        .transpose()
        .map_err(|error| AiError::InvalidPlan(error.to_string()))?
        .unwrap_or_else(|| "{}".into());
    Ok(format!(
        "Current CAD document:\n{document}\n\nComputed geometric context (read-only):\n{context}\n\nUser request:\n{}",
        request.prompt
    ))
}

fn cad_plan_tool() -> Tool {
    Tool::new("apply_cad_plan")
        .with_description("Apply an ordered, atomic batch of parametric CAD document commands")
        .with_schema(json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Short description of the resulting edit"
                },
                "commands": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 24,
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_box" },
                                    "name": { "type": "string" },
                                    "size": { "$ref": "#/$defs/positiveVec3" },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "size", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_cylinder" },
                                    "name": { "type": "string" },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "radius", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_sphere" },
                                    "name": { "type": "string" },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "radius", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_cone" },
                                    "name": { "type": "string" },
                                    "bottom_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "top_radius": { "type": "number", "minimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "bottom_radius", "top_radius", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_torus" },
                                    "name": { "type": "string" },
                                    "major_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "minor_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "major_radius", "minor_radius", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_extrusion" },
                                    "name": { "type": "string" },
                                    "profile": { "$ref": "#/$defs/profile" },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "profile", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_sketch" },
                                    "name": { "type": "string" },
                                    "plane": { "$ref": "#/$defs/sketch_plane" },
                                    "profile": { "$ref": "#/$defs/profile" },
                                    "holes": { "$ref": "#/$defs/holes" },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256,
                                        "description": "Point and Line constraints only; use create_sketch_region for curve-specific constraints"
                                    },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "profile", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_sketch_region" },
                                    "name": { "type": "string" },
                                    "plane": { "$ref": "#/$defs/sketch_plane" },
                                    "profile": { "$ref": "#/$defs/sketch_loop" },
                                    "holes": { "$ref": "#/$defs/sketch_holes" },
                                    "construction": { "$ref": "#/$defs/sketch_construction" },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256,
                                        "description": "Constraints on the outer exact loop; curve constraints must reference compatible exact-curve segment ids"
                                    },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "profile", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_extrusion_from_sketch" },
                                    "name": { "type": "string" },
                                    "sketch_id": { "type": "integer", "minimum": 1 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "sketch_id", "height", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_revolve_from_sketch" },
                                    "name": { "type": "string" },
                                    "sketch_id": { "type": "integer", "minimum": 1 },
                                    "axis_origin": { "$ref": "#/$defs/vec2" },
                                    "axis_direction": { "$ref": "#/$defs/vec2" },
                                    "angle": { "type": "number", "exclusiveMinimum": 0, "maximum": 360 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "sketch_id", "axis_origin", "axis_direction", "angle", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_loft_from_sketches" },
                                    "name": { "type": "string" },
                                    "sketch_ids": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 2,
                                        "maxItems": 32,
                                        "uniqueItems": true,
                                        "description": "Ordered section sketch ids from start cap to end cap"
                                    },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "sketch_ids", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_datum_plane" },
                                    "name": { "type": "string" },
                                    "face": { "$ref": "#/$defs/face_ref" },
                                    "offset": { "type": "number" }
                                },
                                "required": ["op", "name", "face", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_datum_point" },
                                    "name": { "type": "string" },
                                    "vertex": { "$ref": "#/$defs/vertex_ref" },
                                    "offset": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "name", "vertex", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_chamfer" },
                                    "name": { "type": "string" },
                                    "edges": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/edge_ref" },
                                        "minItems": 1,
                                        "uniqueItems": true
                                    },
                                    "distance": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "name", "edges", "distance"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_fillet" },
                                    "name": { "type": "string" },
                                    "edges": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/edge_ref" },
                                        "minItems": 1,
                                        "uniqueItems": true
                                    },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "name", "edges", "radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "create_boolean" },
                                    "name": { "type": "string" },
                                    "operation": { "enum": ["union", "subtract", "intersect"] },
                                    "left": { "type": "integer", "minimum": 1 },
                                    "right": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "name", "operation", "left", "right"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "duplicate" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "name": { "type": "string" },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "move" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "position": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "position"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "rotate" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "rotation": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "rotation"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_box" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "size": { "$ref": "#/$defs/positiveVec3" }
                                },
                                "required": ["op", "id", "size"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_cylinder" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "radius", "height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_sphere" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_cone" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "bottom_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "top_radius": { "type": "number", "minimum": 0 },
                                    "height": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "bottom_radius", "top_radius", "height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_torus" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "major_radius": { "type": "number", "exclusiveMinimum": 0 },
                                    "minor_radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "major_radius", "minor_radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_extrusion" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/profile" },
                                    "height": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "profile", "height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_revolve" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "axis_origin": { "$ref": "#/$defs/vec2" },
                                    "axis_direction": { "$ref": "#/$defs/vec2" },
                                    "angle": { "type": "number", "exclusiveMinimum": 0, "maximum": 360 }
                                },
                                "required": ["op", "id", "axis_origin", "axis_direction", "angle"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "resize_sketch" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/profile" }
                                },
                                "required": ["op", "id", "profile"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_holes" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "holes": { "$ref": "#/$defs/holes" }
                                },
                                "required": ["op", "id", "holes"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_region" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/sketch_loop" },
                                    "holes": { "$ref": "#/$defs/sketch_holes" }
                                },
                                "required": ["op", "id", "profile"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_constraints" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256
                                    }
                                },
                                "required": ["op", "id", "constraints"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "Atomically replace one exact sketch region, construction geometry, and constraints",
                                "properties": {
                                    "op": { "const": "set_sketch_definition" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "profile": { "$ref": "#/$defs/sketch_loop" },
                                    "holes": { "$ref": "#/$defs/sketch_holes" },
                                    "construction": { "$ref": "#/$defs/sketch_construction" },
                                    "constraints": {
                                        "type": "array",
                                        "items": { "$ref": "#/$defs/constraint" },
                                        "maxItems": 256
                                    }
                                },
                                "required": ["op", "id", "profile", "construction", "constraints"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_sketch_plane" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "plane": { "$ref": "#/$defs/sketch_plane" }
                                },
                                "required": ["op", "id", "plane"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_datum_plane_offset" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "offset": { "type": "number" }
                                },
                                "required": ["op", "id", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_datum_point_offset" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "offset": { "$ref": "#/$defs/vec3" }
                                },
                                "required": ["op", "id", "offset"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_chamfer_distance" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "distance": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "distance"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_fillet_radius" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "radius": { "type": "number", "exclusiveMinimum": 0 }
                                },
                                "required": ["op", "id", "radius"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "rename" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "name": { "type": "string" }
                                },
                                "required": ["op", "id", "name"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_visibility" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "visible": { "type": "boolean" }
                                },
                                "required": ["op", "id", "visible"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_color" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "color": { "$ref": "#/$defs/color" }
                                },
                                "required": ["op", "id", "color"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "set_material" },
                                    "id": { "type": "integer", "minimum": 1 },
                                    "name": { "type": "string", "minLength": 1, "maxLength": 80 },
                                    "density_kg_m3": {
                                        "type": "number",
                                        "exclusiveMinimum": 0,
                                        "maximum": 100_000,
                                        "description": "Material density in kilograms per cubic meter"
                                    }
                                },
                                "required": ["op", "id", "name", "density_kg_m3"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "clear_material" },
                                    "id": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "id"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "op": { "const": "delete" },
                                    "id": { "type": "integer", "minimum": 1 }
                                },
                                "required": ["op", "id"],
                                "additionalProperties": false
                            }
                        ]
                    }
                }
            },
            "required": ["summary", "commands"],
            "additionalProperties": false,
            "$defs": {
                "vec2": {
                    "type": "array",
                    "items": { "type": "number" },
                    "minItems": 2,
                    "maxItems": 2
                },
                "vec3": {
                    "type": "array",
                    "items": { "type": "number" },
                    "minItems": 3,
                    "maxItems": 3
                },
                "positiveVec3": {
                    "type": "array",
                    "items": { "type": "number", "exclusiveMinimum": 0 },
                    "minItems": 3,
                    "maxItems": 3
                },
                "color": {
                    "type": "array",
                    "items": { "type": "number", "minimum": 0, "maximum": 1 },
                    "minItems": 4,
                    "maxItems": 4
                },
                "profile": {
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 2,
                        "maxItems": 2
                    },
                    "minItems": 3,
                    "maxItems": 128
                },
                "holes": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/profile" },
                    "maxItems": 32
                },
                "sketch_segment": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "line" },
                                "start": { "$ref": "#/$defs/vec2" },
                                "end": { "$ref": "#/$defs/vec2" }
                            },
                            "required": ["type", "start", "end"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "arc" },
                                "start": { "$ref": "#/$defs/vec2" },
                                "end": { "$ref": "#/$defs/vec2" },
                                "center": { "$ref": "#/$defs/vec2" },
                                "ccw": { "type": "boolean" }
                            },
                            "required": ["type", "start", "end", "center", "ccw"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "rational_quadratic" },
                                "start": { "$ref": "#/$defs/vec2" },
                                "control": { "$ref": "#/$defs/vec2" },
                                "end": { "$ref": "#/$defs/vec2" },
                                "weight": { "type": "number", "exclusiveMinimum": 0 }
                            },
                            "required": ["type", "start", "control", "end", "weight"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "cubic_bezier" },
                                "start": { "$ref": "#/$defs/vec2" },
                                "control1": { "$ref": "#/$defs/vec2" },
                                "control2": { "$ref": "#/$defs/vec2" },
                                "end": { "$ref": "#/$defs/vec2" }
                            },
                            "required": ["type", "start", "control1", "control2", "end"],
                            "additionalProperties": false
                        }
                    ]
                },
                "sketch_loop": {
                    "type": "array",
                    "description": "Ordered exact segments with end-to-start closure",
                    "items": { "$ref": "#/$defs/sketch_segment" },
                    "minItems": 2,
                    "maxItems": 128
                },
                "sketch_holes": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/sketch_loop" },
                    "maxItems": 32
                },
                "sketch_construction": {
                    "type": "array",
                    "description": "Independent exact curve segments that do not form solid boundaries",
                    "items": { "$ref": "#/$defs/sketch_segment" },
                    "maxItems": 128
                },
                "sketch_region": {
                    "type": "object",
                    "properties": {
                        "profile": { "$ref": "#/$defs/sketch_loop" },
                        "holes": { "$ref": "#/$defs/sketch_holes" }
                    },
                    "required": ["profile"],
                    "additionalProperties": false
                },
                "sketch_plane": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": { "type": { "const": "world_xy" } },
                            "required": ["type"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": { "type": { "const": "world_xz" } },
                            "required": ["type"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": { "type": { "const": "world_yz" } },
                            "required": ["type"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "datum_plane" },
                                "datum_id": { "type": "integer", "minimum": 1 }
                            },
                            "required": ["type", "datum_id"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "planar_face" },
                                "face": { "$ref": "#/$defs/face_ref" }
                            },
                            "required": ["type", "face"],
                            "additionalProperties": false
                        }
                    ]
                },
                "constraint": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "coincident" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "horizontal" },
                                "segment": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "segment"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "vertical" },
                                "segment": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "segment"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "fixed" },
                                "point": { "type": "integer", "minimum": 0 },
                                "x": { "type": "number" },
                                "y": { "type": "number" }
                            },
                            "required": ["type", "point", "x", "y"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "distance" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 },
                                "distance": { "type": "number", "minimum": 0 }
                            },
                            "required": ["type", "first", "second", "distance"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the signed X delta from the first distinct point to the second",
                            "properties": {
                                "type": { "const": "horizontal_distance" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 },
                                "distance": { "type": "number" }
                            },
                            "required": ["type", "first", "second", "distance"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the signed Y delta from the first distinct point to the second",
                            "properties": {
                                "type": { "const": "vertical_distance" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 },
                                "distance": { "type": "number" }
                            },
                            "required": ["type", "first", "second", "distance"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the non-negative perpendicular distance from a point to one Line support",
                            "properties": {
                                "type": { "const": "point_line_distance" },
                                "point": { "type": "integer", "minimum": 0 },
                                "line": { "type": "integer", "minimum": 0 },
                                "distance": { "type": "number", "minimum": 0 }
                            },
                            "required": ["type", "point", "line", "distance"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require one Line support to pass through one Arc center",
                            "properties": {
                                "type": { "const": "line_through_center" },
                                "line": { "type": "integer", "minimum": 0 },
                                "arc": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "line", "arc"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Place a point on one finite Line or Arc segment",
                            "properties": {
                                "type": { "const": "point_on_curve" },
                                "point": { "type": "integer", "minimum": 0 },
                                "segment": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "point", "segment"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Place a point at the midpoint of a Line whose endpoints are different entities",
                            "properties": {
                                "type": { "const": "midpoint" },
                                "point": { "type": "integer", "minimum": 0 },
                                "segment": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "point", "segment"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Reflect two distinct points about one Line axis",
                            "properties": {
                                "type": { "const": "symmetric" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 },
                                "axis": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second", "axis"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the positive length of one Line segment",
                            "properties": {
                                "type": { "const": "length" },
                                "segment": { "type": "integer", "minimum": 0 },
                                "length": { "type": "number", "exclusiveMinimum": 0 }
                            },
                            "required": ["type", "segment", "length"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require two distinct Line segments to have equal lengths",
                            "properties": {
                                "type": { "const": "equal_length" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require two distinct Line segments to be parallel",
                            "properties": {
                                "type": { "const": "parallel" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require two distinct Line segments to be perpendicular",
                            "properties": {
                                "type": { "const": "perpendicular" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the directed angle from the first Line segment to the second in degrees",
                            "properties": {
                                "type": { "const": "angle" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 },
                                "angle_degrees": { "type": "number", "minimum": -180, "maximum": 180 }
                            },
                            "required": ["type", "first", "second", "angle_degrees"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the radius of one Arc segment",
                            "properties": {
                                "type": { "const": "radius" },
                                "segment": { "type": "integer", "minimum": 0 },
                                "radius": { "type": "number", "exclusiveMinimum": 0 }
                            },
                            "required": ["type", "segment", "radius"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Fix the center coordinates of one Arc segment",
                            "properties": {
                                "type": { "const": "fixed_center" },
                                "segment": { "type": "integer", "minimum": 0 },
                                "x": { "type": "number" },
                                "y": { "type": "number" }
                            },
                            "required": ["type", "segment", "x", "y"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require two Arc segments to have equal radii",
                            "properties": {
                                "type": { "const": "equal_radius" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require two Arc segments to share a center",
                            "properties": {
                                "type": { "const": "concentric" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require adjacent segments to be tangent; at least one must be curved",
                            "properties": {
                                "type": { "const": "tangent" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "description": "Require two adjacent curved segments to have the same traversal tangent and signed curvature",
                            "properties": {
                                "type": { "const": "curvature_continuous" },
                                "first": { "type": "integer", "minimum": 0 },
                                "second": { "type": "integer", "minimum": 0 }
                            },
                            "required": ["type", "first", "second"],
                            "additionalProperties": false
                        }
                    ]
                },
                "face_ref": {
                    "type": "object",
                    "description": "Persistent face reference from the evaluated CAD scene",
                    "properties": {
                        "feature_id": { "type": "integer", "minimum": 1 },
                        "name": {
                            "type": "object",
                            "description": "Serialized FaceName, including primitive or derived origin"
                        }
                    },
                    "required": ["feature_id", "name"],
                    "additionalProperties": false
                },
                "edge_ref": {
                    "type": "object",
                    "description": "Persistent edge reference from the evaluated CAD scene",
                    "properties": {
                        "feature_id": { "type": "integer", "minimum": 1 },
                        "adjacent_faces": {
                            "type": "array",
                            "items": { "$ref": "#/$defs/face_ref" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "fragment": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["feature_id", "adjacent_faces", "fragment"],
                    "additionalProperties": false
                },
                "vertex_ref": {
                    "type": "object",
                    "description": "Persistent vertex reference from the evaluated CAD scene",
                    "properties": {
                        "feature_id": { "type": "integer", "minimum": 1 },
                        "incident_edges": {
                            "type": "array",
                            "items": { "$ref": "#/$defs/edge_ref" },
                            "minItems": 1,
                            "uniqueItems": true
                        },
                        "fragment": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["feature_id", "incident_edges", "fragment"],
                    "additionalProperties": false
                }
            }
        }))
}

#[cfg(test)]
mod tests {
    use crate::{AiContext, AiPlan, AiRequest};
    use cadx_analysis::SceneAnalysis;
    use cadx_config::ProviderConfig;
    use cadx_core::domain::{
        BooleanOperation, Constraint, ModelCommand, SketchPlane, SketchSegment2D,
    };

    use super::*;

    #[test]
    fn tool_arguments_deserialize_into_commands() {
        let value = json!({
            "summary": "Add a mounting block",
            "commands": [{
                "op": "create_box",
                "name": "Mount",
                "size": [20.0, 10.0, 5.0],
                "position": [1.0, 2.0, 3.0]
            }]
        });

        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert_eq!(plan.commands.len(), 1);
        assert!(matches!(plan.commands[0], ModelCommand::CreateBox { .. }));
    }

    #[test]
    fn provider_configuration_controls_model_without_environment() {
        let assistant = GenAiAssistant::from_provider_config(&ProviderConfig {
            endpoint: Some("https://example.test/v1".into()),
            model: "custom-model".into(),
            api_key: Some("secret".into()),
            adapter: Some("openai".into()),
            timeout_seconds: 12,
        })
        .unwrap();
        assert_eq!(assistant.model_name(), "custom-model");
    }

    #[test]
    fn planning_prompt_contains_read_only_scene_context() {
        let prompt = build_prompt(&AiRequest {
            prompt: "inspect this part".into(),
            document: cadx_core::domain::CadDocument::default(),
            context: Some(AiContext {
                kernel_capabilities: cadx_core::kernel::CadKernelCapabilities::default(),
                selected_feature_id: Some(4),
                selected_face: None,
                selected_edges: Vec::new(),
                selected_vertex: None,
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
            }),
        })
        .unwrap();
        assert!(prompt.contains("total_volume_mm3"));
        assert!(prompt.contains("128.0"));
        assert!(prompt.contains("total_mass_kg"));
        assert!(prompt.contains("center_of_mass_mm"));
        assert!(prompt.contains("kernel_capabilities"));
        assert!(prompt.contains("read-only"));
    }

    #[test]
    fn material_commands_deserialize_for_ai_plans() {
        let value = json!({
            "summary": "Assign aluminum and clear the old tool material",
            "commands": [
                {
                    "op": "set_material",
                    "id": 4,
                    "name": "Aluminum 6061",
                    "density_kg_m3": 2700.0
                },
                {
                    "op": "clear_material",
                    "id": 3
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::SetMaterial { name, density_kg_m3, .. }
                if name == "Aluminum 6061" && (*density_kg_m3 - 2700.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::ClearMaterial { id: 3 }
        ));
    }

    #[test]
    fn planning_prompt_redacts_embedded_step_source() {
        let mut document = cadx_core::domain::CadDocument::default();
        document
            .apply(ModelCommand::ImportStep {
                name: "supplier part".into(),
                source: "sensitive-step-source".into(),
                shell_id: 42,
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
    fn invalid_provider_adapter_is_rejected_before_network_use() {
        let error = GenAiAssistant::from_provider_config(&ProviderConfig {
            adapter: Some("not-a-provider".into()),
            ..ProviderConfig::default()
        })
        .unwrap_err();
        assert!(matches!(error, AiError::Configuration(_)));
    }

    #[test]
    fn new_primitive_and_duplicate_commands_are_supported() {
        let value = json!({
            "summary": "Add a seal and a second one",
            "commands": [
                {
                    "op": "create_torus",
                    "name": "Seal",
                    "major_radius": 12.0,
                    "minor_radius": 3.0,
                    "position": [0.0, 0.0, 5.0]
                },
                {
                    "op": "create_extrusion",
                    "name": "Plate",
                    "profile": [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]],
                    "height": 8.0,
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "create_sketch",
                    "name": "Outline",
                    "profile": [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "create_extrusion_from_sketch",
                    "name": "Pad",
                    "sketch_id": 3,
                    "height": 5.0,
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "duplicate",
                    "id": 1,
                    "position": [30.0, 0.0, 5.0]
                },
                {
                    "op": "set_color",
                    "id": 2,
                    "color": [0.1, 0.2, 0.3, 1.0]
                },
                {
                    "op": "create_boolean",
                    "name": "Union",
                    "operation": "union",
                    "left": 1,
                    "right": 2
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(plan.commands[0], ModelCommand::CreateTorus { .. }));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::CreateExtrusion { .. }
        ));
        assert!(matches!(
            plan.commands[2],
            ModelCommand::CreateSketch { .. }
        ));
        assert!(matches!(
            plan.commands[3],
            ModelCommand::CreateExtrusionFromSketch { .. }
        ));
        assert!(matches!(plan.commands[4], ModelCommand::Duplicate { .. }));
        assert!(matches!(plan.commands[5], ModelCommand::SetColor { .. }));
        assert!(matches!(
            plan.commands[6],
            ModelCommand::CreateBoolean {
                operation: BooleanOperation::Union,
                left: 1,
                right: 2,
                ..
            }
        ));
    }

    #[test]
    fn sketch_constraint_commands_deserialize_for_ai_plans() {
        let value = json!({
            "summary": "Constrain the outline",
            "commands": [
                {
                    "op": "create_sketch",
                    "name": "Outline",
                    "profile": [[0.0, 0.0], [9.0, 1.0], [10.0, 5.0], [0.0, 5.0]],
                    "constraints": [
                        { "type": "horizontal", "segment": 0 },
                        { "type": "distance", "first": 0, "second": 1, "distance": 10.0 }
                    ],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "set_sketch_constraints",
                    "id": 1,
                    "constraints": [{ "type": "fixed", "point": 0, "x": 0.0, "y": 0.0 }]
                },
                {
                    "op": "set_sketch_definition",
                    "id": 1,
                    "profile": [
                        { "type": "line", "start": [0.0, 0.0], "end": [4.0, 0.0] },
                        { "type": "line", "start": [4.0, 0.0], "end": [0.0, 3.0] },
                        { "type": "line", "start": [0.0, 3.0], "end": [0.0, 0.0] }
                    ],
                    "holes": [],
                    "construction": [
                        { "type": "line", "start": [-1.0, 1.0], "end": [5.0, 1.0] }
                    ],
                    "constraints": [
                        { "type": "point_on_curve", "point": 1, "segment": 3 }
                    ]
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateSketch { constraints, .. } if constraints.len() == 2
        ));
        assert!(matches!(
            &plan.commands[1],
            ModelCommand::SetSketchConstraints { constraints, .. } if constraints.len() == 1
        ));
        assert!(matches!(
            &plan.commands[2],
            ModelCommand::SetSketchDefinition {
                construction,
                constraints,
                ..
            } if construction.len() == 1 && constraints.len() == 1
        ));
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
    fn sketch_hole_commands_deserialize_for_ai_plans() {
        let first_hole = json!([[6.0, 4.0], [10.0, 4.0], [10.0, 8.0], [6.0, 8.0]]);
        let value = json!({
            "summary": "Create a plate profile with a window",
            "commands": [
                {
                    "op": "create_sketch",
                    "name": "Window plate",
                    "profile": [[0.0, 0.0], [16.0, 0.0], [16.0, 12.0], [0.0, 12.0]],
                    "holes": [first_hole.clone()],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "set_sketch_holes",
                    "id": 1,
                    "holes": [first_hole]
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateSketch { holes, .. } if holes.len() == 1 && holes[0].len() == 4
        ));
        assert!(matches!(
            &plan.commands[1],
            ModelCommand::SetSketchHoles { holes, .. } if holes.len() == 1
        ));
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

    #[test]
    fn sketch_plane_commands_deserialize_for_ai_plans() {
        let value = json!({
            "summary": "Attach a profile to the machining datum",
            "commands": [
                {
                    "op": "create_sketch",
                    "name": "Datum profile",
                    "plane": { "type": "datum_plane", "datum_id": 8 },
                    "profile": [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]],
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "set_sketch_plane",
                    "id": 9,
                    "plane": { "type": "world_yz" }
                },
                {
                    "op": "set_sketch_plane",
                    "id": 10,
                    "plane": {
                        "type": "planar_face",
                        "face": {
                            "feature_id": 4,
                            "name": {
                                "origin": "primitive",
                                "face": { "role": "box_z_max" }
                            }
                        }
                    }
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            plan.commands[0],
            ModelCommand::CreateSketch {
                plane: SketchPlane::DatumPlane { datum_id: 8 },
                ..
            }
        ));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::SetSketchPlane {
                id: 9,
                plane: SketchPlane::WorldYz
            }
        ));
        assert!(matches!(
            &plan.commands[2],
            ModelCommand::SetSketchPlane {
                id: 10,
                plane: SketchPlane::PlanarFace { face }
            } if face.feature_id == 4
        ));
    }

    #[test]
    fn revolve_commands_deserialize_for_ai_plans() {
        let value = json!({
            "summary": "Turn the profile into a turned body",
            "commands": [
                {
                    "op": "create_revolve_from_sketch",
                    "name": "Turned body",
                    "sketch_id": 3,
                    "axis_origin": [20.0, 0.0],
                    "axis_direction": [0.0, 1.0],
                    "angle": 270.0,
                    "position": [0.0, 0.0, 0.0]
                },
                {
                    "op": "resize_revolve",
                    "id": 4,
                    "axis_origin": [25.0, 0.0],
                    "axis_direction": [0.0, 1.0],
                    "angle": 360.0
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            plan.commands[0],
            ModelCommand::CreateRevolveFromSketch { angle, .. } if (angle - 270.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::ResizeRevolve { .. }
        ));
    }

    #[test]
    fn loft_commands_deserialize_for_ai_plans() {
        let value = json!({
            "summary": "Loft three ordered exact sections",
            "commands": [{
                "op": "create_loft_from_sketches",
                "name": "Transition body",
                "sketch_ids": [3, 5, 8],
                "position": [1.0, 2.0, 3.0]
            }]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateLoftFromSketches {
                sketch_ids,
                position,
                ..
            } if sketch_ids == &[3, 5, 8]
                && position
                    .iter()
                    .zip([1.0, 2.0, 3.0])
                    .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
        ));
    }

    #[test]
    fn datum_plane_tool_arguments_deserialize_with_a_face_ref() {
        let value = json!({
            "summary": "Create a machining datum",
            "commands": [{
                "op": "create_datum_plane",
                "name": "Top datum",
                "face": {
                    "feature_id": 12,
                    "name": {
                        "origin": "primitive",
                        "face": { "role": "box_z_max" }
                    }
                },
                "offset": 0.0
            }]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateDatumPlane { face, .. } if face.feature_id == 12
        ));
    }

    #[test]
    fn datum_point_tool_arguments_deserialize_with_a_vertex_ref() {
        let face = |role| {
            json!({
                "feature_id": 12,
                "name": {
                    "origin": "primitive",
                    "face": { "role": role }
                }
            })
        };
        let value = json!({
            "summary": "Create and move a setup point",
            "commands": [
                {
                    "op": "create_datum_point",
                    "name": "Setup origin",
                    "vertex": {
                        "feature_id": 12,
                        "incident_edges": [{
                            "feature_id": 12,
                            "adjacent_faces": [face("box_x_min"), face("box_y_min")],
                            "fragment": 0
                        }],
                        "fragment": 0
                    },
                    "offset": [1.0, 2.0, 3.0]
                },
                {
                    "op": "set_datum_point_offset",
                    "id": 13,
                    "offset": [4.0, 5.0, 6.0]
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateDatumPoint { vertex, offset, .. }
                if vertex.feature_id == 12 && offset.iter().zip([1.0, 2.0, 3.0])
                    .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
        ));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::SetDatumPointOffset { id: 13, offset }
                if offset.iter().zip([4.0, 5.0, 6.0])
                    .all(|(actual, expected)| (*actual - expected).abs() < f64::EPSILON)
        ));
    }

    #[test]
    fn chamfer_tool_arguments_deserialize_with_a_persistent_edge_ref() {
        let face = |role| {
            json!({
                "feature_id": 4,
                "name": {
                    "origin": "primitive",
                    "face": { "role": role }
                }
            })
        };
        let value = json!({
            "summary": "Break the selected edge",
            "commands": [
                {
                    "op": "create_chamfer",
                    "name": "Edge break",
                    "edges": [{
                        "feature_id": 4,
                        "adjacent_faces": [face("box_x_max"), face("box_z_max")],
                        "fragment": 0
                    }],
                    "distance": 1.5
                },
                {
                    "op": "set_chamfer_distance",
                    "id": 5,
                    "distance": 2.0
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateChamfer { edges, distance, .. }
                if edges.len() == 1 && edges[0].feature_id == 4
                    && (*distance - 1.5).abs() < f64::EPSILON
        ));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::SetChamferDistance { id: 5, distance }
                if (distance - 2.0).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn fillet_tool_arguments_deserialize_with_a_persistent_edge_ref() {
        let face = |role| {
            json!({
                "feature_id": 4,
                "name": {
                    "origin": "primitive",
                    "face": { "role": role }
                }
            })
        };
        let value = json!({
            "summary": "Round the selected edge",
            "commands": [
                {
                    "op": "create_fillet",
                    "name": "Edge round",
                    "edges": [{
                        "feature_id": 4,
                        "adjacent_faces": [face("box_x_max"), face("box_z_max")],
                        "fragment": 0
                    }],
                    "radius": 1.5
                },
                {
                    "op": "set_fillet_radius",
                    "id": 5,
                    "radius": 2.0
                }
            ]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        assert!(matches!(
            &plan.commands[0],
            ModelCommand::CreateFillet { edges, radius, .. }
                if edges.len() == 1 && edges[0].feature_id == 4
                    && (*radius - 1.5).abs() < f64::EPSILON
        ));
        assert!(matches!(
            plan.commands[1],
            ModelCommand::SetFilletRadius { id: 5, radius }
                if (radius - 2.0).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn freeform_curve_plan_deserializes_and_executes_atomically() {
        let value = json!({
            "summary": "Create an exact freeform sketch",
            "commands": [{
                "op": "create_sketch_region",
                "name": "Freeform profile",
                "plane": { "type": "world_xy" },
                "profile": [
                    {
                        "type": "cubic_bezier",
                        "start": [0.0, 0.0],
                        "control1": [3.0, -2.0],
                        "control2": [7.0, -2.0],
                        "end": [10.0, 0.0]
                    },
                    { "type": "line", "start": [10.0, 0.0], "end": [10.0, 10.0] },
                    {
                        "type": "rational_quadratic",
                        "start": [10.0, 10.0],
                        "control": [5.0, 14.0],
                        "end": [0.0, 10.0],
                        "weight": 0.8
                    },
                    { "type": "line", "start": [0.0, 10.0], "end": [0.0, 0.0] }
                ],
                "holes": [],
                "construction": [],
                "constraints": [],
                "position": [0.0, 0.0, 0.0]
            }]
        });
        let plan: AiPlan = serde_json::from_value(value).unwrap();
        let mut document = cadx_core::domain::CadDocument::default();
        document.apply_transaction(plan.commands).unwrap();
        let Primitive::Sketch { region, .. } = &document.features[0].primitive else {
            panic!("expected exact freeform sketch");
        };
        assert!(matches!(
            region.profile.segments[0],
            SketchSegment2D::CubicBezier { .. }
        ));
        assert!(matches!(
            region.profile.segments[2],
            SketchSegment2D::RationalQuadratic { weight, .. } if (weight - 0.8).abs() < f64::EPSILON
        ));
    }
}
