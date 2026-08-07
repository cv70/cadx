//! Relevance-bounded projection of the CAD document into prompt context.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use cadx_core::{
    assembly::{AssemblyMate, ComponentDefinition, ComponentOccurrence},
    domain::{CadDocument, Feature, FeatureId},
};

use crate::AiRequest;

const MAX_PROMPT_FEATURES_WITHOUT_CONTEXT: usize = 64;

const MAX_PROMPT_ASSEMBLIES: usize = 8;

const MAX_PROMPT_OCCURRENCES_PER_ASSEMBLY: usize = 32;

const OCCURRENCE_SCORE_PRIORITY_STRIDE: u64 = 1_000_000;

#[derive(Serialize)]
pub(super) struct PlanningDocumentContext {
    name: String,
    next_feature_id: FeatureId,
    total_feature_count: usize,
    pub(super) features: Vec<Feature>,
    omitted_feature_count: usize,
    total_assembly_count: usize,
    assemblies: Vec<PlanningAssemblyContext>,
    omitted_assembly_count: usize,
    domain_namespaces: Vec<DomainNamespaceSummary>,
}

#[derive(Serialize)]
struct PlanningAssemblyContext {
    id: u64,
    name: String,
    total_definition_count: usize,
    definitions: Vec<ComponentDefinition>,
    omitted_definition_count: usize,
    total_occurrence_count: usize,
    occurrences: Vec<ComponentOccurrence>,
    omitted_occurrence_count: usize,
    total_mate_count: usize,
    mates: Vec<AssemblyMate>,
    omitted_mate_count: usize,
}

#[derive(Serialize)]
struct DomainNamespaceSummary {
    namespace: String,
    entry_count: usize,
}

pub(super) fn planning_document_context(request: &AiRequest) -> PlanningDocumentContext {
    let document = &request.document;
    let requested_feature_ids = request
        .context
        .as_ref()
        .map(|context| {
            context
                .interaction
                .relevant_features
                .iter()
                .map(|feature| feature.feature_id)
                .collect::<Vec<_>>()
        })
        .filter(|feature_ids| !feature_ids.is_empty())
        .unwrap_or_else(|| {
            let mut ids = document
                .features
                .iter()
                .rev()
                .take(MAX_PROMPT_FEATURES_WITHOUT_CONTEXT)
                .map(|feature| feature.id)
                .collect::<Vec<_>>();
            ids.reverse();
            ids
        });
    let mut seen_feature_ids = BTreeSet::new();
    let features = requested_feature_ids
        .iter()
        .filter(|feature_id| seen_feature_ids.insert(**feature_id))
        .filter_map(|feature_id| document.feature(*feature_id).cloned())
        .collect::<Vec<_>>();
    let included_feature_ids = features
        .iter()
        .map(|feature| feature.id)
        .collect::<BTreeSet<_>>();
    let assemblies = planning_assemblies(document, &included_feature_ids, &request.prompt);
    PlanningDocumentContext {
        name: document.name.clone(),
        next_feature_id: document.next_feature_id(),
        total_feature_count: document.features.len(),
        omitted_feature_count: document.features.len().saturating_sub(features.len()),
        features,
        total_assembly_count: document.assemblies.len(),
        omitted_assembly_count: document.assemblies.len().saturating_sub(assemblies.len()),
        assemblies,
        domain_namespaces: document
            .domain_data
            .iter()
            .map(|(namespace, entries)| DomainNamespaceSummary {
                namespace: namespace.clone(),
                entry_count: entries.len(),
            })
            .collect(),
    }
}

