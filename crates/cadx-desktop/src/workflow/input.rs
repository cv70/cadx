use cadx_core::domain::ModelCommand;
use eframe::egui;

use crate::{CadxApp, StatusMessage};

impl CadxApp {
    pub(crate) fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        let (command, shift) = context.input(|input| {
            (
                input.modifiers.command || input.modifiers.ctrl,
                input.modifiers.shift,
            )
        });
        if context.input(|input| command && input.key_pressed(egui::Key::Z)) {
            if shift {
                self.redo();
            } else {
                self.undo();
            }
        } else if context.input(|input| command && input.key_pressed(egui::Key::Y)) {
            self.redo();
        } else if context.input(|input| command && shift && input.key_pressed(egui::Key::E)) {
            self.export_stl();
        } else if context.input(|input| command && input.key_pressed(egui::Key::S)) {
            self.save_document(false);
        } else if context.input(|input| command && input.key_pressed(egui::Key::O)) {
            self.open_document();
        } else if context.input(|input| command && input.key_pressed(egui::Key::N)) {
            self.new_document();
        } else if !context.egui_wants_keyboard_input()
            && context.input(|input| command && input.key_pressed(egui::Key::D))
            && let Some(id) = self.selected
        {
            self.duplicate_feature(id);
        } else if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            if self.sketch_dimension_editor.take().is_some() {
                return;
            }
            self.selected = None;
            self.clear_topology_selection();
            self.clear_measurement();
            self.sync_viewport();
        } else if !context.egui_wants_keyboard_input()
            && context.input(|input| input.key_pressed(egui::Key::Delete))
            && let Some(id) = self.selected
        {
            let _ = self.execute(
                vec![ModelCommand::Delete { id }],
                StatusMessage::Key("status.deleted"),
            );
        }
    }
}
