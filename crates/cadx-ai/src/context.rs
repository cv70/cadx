//! Bounded, read-only context retrieval for the AI-native layer.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cadx_analysis::{BoundingBox, SceneAnalysis};
use cadx_core::{
    assembly::{AssemblyId, ComponentDefinitionId, ComponentOccurrenceId},
    domain::{CadDocument, FeatureId},
    topology::{EdgeRef, FaceRef, VertexRef},
};
use cadx_domain_api::DomainId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

pub const MAX_CONTEXT_FEATURE_DETAILS: usize = 64;
pub const MAX_CONTEXT_SPATIAL_ENTITIES: usize = 32;
pub const MAX_CONTEXT_SELECTED_EDGES: usize = 64;
const DEFAULT_CONTEXT_FEATURE_DETAILS: usize = 32;
const DEFAULT_CONTEXT_SPATIAL_ENTITIES: usize = 16;
const MAX_DOMAIN_SCHEMA_BYTES: usize = 64 * 1024;
const GRAPH_RETRIEVAL_DEPTH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub feature_details: usize,
    pub spatial_entities: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            feature_details: DEFAULT_CONTEXT_FEATURE_DETAILS,
            spatial_entities: DEFAULT_CONTEXT_SPATIAL_ENTITIES,
        }
    }
}