fn planning_assemblies(
    document: &CadDocument,
    included_feature_ids: &BTreeSet<FeatureId>,
    prompt: &str,
) -> Vec<PlanningAssemblyContext> {
    let prompt = prompt.to_lowercase();
    let assembly_intent = [
        "assembly",
        "component",
        "occurrence",
        "mate",
        "装配",
        "组件",
        "实例",
        "配合",
    ]
    .iter()
    .any(|keyword| prompt.contains(keyword));
    document
        .assemblies
        .iter()
        .filter_map(|assembly| {
            let definitions = assembly
                .definitions
                .iter()
                .map(|definition| (definition.id, definition))
                .collect::<BTreeMap<_, _>>();
            let occurrences = assembly
                .occurrences
                .iter()
                .map(|occurrence| (occurrence.id, occurrence))
                .collect::<BTreeMap<_, _>>();
            let mut scores = BTreeMap::<u64, u64>::new();
            for (index, occurrence) in assembly.occurrences.iter().enumerate() {
                if occurrence
                    .feature_ids
                    .iter()
                    .any(|feature_id| included_feature_ids.contains(feature_id))
                {
                    add_occurrence_score(&mut scores, occurrence.id, occurrence_score(0, index));
                }
                if label_matches_prompt(&prompt, &occurrence.name) {
                    add_occurrence_score(&mut scores, occurrence.id, occurrence_score(1, index));
                }
                if definitions
                    .get(&occurrence.definition_id)
                    .is_some_and(|definition| label_matches_prompt(&prompt, &definition.name))
                {
                    add_occurrence_score(&mut scores, occurrence.id, occurrence_score(2, index));
                }
            }
            if assembly_intent || label_matches_prompt(&prompt, &assembly.name) {
                for (index, occurrence) in assembly.occurrences.iter().enumerate() {
                    add_occurrence_score(&mut scores, occurrence.id, occurrence_score(6, index));
                }
            }
            if scores.is_empty() {
                return None;
            }

            let seeds = scores.keys().copied().collect::<Vec<_>>();
            for seed in seeds {
                let mut current = occurrences
                    .get(&seed)
                    .and_then(|occurrence| occurrence.parent_id);
                let mut depth = 0_usize;
                while let Some(parent_id) = current {
                    add_occurrence_score(&mut scores, parent_id, occurrence_score(3, depth));
                    current = occurrences
                        .get(&parent_id)
                        .and_then(|occurrence| occurrence.parent_id);
                    depth += 1;
                }
                for child in assembly
                    .occurrences
                    .iter()
                    .filter(|occurrence| occurrence.parent_id == Some(seed))
                {
                    add_occurrence_score(&mut scores, child.id, occurrence_score(5, 0));
                }
            }
            for mate in &assembly.mates {
                if scores.contains_key(&mate.parent_occurrence_id)
                    || scores.contains_key(&mate.child_occurrence_id)
                {
                    add_occurrence_score(
                        &mut scores,
                        mate.parent_occurrence_id,
                        occurrence_score(4, 0),
                    );
                    add_occurrence_score(
                        &mut scores,
                        mate.child_occurrence_id,
                        occurrence_score(4, 0),
                    );
                }
            }

            let mut ranked = scores.into_iter().collect::<Vec<_>>();
            ranked.sort_by_key(|(occurrence_id, score)| (*score, *occurrence_id));
            ranked.truncate(MAX_PROMPT_OCCURRENCES_PER_ASSEMBLY);
            let included_occurrence_ids = ranked
                .iter()
                .map(|(occurrence_id, _)| *occurrence_id)
                .collect::<BTreeSet<_>>();
            let included_occurrences = ranked
                .into_iter()
                .filter_map(|(occurrence_id, _)| occurrences.get(&occurrence_id).copied().cloned())
                .collect::<Vec<_>>();
            let included_definition_ids = included_occurrences
                .iter()
                .map(|occurrence| occurrence.definition_id)
                .collect::<BTreeSet<_>>();
            let included_definitions = assembly
                .definitions
                .iter()
                .filter(|definition| included_definition_ids.contains(&definition.id))
                .cloned()
                .collect::<Vec<_>>();
            let included_mates = assembly
                .mates
                .iter()
                .filter(|mate| {
                    included_occurrence_ids.contains(&mate.parent_occurrence_id)
                        && included_occurrence_ids.contains(&mate.child_occurrence_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            Some(PlanningAssemblyContext {
                id: assembly.id,
                name: assembly.name.clone(),
                total_definition_count: assembly.definitions.len(),
                omitted_definition_count: assembly
                    .definitions
                    .len()
                    .saturating_sub(included_definitions.len()),
                definitions: included_definitions,
                total_occurrence_count: assembly.occurrences.len(),
                omitted_occurrence_count: assembly
                    .occurrences
                    .len()
                    .saturating_sub(included_occurrences.len()),
                occurrences: included_occurrences,
                total_mate_count: assembly.mates.len(),
                omitted_mate_count: assembly.mates.len().saturating_sub(included_mates.len()),
                mates: included_mates,
            })
        })
        .take(MAX_PROMPT_ASSEMBLIES)
        .collect()
}

fn occurrence_score(priority: u64, tie_breaker: usize) -> u64 {
    priority
        .saturating_mul(OCCURRENCE_SCORE_PRIORITY_STRIDE)
        .saturating_add(tie_breaker as u64)
}

fn add_occurrence_score(scores: &mut BTreeMap<u64, u64>, occurrence_id: u64, score: u64) {
    scores
        .entry(occurrence_id)
        .and_modify(|current| *current = (*current).min(score))
        .or_insert(score);
}

fn label_matches_prompt(prompt: &str, label: &str) -> bool {
    let label = label.trim().to_lowercase();
    label.chars().count() >= 2 && prompt.contains(&label)
}

#[cfg(test)]
mod tests {
    use crate::AiContext;
    use cadx_analysis::SceneAnalysis;
    use cadx_core::{
        assembly::{AssemblyMateKind, AssemblyTransform, ComponentKind},
        domain::ModelCommand,
    };

    use super::*;

    fn add_box(document: &mut CadDocument, name: &str, size: f64) -> FeatureId {
        document
            .apply(ModelCommand::CreateBox {
                name: name.into(),
                size: [size; 3],
                position: [size, 0.0, 0.0],
            })
            .unwrap()
            .unwrap()
    }

    fn context_with_interaction(interaction: crate::context::ContextSnapshot) -> AiContext {
        AiContext {
            interaction,
            kernel_capabilities: cadx_core::kernel::CadKernelCapabilities::default(),
            measurement: None,
            last_boolean_failure: None,
            last_edge_modifier_failure: None,
            last_sketch_failure: None,
            selected_sketch_diagnostic: None,
            selected_sketch_dimensions: Vec::new(),
            scene_analysis: SceneAnalysis::default(),
            interference_analysis: None,
        }
    }

    #[test]
    fn planning_document_only_serializes_retrieved_feature_parameters() {
        let mut document = CadDocument::default();
        let mut feature_ids = Vec::new();
        for index in 0..70 {
            let name = if index == 5 {
                "Target bracket".into()
            } else {
                format!("Body {index}")
            };
            feature_ids.push(add_box(&mut document, &name, f64::from(index) + 1.0));
        }
        let analysis = SceneAnalysis::default();
        let collector =
            crate::context::ContextCollector::collect(crate::context::ContextCollectionInput {
                domain: None,
                document_revision: 3,
                prompt: "resize the target bracket",
                document: &document,
                scene_analysis: &analysis,
                selection: crate::context::ContextSelection {
                    selected_feature_id: Some(feature_ids[1]),
                    ..crate::context::ContextSelection::default()
                },
                viewport: crate::context::ViewportContext::default(),
                domain_schema: serde_json::Map::new(),
                budget: crate::context::ContextBudget {
                    feature_details: 2,
                    spatial_entities: 1,
                },
            })
            .unwrap();
        let request = AiRequest {
            prompt: "resize the target bracket".into(),
            document,
            context: Some(context_with_interaction(collector.into_snapshot())),
        };

        let planning = planning_document_context(&request);
        let included_ids = planning
            .features
            .iter()
            .map(|feature| feature.id)
            .collect::<Vec<_>>();
        assert_eq!(included_ids, vec![feature_ids[1], feature_ids[5]]);
        assert_eq!(planning.total_feature_count, 70);
        assert_eq!(planning.omitted_feature_count, 68);

        let encoded = serde_json::to_value(planning).unwrap();
        assert_eq!(encoded["features"].as_array().unwrap().len(), 2);
        assert_eq!(encoded["features"][0]["primitive"]["size"]["x"], 2.0);
        assert_eq!(encoded["features"][1]["primitive"]["size"]["x"], 6.0);
        assert!(!encoded.to_string().contains("Body 69"));
    }

    #[test]
    fn planning_document_includes_related_assembly_ancestors_and_mates() {
        let mut document = CadDocument::default();
        let selected_feature = add_box(&mut document, "Selected gear", 4.0);
        let unrelated_feature = add_box(&mut document, "Unrelated cover", 8.0);
        document
            .apply(ModelCommand::Move {
                id: selected_feature,
                position: [0.0; 3],
            })
            .unwrap();
        document
            .apply(ModelCommand::Move {
                id: unrelated_feature,
                position: [0.0; 3],
            })
            .unwrap();
        document
            .apply(ModelCommand::CreateAssembly {
                name: "Gearbox".into(),
                definitions: vec![
                    ComponentDefinition {
                        id: 1,
                        name: "Gearbox root".into(),
                        kind: ComponentKind::Assembly,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 2,
                        name: "Gear".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                    ComponentDefinition {
                        id: 3,
                        name: "Cover".into(),
                        kind: ComponentKind::Part,
                        source: None,
                    },
                ],
                occurrences: vec![
                    ComponentOccurrence {
                        id: 1,
                        name: "Gearbox root".into(),
                        definition_id: 1,
                        parent_id: None,
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: Vec::new(),
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 2,
                        name: "Selected gear occurrence".into(),
                        definition_id: 2,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: vec![selected_feature],
                        source: None,
                    },
                    ComponentOccurrence {
                        id: 3,
                        name: "Unrelated cover occurrence".into(),
                        definition_id: 3,
                        parent_id: Some(1),
                        suppressed: false,
                        transform: AssemblyTransform::IDENTITY,
                        feature_ids: vec![unrelated_feature],
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
                    name: "Fixed gear".into(),
                    parent_occurrence_id: 1,
                    child_occurrence_id: 2,
                    parent_frame: AssemblyTransform::IDENTITY,
                    child_frame: AssemblyTransform::IDENTITY,
                    kind: AssemblyMateKind::Fixed,
                    state: 0.0,
                },
            })
            .unwrap();
        let interaction = crate::context::ContextSnapshot {
            relevant_features: vec![crate::context::ContextFeature {
                feature_id: selected_feature,
                name: "Selected gear".into(),
                primitive: "Box".into(),
                visible: true,
                position_mm: [4.0, 0.0, 0.0],
                rotation_degrees: [0.0; 3],
                dependencies: Vec::new(),
                direct_dependents: Vec::new(),
                relevance: vec![crate::context::ContextRelevance::Selected],
                assembly: None,
            }],
            omitted_feature_count: 1,
            ..crate::context::ContextSnapshot::default()
        };
        let request = AiRequest {
            prompt: "adjust the selected gear".into(),
            document,
            context: Some(context_with_interaction(interaction)),
        };

        let planning = planning_document_context(&request);
        assert_eq!(planning.assemblies.len(), 1);
        let assembly = &planning.assemblies[0];
        assert_eq!(
            assembly
                .occurrences
                .iter()
                .map(|occurrence| occurrence.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(assembly.omitted_occurrence_count, 1);
        assert_eq!(assembly.mates.len(), 1);
        assert_eq!(assembly.mates[0].id, 1);
        assert!(
            assembly
                .occurrences
                .iter()
                .all(|occurrence| occurrence.id != 3)
        );
    }
}
