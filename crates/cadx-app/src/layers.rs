use cadx_config::UiLanguage;
use cadx_core::{CadCommand, CommandTransaction, Layer, LayerId, ValidationReport};
use eframe::egui::{self, Color32};

use crate::app::CadxApp;

impl CadxApp {
    pub(crate) fn sync_layer_state(&mut self) {
        if !self
            .workspace
            .document()
            .layers
            .contains_key(&self.active_layer)
        {
            self.active_layer = self.preferred_editable_layer();
        }
        if self.layer_edit_id != Some(self.active_layer) {
            self.load_active_layer_editor();
        } else if let Some(layer) = self.workspace.document().layers.get(&self.active_layer) {
            self.layer_name_edit = layer.name.clone();
            self.layer_color_edit = layer.color;
        }
        self.pending_layer_delete = self
            .pending_layer_delete
            .filter(|id| self.workspace.document().layers.contains_key(id));
        self.delete_target_layer = self.delete_target_layer.filter(|id| {
            self.workspace
                .document()
                .layers
                .get(id)
                .is_some_and(|layer| !layer.locked)
        });
    }

    pub(crate) fn ui_layers(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let layers = self
            .workspace
            .document()
            .layers
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for layer in layers {
            ui.horizontal(|ui| {
                if ui
                    .radio(self.active_layer == layer.id, "")
                    .on_hover_text(language.text("Set active layer", "设为活动图层"))
                    .clicked()
                {
                    self.select_active_layer(layer.id);
                }
                let mut visible = layer.visible;
                if ui
                    .checkbox(&mut visible, "V")
                    .on_hover_text(language.text("Layer visibility", "图层可见性"))
                    .changed()
                {
                    self.set_layer_visibility(layer.id, visible);
                }
                let mut locked = layer.locked;
                if ui
                    .checkbox(&mut locked, "L")
                    .on_hover_text(language.text("Layer lock", "图层锁定"))
                    .changed()
                {
                    self.set_layer_lock(layer.id, locked);
                }
                ui.add(
                    egui::Button::new("")
                        .min_size(egui::vec2(16.0, 16.0))
                        .fill(color32(layer.color)),
                )
                .on_hover_text(language.text("Layer color", "图层颜色"));
                let entity_count = self
                    .workspace
                    .document()
                    .entities
                    .values()
                    .filter(|entity| entity.layer == layer.id)
                    .count();
                ui.label(format!("{} ({entity_count})", layer.name));
            });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [148.0, 22.0],
                egui::TextEdit::singleline(&mut self.new_layer_name)
                    .hint_text(language.text("New layer", "新建图层")),
            );
            let mut color = color32(self.new_layer_color);
            if ui.color_edit_button_srgba(&mut color).changed() {
                self.new_layer_color = color_array(color);
            }
            if ui
                .button("+")
                .on_hover_text(language.text("Create layer", "创建图层"))
                .clicked()
            {
                self.create_layer();
            }
        });

        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [148.0, 22.0],
                egui::TextEdit::singleline(&mut self.layer_name_edit),
            );
            let mut color = color32(self.layer_color_edit);
            if ui.color_edit_button_srgba(&mut color).changed() {
                self.layer_color_edit = color_array(color);
            }
            if ui.button(language.text("Save", "保存")).clicked() {
                self.save_active_layer_properties();
            }
            let can_delete = self.workspace.document().layers.len() > 1
                && self
                    .workspace
                    .document()
                    .layers
                    .values()
                    .any(|layer| layer.id != self.active_layer && !layer.locked);
            if ui
                .add_enabled(
                    can_delete,
                    egui::Button::new(language.text("Delete", "删除")),
                )
                .on_hover_text(language.text(
                    "Delete layer and reassign its entities",
                    "删除图层并重新分配其中的实体",
                ))
                .clicked()
            {
                self.request_active_layer_delete();
            }
        });
    }

    pub(crate) fn ui_entity_layer_picker(&mut self, ui: &mut egui::Ui, entity_id: u64) {
        let Some(entity) = self.workspace.document().entities.get(&entity_id) else {
            return;
        };
        let current_layer = entity.layer;
        let current_name = self
            .workspace
            .document()
            .layers
            .get(&current_layer)
            .map(|layer| layer.name.clone())
            .unwrap_or_else(|| format!("{} {current_layer}", self.language.text("Layer", "图层")));
        let layers = self
            .workspace
            .document()
            .layers
            .values()
            .filter(|layer| !layer.locked)
            .map(|layer| (layer.id, layer.name.clone()))
            .collect::<Vec<_>>();
        let source_locked = self
            .workspace
            .document()
            .layers
            .get(&current_layer)
            .is_some_and(|layer| layer.locked);

        ui.add_enabled_ui(!source_locked, |ui| {
            egui::ComboBox::from_id_salt("entity_layer")
                .selected_text(current_name)
                .show_ui(ui, |ui| {
                    for (layer_id, name) in layers {
                        if ui
                            .selectable_label(layer_id == current_layer, name)
                            .clicked()
                            && layer_id != current_layer
                        {
                            self.move_entity_to_layer(entity_id, layer_id);
                        }
                    }
                });
        });
    }

    pub(crate) fn ui_layer_delete_dialog(&mut self, context: &egui::Context) {
        let Some(layer_id) = self.pending_layer_delete else {
            return;
        };
        let Some(layer) = self.workspace.document().layers.get(&layer_id).cloned() else {
            self.pending_layer_delete = None;
            return;
        };
        let entity_count = self
            .workspace
            .document()
            .entities
            .values()
            .filter(|entity| entity.layer == layer_id)
            .count();
        let targets = self
            .workspace
            .document()
            .layers
            .values()
            .filter(|target| target.id != layer_id && !target.locked)
            .map(|target| (target.id, target.name.clone()))
            .collect::<Vec<_>>();
        let language = self.language;
        let target_name = self
            .delete_target_layer
            .and_then(|id| self.workspace.document().layers.get(&id))
            .map(|target| target.name.clone())
            .unwrap_or_else(|| language.text("Destination", "目标图层").into());
        let mut close = false;

        egui::Window::new(language.text("Delete Layer", "删除图层"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(context, |ui| {
                ui.label(egui::RichText::new(&layer.name).strong());
                ui.label(match language {
                    UiLanguage::English => {
                        format!("{entity_count} entities will be reassigned.")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("将重新分配 {entity_count} 个实体。")
                    }
                });
                egui::ComboBox::from_id_salt("delete_layer_target")
                    .selected_text(target_name)
                    .show_ui(ui, |ui| {
                        for (target_id, name) in &targets {
                            ui.selectable_value(
                                &mut self.delete_target_layer,
                                Some(*target_id),
                                name,
                            );
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.delete_target_layer.is_some(),
                            egui::Button::new(language.text("Delete", "删除")),
                        )
                        .clicked()
                    {
                        self.confirm_layer_delete(layer_id);
                        close = true;
                    }
                    if ui.button(language.text("Cancel", "取消")).clicked() {
                        close = true;
                    }
                });
            });
        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close {
            self.pending_layer_delete = None;
            self.delete_target_layer = None;
        }
    }

    fn preferred_editable_layer(&self) -> LayerId {
        self.workspace
            .document()
            .layers
            .values()
            .find(|layer| layer.visible && !layer.locked)
            .or_else(|| {
                self.workspace
                    .document()
                    .layers
                    .values()
                    .find(|layer| !layer.locked)
            })
            .or_else(|| self.workspace.document().layers.values().next())
            .map_or(1, |layer| layer.id)
    }

    fn select_active_layer(&mut self, layer_id: LayerId) {
        if self.workspace.document().layers.contains_key(&layer_id) {
            self.active_layer = layer_id;
            self.load_active_layer_editor();
        }
    }

    fn load_active_layer_editor(&mut self) {
        if let Some(layer) = self.workspace.document().layers.get(&self.active_layer) {
            self.layer_edit_id = Some(layer.id);
            self.layer_name_edit = layer.name.clone();
            self.layer_color_edit = layer.color;
        }
    }

    fn create_layer(&mut self) {
        let name = self.new_layer_name.trim();
        if name.is_empty() {
            self.status = self
                .language
                .text("Layer name is required.", "必须输入图层名称。")
                .into();
            return;
        }
        let layer = Layer {
            id: self.workspace.document().next_layer_id(),
            name: name.into(),
            visible: true,
            locked: false,
            color: self.new_layer_color,
        };
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            self.language.text("Create drawing layer", "创建制图图层"),
            CommandTransaction::new(vec![CadCommand::CreateLayer {
                layer: layer.clone(),
            }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.active_layer = layer.id;
                self.new_layer_name.clear();
                self.load_active_layer_editor();
                self.mark_layer_change(match self.language {
                    UiLanguage::English => format!(
                        "Created layer {} in semantic commit #{commit_id}",
                        layer.name
                    ),
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中创建图层 {}", layer.name)
                    }
                });
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot create layer: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法创建图层：{error}"),
                }
            }
        }
    }

    fn save_active_layer_properties(&mut self) {
        let Some(existing) = self
            .workspace
            .document()
            .layers
            .get(&self.active_layer)
            .cloned()
        else {
            self.sync_layer_state();
            return;
        };
        let name = self.layer_name_edit.trim();
        if name.is_empty() {
            self.status = self
                .language
                .text("Layer name is required.", "必须输入图层名称。")
                .into();
            return;
        }
        let layer = Layer {
            name: name.into(),
            color: self.layer_color_edit,
            ..existing
        };
        if self.workspace.document().layers[&layer.id] == layer {
            self.status = self
                .language
                .text("Layer properties are unchanged.", "图层属性没有变化。")
                .into();
            return;
        }
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            self.language
                .text("Update drawing layer properties", "更新制图图层属性"),
            CommandTransaction::new(vec![CadCommand::UpdateLayer {
                layer: layer.clone(),
            }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.load_active_layer_editor();
                self.mark_layer_change(match self.language {
                    UiLanguage::English => format!(
                        "Updated layer {} in semantic commit #{commit_id}",
                        layer.name
                    ),
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中更新图层 {}", layer.name)
                    }
                });
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot update layer: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法更新图层：{error}"),
                }
            }
        }
    }

    fn set_layer_visibility(&mut self, layer_id: LayerId, visible: bool) {
        let Some(mut layer) = self.workspace.document().layers.get(&layer_id).cloned() else {
            return;
        };
        layer.visible = visible;
        self.update_layer_state(
            layer,
            self.language
                .text("Change layer visibility", "更改图层可见性"),
        );
    }

    fn set_layer_lock(&mut self, layer_id: LayerId, locked: bool) {
        let Some(mut layer) = self.workspace.document().layers.get(&layer_id).cloned() else {
            return;
        };
        layer.locked = locked;
        let name = layer.name.clone();
        if self.update_layer_state(
            layer,
            self.language.text("Change layer lock", "更改图层锁定"),
        ) && locked
        {
            self.selected_entity = self.selected_entity.filter(|entity_id| {
                self.workspace
                    .document()
                    .entities
                    .get(entity_id)
                    .is_none_or(|entity| entity.layer != layer_id)
            });
            self.status = match self.language {
                UiLanguage::English => format!("Locked layer {name}"),
                UiLanguage::SimplifiedChinese => format!("已锁定图层 {name}"),
            };
        }
    }

    fn update_layer_state(&mut self, layer: Layer, intent: &str) -> bool {
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            intent,
            CommandTransaction::new(vec![CadCommand::UpdateLayer {
                layer: layer.clone(),
            }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                if layer.id == self.active_layer {
                    self.load_active_layer_editor();
                }
                self.mark_layer_change(match self.language {
                    UiLanguage::English => format!(
                        "Updated layer {} in semantic commit #{commit_id}",
                        layer.name
                    ),
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中更新图层 {}", layer.name)
                    }
                });
                true
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot update layer: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法更新图层：{error}"),
                };
                false
            }
        }
    }

    fn move_entity_to_layer(&mut self, entity_id: u64, layer_id: LayerId) {
        let Some(mut entity) = self.workspace.document().entities.get(&entity_id).cloned() else {
            return;
        };
        let name = entity.name.clone();
        entity.layer = layer_id;
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            self.language
                .text("Move entity to drawing layer", "移动实体到制图图层"),
            CommandTransaction::new(vec![CadCommand::UpdateEntity { entity }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => self.mark_layer_change(match self.language {
                UiLanguage::English => {
                    format!("Moved {name} in semantic commit #{commit_id}")
                }
                UiLanguage::SimplifiedChinese => {
                    format!("已在语义提交 #{commit_id} 中移动 {name}")
                }
            }),
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot move {name}: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法移动 {name}：{error}"),
                }
            }
        }
    }

    fn request_active_layer_delete(&mut self) {
        let layer_id = self.active_layer;
        self.delete_target_layer = self
            .workspace
            .document()
            .layers
            .values()
            .find(|layer| layer.id != layer_id && layer.visible && !layer.locked)
            .or_else(|| {
                self.workspace
                    .document()
                    .layers
                    .values()
                    .find(|layer| layer.id != layer_id && !layer.locked)
            })
            .map(|layer| layer.id);
        self.pending_layer_delete = Some(layer_id);
    }

    fn confirm_layer_delete(&mut self, layer_id: LayerId) {
        let Some(target_id) = self.delete_target_layer else {
            return;
        };
        let Some(source) = self.workspace.document().layers.get(&layer_id).cloned() else {
            return;
        };
        let Some(target) = self.workspace.document().layers.get(&target_id) else {
            return;
        };
        if target.locked || target_id == layer_id {
            self.status = self
                .language
                .text(
                    "Select an unlocked destination layer.",
                    "请选择未锁定的目标图层。",
                )
                .into();
            return;
        }

        let mut commands = Vec::new();
        if source.locked {
            commands.push(CadCommand::UpdateLayer {
                layer: Layer {
                    locked: false,
                    ..source.clone()
                },
            });
        }
        commands.extend(
            self.workspace
                .document()
                .entities
                .values()
                .filter(|entity| entity.layer == layer_id)
                .cloned()
                .map(|mut entity| {
                    entity.layer = target_id;
                    CadCommand::UpdateEntity { entity }
                }),
        );
        commands.push(CadCommand::DeleteLayer { id: layer_id });
        let name = source.name;
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            self.language.text(
                "Delete drawing layer and reassign entities",
                "删除制图图层并重新分配实体",
            ),
            CommandTransaction::new(commands),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.active_layer = target_id;
                self.load_active_layer_editor();
                self.mark_layer_change(match self.language {
                    UiLanguage::English => {
                        format!("Deleted layer {name} in semantic commit #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中删除图层 {name}")
                    }
                });
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot delete layer {name}: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法删除图层 {name}：{error}")
                    }
                }
            }
        }
    }

    fn mark_layer_change(&mut self, status: String) {
        self.comparison = None;
        self.constraint_diagnostics.clear();
        self.clear_remote_context_review();
        self.is_dirty = true;
        self.status = status;
    }
}

