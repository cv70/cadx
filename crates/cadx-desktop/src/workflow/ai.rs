use std::sync::Arc;

use cadx_ai::{
    AiContext, AiRequest, AiSketchDimension, DomainAiRequest,
    context::{
        ContextBudget, ContextCollectionInput, ContextCollector, ContextSelection, ViewportContext,
    },
};
use cadx_analysis::{SceneAnalysis, analyze_scene, compare_scenes};
use cadx_app::{TransactionMetadata, TransactionSource};
use eframe::egui;

use crate::{
    AiTaskResponse, CadxApp, ConversationEntry, LocalizedText, PendingAiCandidate, Speaker,
    StatusMessage,
};

impl CadxApp {
    pub(crate) fn request_ai_plan(&mut self, context: &egui::Context) {
        let prompt = self.ai_input.trim().to_owned();
        if prompt.is_empty() || self.ai_tasks.is_pending() {
            return;
        }
        self.ai_input.clear();
        let routed_domain = self.route_domain_prompt(&prompt);
        self.conversation.push(ConversationEntry {
            speaker: Speaker::User,
            content: LocalizedText::Text(prompt.clone()),
        });
        let scene_analysis = match analyze_scene(self.session.scene(), None) {
            Ok(analysis) => analysis,
            Err(error) => {
                self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(error.to_string()),
                });
                return;
            }
        };
        let collector = match self.collect_dynamic_ai_context(&prompt, &scene_analysis) {
            Ok(collector) => collector,
            Err(error) => {
                self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(error.to_string()),
                });
                return;
            }
        };
        self.context_collector = collector.clone();
        let assistant = Arc::clone(&self.assistant);
        let sender = self.ai_sender.clone();
        let base_revision = self.session.revision();
        let request_id = self.ai_tasks.reserve_request_id();
        let repaint = context.clone();
        let task =
            if let Some(domain) = routed_domain {
                let domain_context = self.bounded_domain_context(&collector);
                let tools = self
                    .ai_tools
                    .ai_tools_for(domain)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();
                if tools.is_empty() {
                    self.conversation.push(ConversationEntry {
                        speaker: Speaker::Error,
                        content: LocalizedText::Text(format!(
                            "No AI tools are registered for {}",
                            domain.slug()
                        )),
                    });
                    return;
                }
                let response_context = domain_context.clone();
                let request = DomainAiRequest {
                    prompt,
                    domain,
                    context: domain_context,
                    tools,
                };
                self.runtime.spawn(async move {
                    let result = assistant.plan_domain(request).await.map(|plan| {
                        crate::AiTaskResult::Domain {
                            plan,
                            context: response_context,
                        }
                    });
                    let _ = sender.send(AiTaskResponse {
                        request_id,
                        base_revision,
                        result,
                    });
                    repaint.request_repaint();
                })
            } else {
                let measurement = self
                    .measurement
                    .active
                    .then(|| self.measurement.result(self.session.scene()))
                    .and_then(Result::ok);
                let capabilities = self.session.kernel_capabilities();
                let interference_analysis = capabilities
                    .interference_analysis
                    .then(|| self.session.analyze_interference())
                    .and_then(Result::ok);
                let request = AiRequest {
                    prompt,
                    document: self.session.document().clone(),
                    context: Some(AiContext {
                        interaction: collector.snapshot().clone(),
                        kernel_capabilities: capabilities,
                        measurement,
                        last_boolean_failure: self.last_boolean_failure.clone(),
                        last_edge_modifier_failure: self.last_edge_modifier_failure.clone(),
                        last_sketch_failure: self.last_sketch_failure.clone(),
                        selected_sketch_diagnostic: self
                            .selected
                            .and_then(|id| self.session.scene().sketch_diagnostic(id))
                            .cloned(),
                        selected_sketch_dimensions: self
                            .selected
                            .and_then(|id| {
                                self.session
                                    .scene()
                                    .sketches
                                    .iter()
                                    .find(|sketch| sketch.feature_id == id)
                            })
                            .into_iter()
                            .flat_map(|sketch| &sketch.constraint_annotations)
                            .filter_map(|annotation| {
                                let dimension = annotation.constraint.dimension()?;
                                Some(AiSketchDimension {
                                    constraint_index: annotation.constraint_index,
                                    kind: dimension.kind,
                                    value: dimension.value,
                                })
                            })
                            .collect(),
                        scene_analysis: collector.filter_scene_analysis(scene_analysis),
                        interference_analysis,
                    }),
                };
                self.runtime.spawn(async move {
                    let result = assistant.plan(request).await.map(crate::AiTaskResult::Cad);
                    let _ = sender.send(AiTaskResponse {
                        request_id,
                        base_revision,
                        result,
                    });
                    repaint.request_repaint();
                })
            };
        self.ai_tasks
            .track(request_id, base_revision, task.abort_handle());
    }

    fn bounded_domain_context(
        &self,
        collector: &ContextCollector,
    ) -> cadx_domain_api::DomainContext {
        let snapshot = collector.snapshot();
        let mut selected_feature_ids = snapshot
            .selection
            .selected_feature_id
            .into_iter()
            .chain(
                snapshot
                    .selection
                    .selected_face
                    .as_ref()
                    .map(|reference| reference.feature_id),
            )
            .chain(
                snapshot
                    .selection
                    .selected_edges
                    .iter()
                    .map(|reference| reference.feature_id),
            )
            .chain(
                snapshot
                    .selection
                    .selected_vertex
                    .as_ref()
                    .map(|reference| reference.feature_id),
            )
            .collect::<Vec<_>>();
        selected_feature_ids.sort_unstable();
        selected_feature_ids.dedup();
        cadx_domain_api::DomainContext {
            document_name: snapshot.document_name.clone(),
            selected_feature_name: selected_feature_ids.first().and_then(|id| {
                self.session
                    .document()
                    .feature(*id)
                    .map(|feature| feature.name.clone())
            }),
            selected_feature_ids,
            visible_solid_count: snapshot.visible_solid_count,
            active_feature_count: snapshot.active_feature_count,
            spatial_entities: snapshot
                .spatial_entities
                .iter()
                .map(|entity| cadx_domain_api::DomainSpatialEntity {
                    feature_id: entity.feature_id,
                    name: entity.name.clone(),
                    minimum_mm: entity.bounds.min,
                    maximum_mm: entity.bounds.max,
                })
                .collect(),
        }
    }

    fn collect_dynamic_ai_context(
        &self,
        prompt: &str,
        scene_analysis: &SceneAnalysis,
    ) -> Result<ContextCollector, cadx_ai::context::ContextCollectionError> {
        let domain_schema = self
            .domain_bus
            .is_enabled(self.active_domain)
            .then(|| self.domain_bus.inspector_schema(self.active_domain))
            .flatten()
            .and_then(|schema| serde_json::to_value(schema).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let scene = self.session.scene();
        let focus_point_mm = self
            .selected_vertex
            .as_ref()
            .and_then(|reference| scene.vertex(reference))
            .map(|vertex| vertex.geometry.position)
            .or_else(|| {
                self.selected_edges
                    .first()
                    .and_then(|reference| scene.edge(reference))
                    .map(|edge| edge.geometry.midpoint)
            })
            .or_else(|| {
                self.selected_face
                    .as_ref()
                    .and_then(|reference| scene.face(reference))
                    .map(|face| face.geometry.centroid)
            })
            .or_else(|| {
                self.selected.and_then(|feature_id| {
                    scene_analysis
                        .parts
                        .iter()
                        .find(|part| part.feature_id == feature_id)
                        .map(|part| part.centroid_mm)
                })
            });
        ContextCollector::collect(ContextCollectionInput {
            domain: Some(self.active_domain),
            document_revision: self.session.revision(),
            prompt,
            document: self.session.document(),
            scene_analysis,
            selection: ContextSelection {
                selected_feature_id: self.selected,
                selected_face: self.selected_face.clone(),
                selected_edges: self.selected_edges.clone(),
                selected_vertex: self.selected_vertex.clone(),
                focus_point_mm,
                omitted_edge_count: 0,
            },
            viewport: ViewportContext {
                target_mm: self.camera.target.to_array().map(f64::from),
                camera_distance_mm: f64::from(self.camera.distance),
                yaw_degrees: f64::from(self.camera.yaw.to_degrees()),
                pitch_degrees: f64::from(self.camera.pitch.to_degrees()),
            },
            domain_schema,
            budget: ContextBudget::default(),
        })
    }

    pub(crate) fn receive_ai_plans(&mut self) {
        let translator = self.translator.clone();
        while let Ok(response) = self.ai_receiver.try_recv() {
            let Some(tracked_revision) = self.ai_tasks.finish(response.request_id) else {
                continue;
            };
            debug_assert_eq!(tracked_revision, response.base_revision);
            let base_revision = response.base_revision;
            let result = response.result;
            match result {
                Ok(crate::AiTaskResult::Cad(plan)) => {
                    if base_revision != self.session.revision() {
                        self.conversation.push(ConversationEntry {
                            speaker: Speaker::Error,
                            content: LocalizedText::Key("ai.context_stale"),
                        });
                        continue;
                    }
                    let capabilities = self.session.kernel_capabilities();
                    let mut candidates = Vec::new();
                    let mut rejections = Vec::new();
                    for candidate in plan.into_candidates() {
                        let metadata = TransactionMetadata::new(
                            TransactionSource::Ai,
                            candidate.summary.clone(),
                        );
                        let preview = match self
                            .session
                            .preview_with_metadata(&candidate.commands, metadata)
                        {
                            Ok(preview) => preview,
                            Err(error) => {
                                rejections.push(error.to_string());
                                continue;
                            }
                        };
                        let comparison =
                            match compare_scenes(self.session.scene(), preview.scene(), None) {
                                Ok(comparison) => comparison,
                                Err(error) => {
                                    self.session.discard_preview(
                                        preview,
                                        TransactionMetadata::new(
                                            TransactionSource::Ai,
                                            candidate.summary,
                                        ),
                                    );
                                    rejections.push(error.to_string());
                                    continue;
                                }
                            };
                        let interference = capabilities
                            .interference_analysis
                            .then(|| self.session.analyze_preview_interference(&preview))
                            .and_then(Result::ok);
                        candidates.push(PendingAiCandidate {
                            plan: candidate,
                            preview,
                            comparison,
                            interference,
                            review_items: Vec::new(),
                            domain_effects: None,
                        });
                    }
                    if candidates.is_empty() {
                        let error = rejections.join("; ");
                        self.conversation.push(ConversationEntry {
                            speaker: Speaker::Error,
                            content: LocalizedText::Text(
                                translator.format("ai.plan_rejected", &[("error", &error)]),
                            ),
                        });
                        continue;
                    }
                    self.discard_pending_ai_candidates();
                    self.active_ai_candidate = 0;
                    self.pending_ai_candidates = candidates;
                    self.sync_viewport();
                    let count = self.pending_ai_candidates.len().to_string();
                    let summary = self.pending_ai_candidates[0].plan.summary.clone();
                    self.conversation.push(ConversationEntry {
                        speaker: Speaker::Assistant,
                        content: LocalizedText::Text(format!(
                            "{summary}\n{}",
                            translator.format("ai.candidates_ready", &[("count", &count)])
                        )),
                    });
                    if !rejections.is_empty() {
                        let count = rejections.len().to_string();
                        self.conversation.push(ConversationEntry {
                            speaker: Speaker::Error,
                            content: LocalizedText::Text(
                                translator.format("ai.candidates_rejected", &[("count", &count)]),
                            ),
                        });
                    }
                }
                Ok(crate::AiTaskResult::Domain { plan, context }) => {
                    self.receive_domain_ai_plan(plan, context, base_revision, &translator);
                }
                Err(error) => self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(error.to_string()),
                }),
            }
        }
    }

    fn receive_domain_ai_plan(
        &mut self,
        plan: cadx_ai::DomainAiPlan,
        context: cadx_domain_api::DomainContext,
        base_revision: u64,
        translator: &cadx_i18n::Translator,
    ) {
        if base_revision != self.session.revision() {
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Error,
                content: LocalizedText::Key("ai.context_stale"),
            });
            return;
        }
        let Some(binding) = self
            .ai_tools
            .find_ai_tool(plan.domain, &plan.ai_tool_id)
            .cloned()
        else {
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Error,
                content: LocalizedText::Text(format!(
                    "AI tool {} is no longer registered for {}",
                    plan.ai_tool_id,
                    plan.domain.slug()
                )),
            });
            return;
        };
        if binding.executable_tool.id != plan.executable_tool_id {
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Error,
                content: LocalizedText::Text(format!(
                    "AI tool {} resolved to an inconsistent executable tool",
                    plan.ai_tool_id
                )),
            });
            return;
        }

        let request = cadx_domain_api::DomainToolRequest::new(plan.executable_tool_id, context)
            .with_parameters(plan.parameters);
        let execution = match self.domain_bus.execute(plan.domain, &request) {
            Ok(execution) => execution,
            Err(error) => {
                self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(error.to_string()),
                });
                return;
            }
        };
        let prepared = match self.prepare_domain_execution(plan.domain, execution) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(error),
                });
                return;
            }
        };
        let metadata =
            TransactionMetadata::new(TransactionSource::Ai, prepared.plan.summary.clone());
        let preview = match self
            .session
            .preview_with_metadata(&prepared.plan.commands, metadata)
        {
            Ok(preview) => preview,
            Err(error) => {
                self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(
                        translator.format("ai.plan_rejected", &[("error", &error.to_string())]),
                    ),
                });
                return;
            }
        };
        let comparison = match compare_scenes(self.session.scene(), preview.scene(), None) {
            Ok(comparison) => comparison,
            Err(error) => {
                self.session.discard_preview(
                    preview,
                    TransactionMetadata::new(TransactionSource::Ai, prepared.plan.summary.clone()),
                );
                self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(
                        translator.format("ai.plan_rejected", &[("error", &error.to_string())]),
                    ),
                });
                return;
            }
        };
        let interference = (!prepared.plan.commands.is_empty()
            && self.session.kernel_capabilities().interference_analysis)
            .then(|| self.session.analyze_preview_interference(&preview))
            .and_then(Result::ok);
        let summary = prepared.plan.summary.clone();
        let review_item = format!("{} / {}", plan.domain.slug(), binding.executable_tool.label);
        self.discard_pending_ai_candidates();
        self.active_ai_candidate = 0;
        self.pending_ai_candidates = vec![PendingAiCandidate {
            plan: prepared.plan,
            preview,
            comparison,
            interference,
            review_items: vec![review_item],
            domain_effects: Some(prepared.effects),
        }];
        self.sync_viewport();
        self.conversation.push(ConversationEntry {
            speaker: Speaker::Assistant,
            content: LocalizedText::Text(format!(
                "{summary}\n{}",
                translator.format("ai.candidates_ready", &[("count", "1")])
            )),
        });
    }

    pub(crate) fn ai_plan_is_pending(&self) -> bool {
        self.ai_tasks.is_pending()
    }

    pub(crate) fn cancel_ai_plan(&mut self) -> bool {
        if !self.ai_tasks.cancel() {
            return false;
        }
        self.conversation.push(ConversationEntry {
            speaker: Speaker::Assistant,
            content: LocalizedText::Key("ai.request_canceled"),
        });
        true
    }

    pub(crate) fn cancel_ai_plan_for_document_change(&mut self) -> bool {
        if !self
            .ai_tasks
            .cancel_if_revision_changed(self.session.revision())
        {
            return false;
        }
        self.conversation.push(ConversationEntry {
            speaker: Speaker::Assistant,
            content: LocalizedText::Key("ai.request_canceled_document_changed"),
        });
        true
    }

    pub(crate) fn approve_ai_plan(&mut self) {
        if self.pending_ai_candidates.is_empty() {
            return;
        }
        let index = self
            .active_ai_candidate
            .min(self.pending_ai_candidates.len() - 1);
        let candidate = self.pending_ai_candidates.remove(index);
        self.discard_pending_ai_candidates();
        let count = candidate.plan.commands.len().to_string();
        let summary = candidate.plan.summary;
        if let Err(error) = self.commit_preview_from(
            candidate.preview,
            StatusMessage::Text(summary.clone()),
            TransactionSource::Ai,
            summary.clone(),
        ) {
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Error,
                content: LocalizedText::Text(error.to_string()),
            });
            self.sync_viewport();
            return;
        }
        if let Some(effects) = candidate.domain_effects {
            self.apply_domain_effects(effects);
        }
        let applied = self
            .translator
            .format("ai.operations_applied", &[("count", &count)]);
        self.conversation.push(ConversationEntry {
            speaker: Speaker::Assistant,
            content: LocalizedText::Text(format!("{summary}\n{applied}")),
        });
    }

    pub(crate) fn reject_ai_plan(&mut self) {
        if self.discard_pending_ai_candidates() {
            self.sync_viewport();
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Assistant,
                content: LocalizedText::Key("ai.plan_discarded"),
            });
        }
    }

    pub(crate) fn discard_pending_ai_candidates(&mut self) -> bool {
        let candidates = std::mem::take(&mut self.pending_ai_candidates);
        let had_candidates = !candidates.is_empty();
        self.active_ai_candidate = 0;
        for candidate in candidates {
            self.session.discard_preview(
                candidate.preview,
                TransactionMetadata::new(TransactionSource::Ai, candidate.plan.summary),
            );
        }
        had_candidates
    }
}

