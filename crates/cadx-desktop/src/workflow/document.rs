use cadx_app::{DocumentState, TransactionSource, plan_step_import};
use cadx_core::domain::CadDocument;
use cadx_io::{
    DOCUMENT_EXTENSION, load_document, read_step, save_document, write_3mf, write_binary_stl,
    write_step,
};

use crate::{CadxApp, StatusMessage};

impl CadxApp {
    pub(crate) fn new_document(&mut self) {
        if !self.confirm_discard_changes() {
            return;
        }
        if let Err(error) = self.session.replace_document(CadDocument::default()) {
            self.status = StatusMessage::Text(error.to_string());
            return;
        }
        self.document_path = None;
        self.selected = None;
        self.clear_topology_selection();
        self.clear_measurement();
        self.pending_ai_plan = None;
        self.loft_dialog = None;
        self.boolean_dialog = None;
        self.edge_modifier_dialog = None;
        self.interference_dialog = None;
        self.last_boolean_failure = None;
        self.last_edge_modifier_failure = None;
        self.last_sketch_failure = None;
        self.last_sketch_failure_feature = None;
        self.sketch_dimension_editor = None;
        self.sync_domain_state();
        self.status = StatusMessage::Key("status.new_document");
        self.sync_viewport();
    }

    pub(crate) fn open_document(&mut self) {
        if !self.confirm_discard_changes() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CADX document", &[DOCUMENT_EXTENSION])
            .pick_file()
        else {
            return;
        };
        match load_document(path.clone()) {
            Ok(document) => {
                if let Err(error) = self.session.replace_document(document) {
                    self.status = StatusMessage::Text(error.to_string());
                    return;
                }
                self.document_path = Some(path);
                self.selected = self
                    .session
                    .document()
                    .features
                    .last()
                    .map(|feature| feature.id);
                self.pending_ai_plan = None;
                self.loft_dialog = None;
                self.boolean_dialog = None;
                self.edge_modifier_dialog = None;
                self.interference_dialog = None;
                self.last_boolean_failure = None;
                self.last_edge_modifier_failure = None;
                self.last_sketch_failure = None;
                self.last_sketch_failure_feature = None;
                self.sketch_dimension_editor = None;
                self.sync_domain_state();
                self.clear_topology_selection();
                self.clear_measurement();
                self.status = StatusMessage::Key("status.opened");
                self.sync_viewport();
            }
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    pub(crate) fn save_document(&mut self, save_as: bool) {
        let path = if save_as {
            None
        } else {
            self.document_path.clone()
        }
        .or_else(|| {
            rfd::FileDialog::new()
                .add_filter("CADX document", &[DOCUMENT_EXTENSION])
                .set_file_name("model.cadx")
                .save_file()
        });
        let Some(mut path) = path else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension(DOCUMENT_EXTENSION);
        }
        match save_document(self.session.document(), path.clone()) {
            Ok(()) => {
                self.document_path = Some(path);
                self.session.mark_saved();
                self.status = StatusMessage::Key("status.saved");
            }
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    pub(crate) fn import_step(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("STEP model", &["step", "stp"])
            .pick_file()
        else {
            return;
        };
        let result = read_step(path.clone()).and_then(|import| {
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Imported STEP");
            let plan = plan_step_import(self.session.document(), import, stem)
                .map_err(|error| cadx_io::ExportError::InvalidStep(error.to_string()))?;
            self.execute_from(
                plan.commands,
                StatusMessage::Key("status.imported_step"),
                TransactionSource::Import,
            )
            .map_err(|error| cadx_io::ExportError::InvalidStep(error.to_string()))?;
            if plan.unsupported_color_count > 0 {
                let count = plan.unsupported_color_count.to_string();
                self.status = StatusMessage::Text(
                    self.translator
                        .format("status.imported_step_color_warning", &[("count", &count)]),
                );
            }
            Ok(())
        });
        if let Err(error) = result {
            self.status = StatusMessage::Text(error.to_string());
        }
    }

    pub(crate) fn export_stl(&mut self) {
        let default_name = self
            .document_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .map_or_else(|| "model.stl".into(), |stem| format!("{stem}.stl"));
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("STL mesh", &["stl"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("stl");
        }
        match write_binary_stl(self.session.scene(), &path) {
            Ok(()) => self.status = StatusMessage::Key("status.exported_stl"),
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    pub(crate) fn export_3mf(&mut self) {
        let default_name = self
            .document_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .map_or_else(|| "model.3mf".into(), |stem| format!("{stem}.3mf"));
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("3MF model", &["3mf"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("3mf");
        }
        match write_3mf(self.session.scene(), &path) {
            Ok(()) => self.status = StatusMessage::Key("status.exported_3mf"),
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    pub(crate) fn export_step(&mut self) {
        let default_name = self
            .document_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .map_or_else(|| "model.step".into(), |stem| format!("{stem}.step"));
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("STEP model", &["step", "stp"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("step");
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model.step");
        let result = self
            .exchange_kernel
            .encode_step(self.session.document(), file_name)
            .map_err(|error| error.to_string())
            .and_then(|source| write_step(&source, &path).map_err(|error| error.to_string()));
        match result {
            Ok(()) => self.status = StatusMessage::Key("status.exported_step"),
            Err(error) => self.status = StatusMessage::Text(error),
        }
    }

    fn confirm_discard_changes(&self) -> bool {
        if self.session.state() == DocumentState::Clean {
            return true;
        }
        let result = rfd::MessageDialog::new()
            .set_title(self.translator.text("dialog.unsaved_title"))
            .set_description(self.translator.text("dialog.unsaved_description"))
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        result == rfd::MessageDialogResult::Yes
    }
}