fn color32(color: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3])
}

fn color_array(color: Color32) -> [u8; 4] {
    [color.r(), color.g(), color.b(), color.a()]
}

#[cfg(test)]
mod tests {
    use cadx_core::Point2;

    use super::*;
    use crate::viewport::ViewportTool;

    #[test]
    fn active_layer_drives_drawing_and_locked_layers_reject_authoring() {
        let mut app = CadxApp {
            new_layer_name: "Dimensions".into(),
            viewport_tool: ViewportTool::Line,
            ..Default::default()
        };
        app.create_layer();
        let dimensions = app.active_layer;

        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        assert_eq!(app.workspace.document().entities[&1].layer, dimensions);

        app.set_layer_lock(dimensions, true);
        app.commit_draw_gesture(Point2::new(0.0, 2.0), Point2::new(10.0, 2.0));
        assert_eq!(app.workspace.document().entities.len(), 1);
        assert!(app.status.contains("Unlock layer"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn deleting_a_layer_reassigns_entities_atomically_and_is_undoable() {
        let mut app = CadxApp {
            new_layer_name: "Temporary".into(),
            viewport_tool: ViewportTool::Rectangle,
            ..Default::default()
        };
        app.create_layer();
        let temporary = app.active_layer;
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(10.0, 8.0));
        app.delete_target_layer = Some(1);

        app.confirm_layer_delete(temporary);

        assert!(!app.workspace.document().layers.contains_key(&temporary));
        assert_eq!(app.workspace.document().entities[&1].layer, 1);
        app.workspace.kernel().undo().unwrap();
        assert!(app.workspace.document().layers.contains_key(&temporary));
        assert_eq!(app.workspace.document().entities[&1].layer, temporary);
        app.workspace.validate_integrity().unwrap();
    }
}
