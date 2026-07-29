use eframe::egui::{self, Color32};

use crate::app::CadxApp;
use crate::localization::unit_label;

impl CadxApp {
    pub(crate) fn ui_status_bar(&mut self, context: &egui::Context) {
        let language = self.language;
        egui::TopBottomPanel::bottom("status")
            .exact_height(30.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&self.status)
                            .small()
                            .color(Color32::LIGHT_GRAY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}: {}",
                                language.text("Units", "单位"),
                                unit_label(self.workspace.document().units)
                            ))
                            .small()
                            .color(Color32::GRAY),
                        );
                        ui.separator();
                        ui.label(egui::RichText::new("GPU").small().color(Color32::GRAY))
                            .on_hover_text(&self.gpu_adapter);
                        ui.separator();
                        let active_layer = self
                            .workspace
                            .document()
                            .layers
                            .get(&self.active_layer)
                            .map_or("-", |layer| layer.name.as_str());
                        ui.label(
                            egui::RichText::new(format!(
                                "{}: {active_layer}",
                                language.text("Layer", "图层")
                            ))
                            .small()
                            .color(Color32::GRAY),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new(if self.is_dirty {
                                language.text("Unsaved project", "工程未保存")
                            } else {
                                language.text("Native project saved", "原生工程已保存")
                            })
                            .small()
                            .color(if self.is_dirty {
                                Color32::from_rgb(225, 185, 71)
                            } else {
                                Color32::from_rgb(111, 220, 196)
                            }),
                        );
                        if let Some((label, color, detail)) = self.recovery.presentation(language) {
                            ui.separator();
                            let response =
                                ui.label(egui::RichText::new(label).small().color(color));
                            if let Some(detail) = detail {
                                response.on_hover_text(detail);
                            }
                        }
                    });
                });
            });
    }
}
