use std::sync::Arc;

use cadx_ai::{AiContext, AiRequest, AiSketchDimension};
use cadx_analysis::analyze_scene;
use eframe::egui;

use crate::{CadxApp, ConversationEntry, LocalizedText, Speaker, StatusMessage};

impl CadxApp {
    pub(crate) fn request_ai_plan(&mut self, context: &egui::Context) {
        let prompt = self.ai_input.trim().to_owned();
        if prompt.is_empty() || self.ai_pending {
            return;
        }
        self.ai_input.clear();
        self.ai_pending = true;
        self.conversation.push(ConversationEntry {
            speaker: Speaker::User,
            content: LocalizedText::Text(prompt.clone()),
        });
        let measurement = self
            .measurement
            .active
            .then(|| self.measurement.result(self.session.scene()))
            .and_then(Result::ok);
        let request = AiRequest {
            prompt,
            document: self.session.document().clone(),
            context: analyze_scene(self.session.scene(), None)
                .ok()
                .map(|scene_analysis| AiContext {
                    kernel_capabilities: self.session.kernel_capabilities(),
                    selected_feature_id: self.selected,
                    selected_face: self.selected_face.clone(),
                    selected_edges: self.selected_edges.clone(),
                    selected_vertex: self.selected_vertex.clone(),
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
                    scene_analysis,
                }),
        };
        let assistant = Arc::clone(&self.assistant);
        let sender = self.ai_sender.clone();
        let repaint = context.clone();
        self.runtime.spawn(async move {
            let result = assistant.plan(request).await;
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }

    pub(crate) fn receive_ai_plans(&mut self) {
        let translator = self.translator.clone();
        while let Ok(result) = self.ai_receiver.try_recv() {
            self.ai_pending = false;
            match result {
                Ok(plan) => {
                    let summary = plan.summary.clone();
                    match self.session.validate(&plan.commands) {
                        Ok(()) => {
                            self.pending_ai_plan = Some(plan);
                            self.conversation.push(ConversationEntry {
                                speaker: Speaker::Assistant,
                                content: LocalizedText::Text(format!(
                                    "{summary}\n{}",
                                    translator.text("ai.plan_ready")
                                )),
                            });
                        }
                        Err(error) => self.conversation.push(ConversationEntry {
                            speaker: Speaker::Error,
                            content: LocalizedText::Text(
                                translator
                                    .format("ai.plan_rejected", &[("error", &error.to_string())]),
                            ),
                        }),
                    }
                }
                Err(error) => self.conversation.push(ConversationEntry {
                    speaker: Speaker::Error,
                    content: LocalizedText::Text(error.to_string()),
                }),
            }
        }
    }

    pub(crate) fn approve_ai_plan(&mut self) {
        let Some(plan) = self.pending_ai_plan.take() else {
            return;
        };
        let count = plan.commands.len().to_string();
        let summary = plan.summary;
        if let Err(error) = self.execute(plan.commands, StatusMessage::Text(summary.clone())) {
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Error,
                content: LocalizedText::Text(error.to_string()),
            });
            return;
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
        if self.pending_ai_plan.take().is_some() {
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Assistant,
                content: LocalizedText::Key("ai.plan_discarded"),
            });
        }
    }
}
