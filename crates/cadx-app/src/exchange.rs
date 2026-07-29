use std::path::{Path, PathBuf};

use cadx_config::UiLanguage;
use cadx_core::{CadCommand, CheckResult, CheckStatus, ValidationReport};
use cadx_io::{
    DXF_EXTENSION, PDF_EXTENSION, PdfExportOptions, export_dxf, export_pdf, plan_dxf_import,
};

use crate::app::CadxApp;

impl CadxApp {
    pub(crate) fn import_dxf(&mut self) {
        let path = self.exchange_path.trim();
        if path.is_empty() {
            self.status = self
                .language
                .text(
                    "Enter a DXF path before importing.",
                    "导入前请输入 DXF 路径。",
                )
                .into();
            return;
        }
        let path = ensure_dxf_extension(path);
        let plan = match plan_dxf_import(self.workspace.document(), &path) {
            Ok(plan) => plan,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot import DXF: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法导入 DXF：{error}"),
                };
                return;
            }
        };
        if plan.transaction.commands.is_empty() {
            self.exchange_path = path;
            self.status = match self.language {
                UiLanguage::English => format!(
                    "DXF contains no supported model-space entities; {} skipped.",
                    plan.report.skipped_entities
                ),
                UiLanguage::SimplifiedChinese => format!(
                    "DXF 不包含受支持的模型空间实体；已跳过 {} 个。",
                    plan.report.skipped_entities
                ),
            };
            return;
        }

        let selected_entity = plan.transaction.commands.iter().rev().find_map(|command| {
            if let CadCommand::CreateEntity { entity } = command {
                Some(entity.id)
            } else {
                None
            }
        });
        let report = plan.report;
        let validation = ValidationReport {
            checks: vec![
                CheckResult {
                    name: "DXF resource limits".into(),
                    status: CheckStatus::Passed,
                    detail: "Input bytes, layers, entities, and vertices are within limits.".into(),
                },
                CheckResult {
                    name: "DXF atomic mapping".into(),
                    status: CheckStatus::Passed,
                    detail: format!(
                        "Mapped {} entities at scale {} from {}.",
                        report.imported_entities, report.scale_factor, report.source_units
                    ),
                },
                CheckResult {
                    name: "DXF unsupported content".into(),
                    status: if report.skipped_entities == 0 {
                        CheckStatus::Passed
                    } else {
                        CheckStatus::Warning
                    },
                    detail: format!("Skipped {} unsupported entities.", report.skipped_entities),
                },
            ],
        };
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            format!("Import {} DXF entities", report.imported_entities),
            plan.transaction,
            validation,
        ) {
            Ok(commit_id) => {
                self.exchange_path = path;
                self.selected_entity = selected_entity;
                self.comparison = None;
                self.constraint_diagnostics.clear();
                self.clear_remote_context_review();
                self.is_dirty = true;
                self.sync_layer_state();
                self.fit_view();
                self.status = match self.language {
                    UiLanguage::English => format!(
                        "DXF import #{commit_id}: {} entities, {} new layers, {} skipped.",
                        report.imported_entities, report.created_layers, report.skipped_entities
                    ),
                    UiLanguage::SimplifiedChinese => format!(
                        "DXF 导入 #{commit_id}：{} 个实体，{} 个新图层，跳过 {} 个。",
                        report.imported_entities, report.created_layers, report.skipped_entities
                    ),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot commit DXF import: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法提交 DXF 导入：{error}"),
                }
            }
        }
    }

    pub(crate) fn export_dxf(&mut self) {
        let path = self.exchange_path.trim();
        if path.is_empty() {
            self.status = self
                .language
                .text(
                    "Enter a DXF path before exporting.",
                    "导出前请输入 DXF 路径。",
                )
                .into();
            return;
        }
        let path = ensure_dxf_extension(path);
        match export_dxf(self.workspace.document(), &path) {
            Ok(report) => {
                self.exchange_path = report.path.display().to_string();
                let omitted = report.omitted_parameters
                    + report.omitted_constraints
                    + report.omitted_locked_layers;
                self.status = match self.language {
                    UiLanguage::English => format!(
                        "DXF export: {} entities, {} simplified, {} skipped, {} metadata omitted.",
                        report.exported_entities,
                        report.simplified_entities,
                        report.skipped_entities,
                        omitted
                    ),
                    UiLanguage::SimplifiedChinese => format!(
                        "DXF 导出：{} 个实体，简化 {} 个，跳过 {} 个，省略 {} 项元数据。",
                        report.exported_entities,
                        report.simplified_entities,
                        report.skipped_entities,
                        omitted
                    ),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot export DXF: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法导出 DXF：{error}"),
                }
            }
        }
    }

    pub(crate) fn export_pdf_drawing(&mut self) {
        let path = self.pdf_path.trim();
        if path.is_empty() {
            self.status = self
                .language
                .text(
                    "Enter a PDF path before exporting.",
                    "导出前请输入 PDF 路径。",
                )
                .into();
            return;
        }
        let path = ensure_pdf_extension(path);
        let options = PdfExportOptions {
            page_size: self.pdf_page_size,
            orientation: self.pdf_orientation,
            margin_mm: self.pdf_margin_mm,
            ..Default::default()
        };
        match export_pdf(self.workspace.document(), &path, options) {
            Ok(report) => {
                self.pdf_path = report.path.display().to_string();
                let omitted = report.omitted_parameters
                    + report.omitted_constraints
                    + report.omitted_locked_layers;
                self.status = match self.language {
                    UiLanguage::English => format!(
                        "PDF export: {} entities, {} simplified, {} skipped, {} metadata omitted.",
                        report.exported_entities,
                        report.simplified_entities,
                        report.skipped_entities,
                        omitted
                    ),
                    UiLanguage::SimplifiedChinese => format!(
                        "PDF 导出：{} 个实体，简化 {} 个，跳过 {} 个，省略 {} 项元数据。",
                        report.exported_entities,
                        report.simplified_entities,
                        report.skipped_entities,
                        omitted
                    ),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot export PDF: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法导出 PDF：{error}"),
                }
            }
        }
    }
}