impl ContextBudget {
    fn validate(self) -> Result<Self, ContextCollectionError> {
        if !(1..=MAX_CONTEXT_FEATURE_DETAILS).contains(&self.feature_details) {
            return Err(ContextCollectionError::InvalidFeatureBudget(
                self.feature_details,
            ));
        }
        if !(1..=MAX_CONTEXT_SPATIAL_ENTITIES).contains(&self.spatial_entities) {
            return Err(ContextCollectionError::InvalidSpatialBudget(
                self.spatial_entities,
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContextSelection {
    pub selected_feature_id: Option<FeatureId>,
    pub selected_face: Option<FaceRef>,
    pub selected_edges: Vec<EdgeRef>,
    pub selected_vertex: Option<VertexRef>,
    /// Model-space witness for the most specific resolved topology selection.
    pub focus_point_mm: Option<[f64; 3]>,
    #[serde(default)]
    pub omitted_edge_count: usize,
}

impl ContextSelection {
    fn feature_ids(&self) -> BTreeSet<FeatureId> {
        self.selected_feature_id
            .into_iter()
            .chain(
                self.selected_face
                    .as_ref()
                    .map(|reference| reference.feature_id),
            )
            .chain(
                self.selected_edges
                    .iter()
                    .map(|reference| reference.feature_id),
            )
            .chain(
                self.selected_vertex
                    .as_ref()
                    .map(|reference| reference.feature_id),
            )
            .collect()
    }

    fn validate_and_bound(&mut self) -> Result<(), ContextCollectionError> {
        if self
            .focus_point_mm
            .is_some_and(|point| !finite_point(point))
        {
            return Err(ContextCollectionError::InvalidFocusPoint);
        }
        if self.selected_edges.len() > MAX_CONTEXT_SELECTED_EDGES {
            self.omitted_edge_count = self
                .omitted_edge_count
                .saturating_add(self.selected_edges.len() - MAX_CONTEXT_SELECTED_EDGES);
            self.selected_edges.truncate(MAX_CONTEXT_SELECTED_EDGES);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewportContext {
    pub target_mm: [f64; 3],
    pub camera_distance_mm: f64,
    pub yaw_degrees: f64,
    pub pitch_degrees: f64,
}

impl Default for ViewportContext {
    fn default() -> Self {
        Self {
            target_mm: [0.0; 3],
            camera_distance_mm: 1.0,
            yaw_degrees: 0.0,
            pitch_degrees: 0.0,
        }
    }
}

impl ViewportContext {
    fn validate(self) -> Result<Self, ContextCollectionError> {
        if !finite_point(self.target_mm)
            || !self.camera_distance_mm.is_finite()
            || self.camera_distance_mm <= 0.0
            || !self.yaw_degrees.is_finite()
            || !self.pitch_degrees.is_finite()
        {
            return Err(ContextCollectionError::InvalidViewport);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRelevance {
    Selected,
    PromptMatch,
    Dependency,
    Dependent,
    Spatial,
    Recent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAssemblyFeature {
    pub assembly_id: AssemblyId,
    pub definition_id: ComponentDefinitionId,
    pub occurrence_id: ComponentOccurrenceId,
    pub parent_occurrence_id: Option<ComponentOccurrenceId>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextFeature {
    pub feature_id: FeatureId,
    pub name: String,
    pub primitive: String,
    pub visible: bool,
    pub position_mm: [f64; 3],
    pub rotation_degrees: [f64; 3],
    pub dependencies: Vec<FeatureId>,
    pub direct_dependents: Vec<FeatureId>,
    pub relevance: Vec<ContextRelevance>,
    pub assembly: Option<ContextAssemblyFeature>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpatialContextEntity {
    pub feature_id: FeatureId,
    pub name: String,
    pub bounds: BoundingBox,
    pub centroid_mm: [f64; 3],
    pub distance_to_focus_mm: f64,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub domain: Option<DomainId>,
    pub document_revision: u64,
    pub document_name: String,
    pub active_feature_count: usize,
    pub visible_solid_count: usize,
    pub selection: ContextSelection,
    pub viewport: ViewportContext,
    pub relevant_features: Vec<ContextFeature>,
    pub omitted_feature_count: usize,
    pub spatial_entities: Vec<SpatialContextEntity>,
    pub omitted_spatial_entity_count: usize,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub domain_schema: Map<String, Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub domain_schema_omitted: bool,
}

impl Default for ContextSnapshot {
    fn default() -> Self {
        Self {
            domain: None,
            document_revision: 0,
            document_name: String::new(),
            active_feature_count: 0,
            visible_solid_count: 0,
            selection: ContextSelection::default(),
            viewport: ViewportContext::default(),
            relevant_features: Vec::new(),
            omitted_feature_count: 0,
            spatial_entities: Vec::new(),
            omitted_spatial_entity_count: 0,
            domain_schema: Map::new(),
            domain_schema_omitted: false,
        }
    }
}

pub struct ContextCollectionInput<'a> {
    pub domain: Option<DomainId>,
    pub document_revision: u64,
    pub prompt: &'a str,
    pub document: &'a CadDocument,
    pub scene_analysis: &'a SceneAnalysis,
    pub selection: ContextSelection,
    pub viewport: ViewportContext,
    pub domain_schema: Map<String, Value>,
    pub budget: ContextBudget,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ContextCollectionError {
    #[error("AI context feature budget {0} must be between 1 and {MAX_CONTEXT_FEATURE_DETAILS}")]
    InvalidFeatureBudget(usize),
    #[error("AI context spatial budget {0} must be between 1 and {MAX_CONTEXT_SPATIAL_ENTITIES}")]
    InvalidSpatialBudget(usize),
    #[error("AI context viewport contains a non-finite or non-positive value")]
    InvalidViewport,
    #[error("AI context selection focus point is non-finite")]
    InvalidFocusPoint,
}

/// Collects deterministic host-provided state without retaining kernel handles.
#[derive(Debug, Clone, Default)]
pub struct ContextCollector {
    snapshot: ContextSnapshot,
}

impl ContextCollector {
    /// Retrieves a bounded context snapshot from one immutable document revision.
    ///
    /// # Errors
    ///
    /// Returns `ContextCollectionError` when host focus values or requested
    /// budgets are invalid.
    pub fn collect(mut input: ContextCollectionInput<'_>) -> Result<Self, ContextCollectionError> {
        let budget = input.budget.validate()?;
        let viewport = input.viewport.validate()?;
        input.selection.validate_and_bound()?;
        let selected_feature_ids = input.selection.feature_ids();
        let focus_point = input.selection.focus_point_mm.unwrap_or(viewport.target_mm);

        let mut spatial_entities = input
            .scene_analysis
            .parts
            .iter()
            .map(|part| SpatialContextEntity {
                feature_id: part.feature_id,
                name: part.name.clone(),
                bounds: part.bounds,
                centroid_mm: part.centroid_mm,
                distance_to_focus_mm: distance_to_bounds(focus_point, part.bounds),
                selected: selected_feature_ids.contains(&part.feature_id),
            })
            .collect::<Vec<_>>();
        spatial_entities.sort_by(|first, second| {
            second
                .selected
                .cmp(&first.selected)
                .then_with(|| {
                    first
                        .distance_to_focus_mm
                        .total_cmp(&second.distance_to_focus_mm)
                })
                .then_with(|| first.feature_id.cmp(&second.feature_id))
        });

        let mut scores = BTreeMap::<FeatureId, u64>::new();
        let mut relevance = BTreeMap::<FeatureId, BTreeSet<ContextRelevance>>::new();
        for feature_id in &selected_feature_ids {
            add_candidate(
                &mut scores,
                &mut relevance,
                *feature_id,
                0,
                ContextRelevance::Selected,
            );
        }

        let prompt_matches = input
            .document
            .features
            .iter()
            .filter(|feature| prompt_matches_feature(input.prompt, feature.id, &feature.name))
            .map(|feature| feature.id)
            .collect::<Vec<_>>();
        for (index, feature_id) in prompt_matches.iter().copied().enumerate() {
            add_candidate(
                &mut scores,
                &mut relevance,
                feature_id,
                1_000 + index as u64,
                ContextRelevance::PromptMatch,
            );
        }

        let graph_roots = selected_feature_ids
            .iter()
            .copied()
            .chain(prompt_matches.iter().copied())
            .collect::<BTreeSet<_>>();
        add_graph_context(input.document, &graph_roots, &mut scores, &mut relevance);

        for (index, entity) in spatial_entities.iter().enumerate() {
            add_candidate(
                &mut scores,
                &mut relevance,
                entity.feature_id,
                3_000 + index as u64,
                ContextRelevance::Spatial,
            );
        }
        for (index, feature) in input.document.features.iter().rev().enumerate() {
            add_candidate(
                &mut scores,
                &mut relevance,
                feature.id,
                4_000 + index as u64,
                ContextRelevance::Recent,
            );
        }

        let mut ranked_features = scores.into_iter().collect::<Vec<_>>();
        ranked_features.sort_by_key(|(feature_id, score)| (*score, *feature_id));
        ranked_features.truncate(budget.feature_details);
        let relevant_features = ranked_features
            .into_iter()
            .filter_map(|(feature_id, _)| {
                let feature = input.document.feature(feature_id)?;
                let assembly = input
                    .document
                    .assembly_occurrence_for_feature(feature_id)
                    .and_then(|(assembly, occurrence)| {
                        input
                            .document
                            .assembly_feature_instance(feature_id)
                            .map(|instance| ContextAssemblyFeature {
                                assembly_id: assembly.id,
                                definition_id: instance.definition_id,
                                occurrence_id: occurrence.id,
                                parent_occurrence_id: occurrence.parent_id,
                                suppressed: occurrence.suppressed,
                            })
                    });
                Some(ContextFeature {
                    feature_id,
                    name: feature.name.clone(),
                    primitive: feature.primitive.label().into(),
                    visible: feature.visible,
                    position_mm: feature.translation.as_array(),
                    rotation_degrees: feature.rotation.as_array(),
                    dependencies: feature.primitive.dependencies(),
                    direct_dependents: input
                        .document
                        .dependents(feature_id)
                        .map(|dependent| dependent.id)
                        .collect(),
                    relevance: relevance
                        .remove(&feature_id)
                        .map_or_else(Vec::new, |reasons| reasons.into_iter().collect()),
                    assembly,
                })
            })
            .collect::<Vec<_>>();

        let omitted_feature_count = input
            .document
            .features
            .len()
            .saturating_sub(relevant_features.len());
        let omitted_spatial_entity_count = spatial_entities
            .len()
            .saturating_sub(budget.spatial_entities);
        spatial_entities.truncate(budget.spatial_entities);

        let domain_schema_omitted = serde_json::to_vec(&input.domain_schema)
            .is_ok_and(|encoded| encoded.len() > MAX_DOMAIN_SCHEMA_BYTES);
        let domain_schema = if domain_schema_omitted {
            Map::new()
        } else {
            input.domain_schema
        };
        Ok(Self {
            snapshot: ContextSnapshot {
                domain: input.domain,
                document_revision: input.document_revision,
                document_name: input.document.name.clone(),
                active_feature_count: input.document.features.len(),
                visible_solid_count: input.scene_analysis.parts.len(),
                selection: input.selection,
                viewport,
                relevant_features,
                omitted_feature_count,
                spatial_entities,
                omitted_spatial_entity_count,
                domain_schema,
                domain_schema_omitted,
            },
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> &ContextSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn into_snapshot(self) -> ContextSnapshot {
        self.snapshot
    }

    /// Retains aggregate engineering values while bounding per-part detail to
    /// the retrieved spatial neighborhood.
    #[must_use]
    pub fn filter_scene_analysis(&self, mut analysis: SceneAnalysis) -> SceneAnalysis {
        let included = self
            .snapshot
            .spatial_entities
            .iter()
            .map(|entity| entity.feature_id)
            .collect::<BTreeSet<_>>();
        analysis
            .parts
            .retain(|part| included.contains(&part.feature_id));
        analysis
    }

    #[must_use]
    pub fn as_json(&self) -> Value {
        serde_json::to_value(&self.snapshot).unwrap_or(Value::Null)
    }
}

fn add_graph_context(
    document: &CadDocument,
    roots: &BTreeSet<FeatureId>,
    scores: &mut BTreeMap<FeatureId, u64>,
    relevance: &mut BTreeMap<FeatureId, BTreeSet<ContextRelevance>>,
) {
    let mut queue = roots
        .iter()
        .copied()
        .map(|feature_id| (feature_id, 0_usize))
        .collect::<VecDeque<_>>();
    let mut visited = roots.clone();
    while let Some((feature_id, depth)) = queue.pop_front() {
        if depth >= GRAPH_RETRIEVAL_DEPTH {
            continue;
        }
        let next_depth = depth + 1;
        let Some(feature) = document.feature(feature_id) else {
            continue;
        };
        for dependency in feature.primitive.dependencies() {
            add_candidate(
                scores,
                relevance,
                dependency,
                2_000 + next_depth as u64 * 10,
                ContextRelevance::Dependency,
            );
            if visited.insert(dependency) {
                queue.push_back((dependency, next_depth));
            }
        }
        for dependent in document.dependents(feature_id) {
            add_candidate(
                scores,
                relevance,
                dependent.id,
                2_005 + next_depth as u64 * 10,
                ContextRelevance::Dependent,
            );
            if visited.insert(dependent.id) {
                queue.push_back((dependent.id, next_depth));
            }
        }
    }
}

fn add_candidate(
    scores: &mut BTreeMap<FeatureId, u64>,
    relevance: &mut BTreeMap<FeatureId, BTreeSet<ContextRelevance>>,
    feature_id: FeatureId,
    score: u64,
    reason: ContextRelevance,
) {
    scores
        .entry(feature_id)
        .and_modify(|current| *current = (*current).min(score))
        .or_insert(score);
    relevance.entry(feature_id).or_default().insert(reason);
}

fn prompt_matches_feature(prompt: &str, feature_id: FeatureId, name: &str) -> bool {
    let prompt = prompt.to_lowercase();
    let name = name.trim().to_lowercase();
    if name.chars().count() >= 2 && prompt.contains(&name) {
        return true;
    }
    let tokens = name
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2);
    if tokens.into_iter().any(|token| prompt.contains(token)) {
        return true;
    }
    [
        format!("#{feature_id}"),
        format!("feature {feature_id}"),
        format!("feature #{feature_id}"),
        format!("特征{feature_id}"),
        format!("特征 #{feature_id}"),
    ]
    .iter()
    .any(|needle| prompt.contains(needle))
}

fn finite_point(point: [f64; 3]) -> bool {
    point.into_iter().all(f64::is_finite)
}

fn distance_to_bounds(point: [f64; 3], bounds: BoundingBox) -> f64 {
    point
        .into_iter()
        .enumerate()
        .map(|(axis, coordinate)| {
            if coordinate < bounds.min[axis] {
                bounds.min[axis] - coordinate
            } else if coordinate > bounds.max[axis] {
                coordinate - bounds.max[axis]
            } else {
                0.0
            }
        })
        .map(|delta| delta * delta)
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use cadx_analysis::PartAnalysis;
    use cadx_core::domain::{BooleanOperation, CadDocument, ModelCommand};

    use super::*;

    fn add_box(document: &mut CadDocument, name: &str, x: f64) -> FeatureId {
        document
            .apply(ModelCommand::CreateBox {
                name: name.into(),
                size: [2.0; 3],
                position: [x, 0.0, 0.0],
            })
            .unwrap()
            .unwrap()
    }

    fn analysis(document: &CadDocument) -> SceneAnalysis {
        let parts = document
            .features
            .iter()
            .map(|feature| {
                let center = feature.translation.as_array();
                PartAnalysis {
                    feature_id: feature.id,
                    name: feature.name.clone(),
                    triangle_count: 12,
                    surface_area_mm2: 24.0,
                    volume_mm3: 8.0,
                    centroid_mm: center,
                    bounds: BoundingBox {
                        min: [center[0] - 1.0, -1.0, -1.0],
                        max: [center[0] + 1.0, 1.0, 1.0],
                    },
                    material: None,
                    density_kg_m3: None,
                    mass_kg: None,
                    inertia_centroid_kg_mm2: None,
                }
            })
            .collect::<Vec<_>>();
        let total_surface_area_mm2 = parts.iter().map(|part| part.surface_area_mm2).sum();
        let total_volume_mm3 = parts.iter().map(|part| part.volume_mm3).sum();
        SceneAnalysis {
            parts,
            total_surface_area_mm2,
            total_volume_mm3,
            ..SceneAnalysis::default()
        }
    }

    fn input<'a>(
        prompt: &'a str,
        document: &'a CadDocument,
        analysis: &'a SceneAnalysis,
    ) -> ContextCollectionInput<'a> {
        ContextCollectionInput {
            domain: Some(DomainId::Mcad),
            document_revision: 9,
            prompt,
            document,
            scene_analysis: analysis,
            selection: ContextSelection::default(),
            viewport: ViewportContext {
                target_mm: [0.0; 3],
                camera_distance_mm: 100.0,
                yaw_degrees: 30.0,
                pitch_degrees: -20.0,
            },
            domain_schema: Map::new(),
            budget: ContextBudget::default(),
        }
    }

    #[test]
    fn selected_prompt_graph_and_spatial_context_precede_recent_fallback() {
        let mut document = CadDocument::default();
        let source = add_box(&mut document, "Source plate", 50.0);
        let second_source = add_box(&mut document, "Source boss", 55.0);
        let selected = document
            .apply(ModelCommand::CreateBoolean {
                name: "Selected result".into(),
                operation: BooleanOperation::Union,
                left: source,
                right: second_source,
            })
            .unwrap()
            .unwrap();
        let prompt_match = add_box(&mut document, "Target bracket", 100.0);
        let nearest = add_box(&mut document, "Nearest", 0.0);
        for index in 0..8 {
            add_box(
                &mut document,
                &format!("Recent {index}"),
                200.0 + f64::from(index),
            );
        }
        let analysis = analysis(&document);
        let mut collection_input = input("resize the target bracket", &document, &analysis);
        collection_input.selection.selected_feature_id = Some(selected);
        collection_input.selection.focus_point_mm = Some([0.0; 3]);
        collection_input.budget.feature_details = 5;
        let snapshot = ContextCollector::collect(collection_input)
            .unwrap()
            .into_snapshot();
        let ids = snapshot
            .relevant_features
            .iter()
            .map(|feature| feature.feature_id)
            .collect::<Vec<_>>();

        assert_eq!(ids[0], selected);
        assert_eq!(ids[1], prompt_match);
        assert!(ids.contains(&source));
        assert!(ids.contains(&second_source));
        assert!(ids.contains(&nearest));
        assert_eq!(snapshot.omitted_feature_count, document.features.len() - 5);
        assert!(
            snapshot.relevant_features[0]
                .relevance
                .contains(&ContextRelevance::Selected)
        );
        assert!(
            snapshot
                .relevant_features
                .iter()
                .find(|feature| feature.feature_id == source)
                .unwrap()
                .relevance
                .contains(&ContextRelevance::Dependency)
        );
    }

    #[test]
    fn spatial_entities_are_deterministic_selected_first_and_bounded() {
        let mut document = CadDocument::default();
        let far = add_box(&mut document, "Far selected", 100.0);
        let near = add_box(&mut document, "Near", 2.0);
        let middle = add_box(&mut document, "Middle", 10.0);
        let analysis = analysis(&document);
        let mut collection_input = input("", &document, &analysis);
        collection_input.selection.selected_feature_id = Some(far);
        collection_input.budget.spatial_entities = 2;
        let collector = ContextCollector::collect(collection_input).unwrap();
        let snapshot = collector.snapshot();

        assert_eq!(snapshot.spatial_entities[0].feature_id, far);
        assert_eq!(snapshot.spatial_entities[1].feature_id, near);
        assert_eq!(snapshot.omitted_spatial_entity_count, 1);
        let filtered = collector.filter_scene_analysis(analysis);
        assert_eq!(filtered.parts.len(), 2);
        assert!((filtered.total_volume_mm3 - 24.0).abs() < f64::EPSILON);
        assert!(!filtered.parts.iter().any(|part| part.feature_id == middle));
    }

    #[test]
    fn topology_selection_and_viewport_are_typed_and_serializable() {
        let mut document = CadDocument::default();
        let feature_id = add_box(&mut document, "Body", 0.0);
        let analysis = analysis(&document);
        let mut collection_input = input("", &document, &analysis);
        collection_input.selection.selected_face = Some(FaceRef::primitive(
            feature_id,
            cadx_core::topology::PrimitiveFace::BoxZMax,
        ));
        collection_input.selection.focus_point_mm = Some([0.0, 0.0, 1.0]);
        let collector = ContextCollector::collect(collection_input).unwrap();
        let json = collector.as_json();

        assert_eq!(json["document_revision"], 9);
        assert_eq!(json["selection"]["selected_face"]["feature_id"], feature_id);
        assert_eq!(json["selection"]["focus_point_mm"][2], 1.0);
        assert_eq!(json["viewport"]["camera_distance_mm"], 100.0);
    }

    #[test]
    fn invalid_host_values_and_budgets_fail_closed() {
        let document = CadDocument::default();
        let analysis = SceneAnalysis::default();
        let mut invalid_budget = input("", &document, &analysis);
        invalid_budget.budget.feature_details = 0;
        assert_eq!(
            ContextCollector::collect(invalid_budget).unwrap_err(),
            ContextCollectionError::InvalidFeatureBudget(0)
        );

        let mut invalid_viewport = input("", &document, &analysis);
        invalid_viewport.viewport.camera_distance_mm = f64::NAN;
        assert_eq!(
            ContextCollector::collect(invalid_viewport).unwrap_err(),
            ContextCollectionError::InvalidViewport
        );
    }
}
