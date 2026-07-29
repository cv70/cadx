use cadx_config::UiLanguage;
use cadx_core::{
    CadCommand, CommandTransaction, ConstraintKind, ConstraintSolverSettings, EntityKind,
    Parameter, PointAnchor, SketchConstraint, SketchPoint, SketchSegment, ValidationReport,
    solve_constraints,
};
use eframe::egui::{self, Color32};

use crate::app::CadxApp;

impl CadxApp {
    pub(crate) fn ui_parametrics(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(egui::RichText::new(language.text("Parameters", "参数")).strong());

        let values = self.workspace.document().evaluate_parameter_values().ok();
        egui::ScrollArea::vertical()
            .id_salt("parameter_list")
            .max_height(116.0)
            .show(ui, |ui| {
                if self.workspace.document().parameters.is_empty() {
                    ui.label(
                        egui::RichText::new(language.text("No parameters", "暂无参数"))
                            .small()
                            .color(Color32::GRAY),
                    );
                }
                for parameter in self.workspace.document().parameters.values() {
                    let value = values
                        .as_ref()
                        .and_then(|values| values.get(&parameter.id))
                        .copied();
                    let source = parameter
                        .expression
                        .as_ref()
                        .map(|expression| expression.source())
                        .unwrap_or(language.text("literal", "常量"));
                    ui.label(
                        egui::RichText::new(match value {
                            Some(value) => format!("{} = {value:.4} ({source})", parameter.name),
                            None => {
                                format!("{} ({})", parameter.name, language.text("invalid", "无效"))
                            }
                        })
                        .small()
                        .color(Color32::LIGHT_GRAY),
                    );
                }
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(language.text("Name", "名称"));
            ui.add(
                egui::TextEdit::singleline(&mut self.parameter_name)
                    .desired_width(114.0)
                    .hint_text("width"),
            );
        });
        ui.horizontal(|ui| {
            ui.label(language.text("Value", "值"));
            ui.add(egui::DragValue::new(&mut self.parameter_value).speed(0.25));
        });
        ui.horizontal(|ui| {
            ui.label(language.text("Formula", "公式"));
            ui.add(
                egui::TextEdit::singleline(&mut self.parameter_expression)
                    .desired_width(170.0)
                    .hint_text("base_width * 2"),
            );
        });
        if ui
            .button(language.text("Save parameter", "保存参数"))
            .clicked()
        {
            self.save_parameter();
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(language.text("Constraints", "约束")).strong());
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    self.workspace.document().constraints.len(),
                    language.text("total", "项")
                ))
                .small()
                .color(Color32::GRAY),
            );
            if ui
                .add_enabled(
                    !self.workspace.document().constraints.is_empty(),
                    egui::Button::new(language.text("Solve", "求解")),
                )
                .clicked()
            {
                self.solve_active_constraints();
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("constraint_list")
            .max_height(116.0)
            .show(ui, |ui| {
                if self.workspace.document().constraints.is_empty() {
                    ui.label(
                        egui::RichText::new(language.text(
                            "Select a line to add orientation constraints.",
                            "选择直线以添加方向约束。",
                        ))
                        .small()
                        .color(Color32::GRAY),
                    );
                }
                for constraint in self.workspace.document().constraints.values() {
                    let diagnostic = self
                        .constraint_diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.constraint_id == constraint.id);
                    let color = match diagnostic {
                        Some(diagnostic) if diagnostic.satisfied => {
                            Color32::from_rgb(111, 220, 196)
                        }
                        Some(_) => Color32::from_rgb(231, 112, 106),
                        None => Color32::LIGHT_GRAY,
                    };
                    let residual = diagnostic
                        .map(|diagnostic| {
                            format!(
                                " {} {:.3e}",
                                language.text("residual", "残差"),
                                diagnostic.residual
                            )
                        })
                        .unwrap_or_default();
                    ui.label(
                        egui::RichText::new(format!(
                            "#{} {}{}",
                            constraint.id, constraint.name, residual
                        ))
                        .small()
                        .color(color),
                    );
                }
            });
    }

    pub(crate) fn save_parameter(&mut self) {
        let name = self.parameter_name.trim();
        if name.is_empty() {
            self.status = self
                .language
                .text("Parameter name is required.", "必须输入参数名称。")
                .into();
            return;
        }
        let existing = self
            .workspace
            .document()
            .parameters
            .values()
            .find(|parameter| parameter.name == name)
            .cloned();
        let id = existing
            .as_ref()
            .map(|parameter| parameter.id)
            .unwrap_or_else(|| self.workspace.document().next_parameter_id());
        let unit = existing
            .as_ref()
            .map(|parameter| parameter.unit)
            .unwrap_or(self.workspace.document().units);
        let parameter = if self.parameter_expression.trim().is_empty() {
            Ok(Parameter::literal(id, name, self.parameter_value, unit))
        } else {
            Parameter::formula(id, name, self.parameter_expression.trim(), unit)
        };
        let parameter = match parameter {
            Ok(parameter) => parameter,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot save parameter: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法保存参数：{error}"),
                };
                return;
            }
        };
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            if existing.is_some() {
                self.language.text("Update parameter", "更新参数")
            } else {
                self.language.text("Create parameter", "创建参数")
            },
            CommandTransaction::new(vec![CadCommand::SetParameter { parameter }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.constraint_diagnostics.clear();
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Saved parameter in semantic commit #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中保存参数")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot save parameter: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法保存参数：{error}"),
                }
            }
        }
    }

    pub(crate) fn add_orientation_constraint(&mut self, vertical: bool) {
        let Some(entity_id) = self.selected_entity else {
            return;
        };
        let Some(entity) = self.workspace.document().entities.get(&entity_id) else {
            return;
        };
        if !matches!(
            entity.kind,
            EntityKind::Line { .. } | EntityKind::Wall { .. }
        ) {
            self.status = self
                .language
                .text(
                    "Orientation constraints require a line or wall.",
                    "方向约束只能应用于直线或墙体。",
                )
                .into();
            return;
        }
        let segment = SketchSegment::new(
            SketchPoint::new(entity_id, PointAnchor::Start),
            SketchPoint::new(entity_id, PointAnchor::End),
        );
        let kind = if vertical {
            ConstraintKind::Vertical { segment }
        } else {
            ConstraintKind::Horizontal { segment }
        };
        if self
            .workspace
            .document()
            .constraints
            .values()
            .any(|constraint| constraint.kind == kind)
        {
            self.status = self
                .language
                .text(
                    "That orientation constraint already exists.",
                    "该方向约束已存在。",
                )
                .into();
            return;
        }
        let name = if vertical {
            format!("{} {entity_id}", self.language.text("Vertical", "垂直"))
        } else {
            format!("{} {entity_id}", self.language.text("Horizontal", "水平"))
        };
        let constraint = SketchConstraint {
            id: self.workspace.document().next_constraint_id(),
            name,
            driving: true,
            kind,
        };
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            self.language
                .text("Create orientation constraint", "创建方向约束"),
            CommandTransaction::new(vec![CadCommand::CreateConstraint { constraint }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.constraint_diagnostics.clear();
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Saved constraint in semantic commit #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中保存约束")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot save constraint: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法保存约束：{error}"),
                }
            }
        }
    }

    pub(crate) fn solve_active_constraints(&mut self) {
        let solution = match solve_constraints(
            self.workspace.document(),
            ConstraintSolverSettings::default(),
        ) {
            Ok(solution) => solution,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot solve constraints: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法求解约束：{error}"),
                };
                return;
            }
        };
        self.constraint_diagnostics = solution.diagnostics.clone();
        if !solution.converged {
            self.status = match self.language {
                UiLanguage::English => format!(
                    "Constraints did not converge after {} iteration(s); maximum residual {:.3e}.",
                    solution.iterations,
                    solution.maximum_driving_residual()
                ),
                UiLanguage::SimplifiedChinese => format!(
                    "约束在 {} 次迭代后仍未收敛；最大残差 {:.3e}。",
                    solution.iterations,
                    solution.maximum_driving_residual()
                ),
            };
            return;
        }
        if solution.updated_entities.is_empty() {
            self.status = self
                .language
                .text("Constraints are already satisfied.", "约束已满足。")
                .into();
            return;
        }
        let transaction = match solution.transaction() {
            Ok(transaction) => transaction,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Cannot save constraint solution: {error}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("无法保存约束解：{error}")
                    }
                };
                return;
            }
        };
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            self.language
                .text("Solve parametric constraints", "求解参数化约束"),
            transaction,
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Saved solved geometry in semantic commit #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中保存求解后的几何")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Cannot save constraint solution: {error}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("无法保存约束解：{error}")
                    }
                }
            }
        }
    }
}