pub(crate) fn default_exchange_path(project_path: &str) -> String {
    let mut path = PathBuf::from(project_path);
    if path.file_name().is_none() {
        return format!("Drawing.{DXF_EXTENSION}");
    }
    path.set_extension(DXF_EXTENSION);
    path.display().to_string()
}

pub(crate) fn default_pdf_path(project_path: &str) -> String {
    let mut path = PathBuf::from(project_path);
    if path.file_name().is_none() {
        return format!("Drawing.{PDF_EXTENSION}");
    }
    path.set_extension(PDF_EXTENSION);
    path.display().to_string()
}

fn ensure_dxf_extension(path: &str) -> String {
    if Path::new(path).extension().is_none() {
        format!("{path}.{DXF_EXTENSION}")
    } else {
        path.into()
    }
}

fn ensure_pdf_extension(path: &str) -> String {
    if Path::new(path).extension().is_none() {
        format!("{path}.{PDF_EXTENSION}")
    } else {
        path.into()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use cadx_core::{
        CadCommand, CadDocument, CommandTransaction, Entity, EntityKind, Point2, ValidationReport,
    };
    use cadx_io::{PDF_EXTENSION, export_dxf};

    use super::*;

    fn test_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cadx-app-{label}-{}-{nonce}.{DXF_EXTENSION}",
            std::process::id()
        ))
    }

    fn test_pdf_path(label: &str) -> PathBuf {
        test_path(label).with_extension(PDF_EXTENSION)
    }

    fn source_document() -> CadDocument {
        let mut document = CadDocument::new("DXF source");
        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: Entity {
                id: 1,
                layer: 1,
                name: "Source line".into(),
                visible: true,
                kind: EntityKind::Line {
                    start: Point2::new(0.0, 0.0),
                    end: Point2::new(25.0, 10.0),
                },
                parameter_refs: BTreeSet::new(),
            },
        }])
        .apply(&mut document)
        .unwrap();
        document
    }

    #[test]
    fn workbench_dxf_import_is_one_semantic_commit() {
        let path = test_path("import");
        export_dxf(&source_document(), &path).unwrap();
        let mut app = CadxApp {
            exchange_path: path.display().to_string(),
            ..Default::default()
        };
        let before = app.workspace.history().commits.len();

        app.import_dxf();

        assert_eq!(app.workspace.history().commits.len(), before + 1);
        assert_eq!(app.workspace.document().entities.len(), 1);
        assert_eq!(app.selected_entity, Some(1));
        assert!(app.is_dirty);
        let commit = &app.workspace.history().commits[&app.workspace.history().head()];
        assert_eq!(commit.intent, "Import 1 DXF entities");
        assert_eq!(commit.validation.checks.len(), 3);
        app.workspace.validate_integrity().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn workbench_dxf_export_does_not_change_history_or_dirty_state() {
        let path = test_path("export");
        let mut app = CadxApp {
            exchange_path: path.display().to_string(),
            ..Default::default()
        };
        let expected_revision = app.workspace.revision();
        app.workspace
            .kernel()
            .apply_user_transaction(
                expected_revision,
                "Create source line",
                CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: Entity {
                        id: 1,
                        layer: 1,
                        name: "Line".into(),
                        visible: true,
                        kind: EntityKind::Line {
                            start: Point2::new(0.0, 0.0),
                            end: Point2::new(10.0, 0.0),
                        },
                        parameter_refs: BTreeSet::new(),
                    },
                }]),
                ValidationReport::default(),
            )
            .unwrap();
        app.is_dirty = false;
        let head = app.workspace.history().head();

        app.export_dxf();

        assert_eq!(app.workspace.history().head(), head);
        assert!(!app.is_dirty);
        assert!(path.is_file());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn workbench_dxf_import_does_not_create_an_empty_commit() {
        let path = test_path("empty-import");
        export_dxf(&CadDocument::new("Empty source"), &path).unwrap();
        let mut app = CadxApp {
            exchange_path: path.display().to_string(),
            ..Default::default()
        };
        let head = app.workspace.history().head();
        let commit_count = app.workspace.history().commits.len();

        app.import_dxf();

        assert_eq!(app.workspace.history().head(), head);
        assert_eq!(app.workspace.history().commits.len(), commit_count);
        assert!(app.status.contains("no supported model-space entities"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn workbench_pdf_export_does_not_change_history_or_dirty_state() {
        let path = test_pdf_path("pdf-export");
        let mut app = CadxApp {
            workspace: cadx_core::TaskWorkspace::new(source_document()),
            pdf_path: path.display().to_string(),
            pdf_page_size: cadx_io::PdfPageSize::Letter,
            pdf_orientation: cadx_io::PdfOrientation::Portrait,
            pdf_margin_mm: 15.0,
            ..Default::default()
        };
        app.is_dirty = false;
        let head = app.workspace.history().head();

        app.export_pdf_drawing();

        assert_eq!(app.workspace.history().head(), head);
        assert!(!app.is_dirty);
        assert_eq!(app.pdf_path, path.display().to_string());
        assert!(fs::read(&path).unwrap().starts_with(b"%PDF-"));
        assert!(app.status.starts_with("PDF export:"));
        fs::remove_file(path).unwrap();
    }
}