#[cfg(test)]
mod tests {
    use std::future;

    use crate::AiTaskTracker;

    #[test]
    fn cancel_aborts_provider_task_and_rejects_its_late_response() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let task = runtime.spawn(future::pending::<()>());
        let mut tasks = AiTaskTracker::default();
        let request_id = tasks.reserve_request_id();
        tasks.track(request_id, 7, task.abort_handle());

        assert!(tasks.is_pending());
        assert!(tasks.cancel());
        assert!(!tasks.is_pending());
        assert!(runtime.block_on(task).unwrap_err().is_cancelled());
        assert_eq!(tasks.finish(request_id), None);
    }

    #[test]
    fn request_ids_isolate_old_responses_from_the_active_request() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let first_task = runtime.spawn(future::pending::<()>());
        let second_task = runtime.spawn(future::pending::<()>());
        let mut tasks = AiTaskTracker::default();
        let first = tasks.reserve_request_id();
        tasks.track(first, 10, first_task.abort_handle());
        assert!(tasks.cancel());
        let second = tasks.reserve_request_id();
        tasks.track(second, 11, second_task.abort_handle());

        assert_ne!(first, second);
        assert_eq!(tasks.finish(first), None);
        assert!(tasks.is_pending());
        assert_eq!(tasks.finish(second), Some(11));
        assert!(!tasks.is_pending());

        first_task.abort();
        second_task.abort();
        let _ = runtime.block_on(first_task);
        let _ = runtime.block_on(second_task);
    }

    #[test]
    fn tracking_a_replacement_defensively_aborts_the_previous_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let first_task = runtime.spawn(future::pending::<()>());
        let second_task = runtime.spawn(future::pending::<()>());
        let mut tasks = AiTaskTracker::default();
        let first = tasks.reserve_request_id();
        tasks.track(first, 30, first_task.abort_handle());
        let second = tasks.reserve_request_id();
        tasks.track(second, 30, second_task.abort_handle());

        assert!(runtime.block_on(first_task).unwrap_err().is_cancelled());
        assert_eq!(tasks.finish(first), None);
        assert_eq!(tasks.finish(second), Some(30));

        second_task.abort();
        let _ = runtime.block_on(second_task);
    }

    #[test]
    fn document_revision_change_cancels_only_stale_requests() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let task = runtime.spawn(future::pending::<()>());
        let mut tasks = AiTaskTracker::default();
        let request_id = tasks.reserve_request_id();
        tasks.track(request_id, 21, task.abort_handle());

        assert!(!tasks.cancel_if_revision_changed(21));
        assert!(tasks.is_pending());
        assert!(tasks.cancel_if_revision_changed(22));
        assert!(!tasks.is_pending());
        assert!(runtime.block_on(task).unwrap_err().is_cancelled());
    }
}
