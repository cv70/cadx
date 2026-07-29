use cadx_config::UiLanguage;
use cadx_core::{
    AgentKind, ChangeSetStatus, DocumentDiff, EntityKind, HistoryComparison, ObjectId,
    RevertConflict, RevertConflictReason, TaskStatus, Units,
};
use eframe::egui::{self, Color32};

use crate::app::{CadxApp, unix_time_now};
use crate::localization::{
    agent_run_status_label, change_set_status_label, domain_label, entity_kind_label,
    pdf_orientation_label, pdf_page_size_label, task_status_label, unit_label, viewport_tool_label,
};
use crate::viewport::{ViewportMode, ViewportTool};

impl CadxApp {
    pub(crate) fn ui_top_bar(&mut self, context: &egui::Context) {
        let language = self.language;
        egui::TopBottomPanel::top("top_bar")
            .exact_height(48.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("CADX")
                            .size(21.0)
                            .strong()
                            .color(Color32::from_rgb(111, 220, 196)),
                    );
                    ui.separator();
                    let document_title =
                        if self.workspace.document().metadata.title == "Untitled CADX project" {
                            language.text("Untitled CADX project", "未命名 CADX 工程")
                        } else {
                            &self.workspace.document().metadata.title
                        };
                    let title = if self.is_dirty {
                        format!("{document_title} *")
                    } else {
                        document_title.into()
                    };
                    ui.label(egui::RichText::new(title).strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {}",
                            language.text("Branch", "分支"),
                            self.workspace.history().active_branch
                        ))
                        .color(Color32::LIGHT_GRAY),
                    );
                    if ui
                        .add_enabled(
                            self.workspace.can_undo(),
                            egui::Button::new(language.text("Undo", "撤销")),
                        )
                        .on_hover_text(language.text(
                            "Undo latest model change (Cmd/Ctrl+Z)",
                            "撤销最近的模型更改 (Cmd/Ctrl+Z)",
                        ))
                        .clicked()
                    {
                        self.undo_latest_change();
                    }
                    if ui
                        .add_enabled(
                            self.workspace.can_redo(),
                            egui::Button::new(language.text("Redo", "重做")),
                        )
                        .on_hover_text(language.text(
                            "Redo latest model change (Cmd/Ctrl+Shift+Z or Ctrl+Y)",
                            "重做最近的模型更改 (Cmd/Ctrl+Shift+Z 或 Ctrl+Y)",
                        ))
                        .clicked()
                    {
                        self.redo_latest_change();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut selected_language = self.language;
                        egui::ComboBox::from_id_salt("interface_language")
                            .selected_text(selected_language.native_name())
                            .width(88.0)
                            .show_ui(ui, |ui| {
                                for candidate in cadx_config::UiLanguage::ALL {
                                    ui.selectable_value(
                                        &mut selected_language,
                                        candidate,
                                        candidate.native_name(),
                                    );
                                }
                            });
                        if selected_language != self.language {
                            self.apply_interface_language(selected_language, true);
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {}",
                                self.workspace.document().entities.len(),
                                language.text("entities", "个实体")
                            ))
                            .color(Color32::LIGHT_GRAY),
                        );
                        ui.separator();
                        ui.label(language.text("Local workspace", "本地工作区"));
                        if ui.button(language.text("Save", "保存")).clicked() {
                            self.save_project();
                        }
                        if ui.button(language.text("Open", "打开")).clicked() {
                            self.open_project();
                        }
                        ui.add_sized(
                            [210.0, 24.0],
                            egui::TextEdit::singleline(&mut self.project_path)
                                .hint_text("project.cadx"),
                        );
                    });
                });
            });
    }

    pub(crate) fn ui_task_panel(&mut self, context: &egui::Context) {
        let language = self.language;
        egui::SidePanel::left("tasks")
            .min_width(264.0)
            .max_width(320.0)
            .show(context, |ui| {
                ui.heading(language.text("Design Tasks", "设计任务"));
                ui.add_space(4.0);
                let remote_running = self.remote_agent_running();
                ui.label(language.text("Goal", "目标"));
                ui.add_enabled(
                    !remote_running,
                    egui::TextEdit::multiline(&mut self.task_goal)
                        .desired_rows(5)
                        .hint_text(language.text("Describe the intended design", "描述设计目标")),
                );
                ui.add_enabled(
                    !remote_running,
                    egui::Checkbox::new(
                        &mut self.direct_write,
                        language.text("Allow this task to save changes", "允许此任务保存更改"),
                    ),
                );
                if ui
                    .add_enabled(
                        !remote_running,
                        egui::Checkbox::new(
                            &mut self.remote_enabled,
                            language.text("Use remote planner", "使用远程规划器"),
                        ),
                    )
                    .changed()
                {
                    self.clear_remote_context_review();
                }
                if self.remote_enabled {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {}",
                            language.text("Config", "配置"),
                            self.remote_config_path
                        ))
                        .small()
                        .color(Color32::GRAY),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                !remote_running,
                                egui::Button::new(language.text("Review context", "审查上下文")),
                            )
                            .clicked()
                        {
                            self.prepare_remote_disclosure();
                        }
                        egui::ComboBox::from_id_salt("remote_grant_duration")
                            .selected_text(remote_grant_duration_label(
                                language,
                                self.remote_grant_duration_seconds,
                            ))
                            .show_ui(ui, |ui| {
                                for duration in [60 * 60, 24 * 60 * 60, 7 * 24 * 60 * 60, 0] {
                                    ui.selectable_value(
                                        &mut self.remote_grant_duration_seconds,
                                        duration,
                                        remote_grant_duration_label(language, duration),
                                    );
                                }
                            });
                        if ui
                            .add_enabled(
                                !remote_running && self.remote_disclosure.is_some(),
                                egui::Button::new(
                                    language.text("Grant project access", "授予项目访问权限"),
                                ),
                            )
                            .clicked()
                        {
                            self.create_project_remote_grant();
                        }
                        if ui
                            .add_enabled(
                                self.remote_grant_id.is_some(),
                                egui::Button::new(language.text("Revoke grant", "撤销授权")),
                            )
                            .clicked()
                        {
                            self.revoke_remote_access_grant();
                        }
                    });
                    if let Some(disclosure) = &self.remote_disclosure {
                        ui.label(
                            egui::RichText::new(format!(
                                "CS #{} / {} #{} / {}; {} {}; {} {}",
                                disclosure.change_set_id,
                                language.text("Run", "运行"),
                                disclosure.run_id,
                                disclosure.config.model,
                                disclosure.requested_capabilities.len(),
                                language.text("capabilities", "项能力"),
                                disclosure.context.selected_entity_ids.len(),
                                language.text("selected", "个已选择对象"),
                            ))
                            .small()
                            .color(Color32::LIGHT_GRAY),
                        );
                        ui.label(
                            egui::RichText::new(remote_payload_summary(language, disclosure))
                                .small()
                                .color(Color32::GRAY),
                        );
                        ui.label(
                            egui::RichText::new(match language {
                                UiLanguage::English => format!(
                                    "Revision #{}; {} data categories; {} bytes; SHA-256 {}",
                                    disclosure.source_revision,
                                    disclosure.data_categories.len(),
                                    disclosure.payload_bytes,
                                    short_hash(&disclosure.payload_hash)
                                ),
                                UiLanguage::SimplifiedChinese => format!(
                                    "版本 #{}；{} 类数据；{} 字节；SHA-256 {}",
                                    disclosure.source_revision,
                                    disclosure.data_categories.len(),
                                    disclosure.payload_bytes,
                                    short_hash(&disclosure.payload_hash)
                                ),
                            })
                            .small()
                            .color(Color32::GRAY),
                        );
                        ui.label(
                            egui::RichText::new(&disclosure.config.endpoint)
                                .small()
                                .color(Color32::GRAY),
                        );
                        if let Some(grant_id) = self.remote_grant_id
                            && let Some(grant) =
                                self.workspace.remote_access_grants().get(&grant_id)
                        {
                            ui.label(
                                egui::RichText::new(remote_grant_status_label(
                                    language, grant,
                                ))
                                .small()
                                .color(Color32::from_rgb(111, 220, 196)),
                            );
                        }
                    }
                    if remote_running {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new(
                                    language.text("Remote planner running", "远程规划器运行中"),
                                )
                                .small()
                                .color(Color32::LIGHT_GRAY),
                            );
                        });
                    }
                }
                let active_status = self
                    .active_task
                    .and_then(|task_id| self.workspace.task(task_id))
                    .map(|task| task.status);
                let active_change_set_status = self
                    .active_task
                    .and_then(|task_id| self.workspace.task(task_id))
                    .and_then(cadx_core::DesignTask::active_change_set)
                    .map(|change_set| change_set.status);
                let can_run = active_status.is_none_or(|status| {
                    matches!(
                        status,
                        TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused
                    )
                });
                let can_add_prompt = active_status.is_some_and(|status| {
                    matches!(
                        status,
                        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                    )
                });
                let can_retry = matches!(
                    active_change_set_status,
                    Some(ChangeSetStatus::PartiallyFailed | ChangeSetStatus::Cancelled)
                );
                let can_cancel = active_status.is_some_and(|status| {
                    matches!(
                        status,
                        TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused
                    )
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !remote_running,
                            egui::Button::new(language.text("Create task", "创建任务")),
                        )
                        .clicked()
                    {
                        self.create_task();
                    }
                    if ui
                        .add_enabled(
                            !remote_running && can_add_prompt,
                            egui::Button::new(language.text("Add prompt", "添加 Prompt")),
                        )
                        .clicked()
                    {
                        self.add_prompt_to_active_task();
                    }
                    if ui
                        .add_enabled(
                            !remote_running && can_retry,
                            egui::Button::new(language.text("Retry", "重试")),
                        )
                        .clicked()
                    {
                        self.retry_active_change_set();
                    }
                    if ui
                        .add_enabled(
                            !remote_running && can_run,
                            egui::Button::new(language.text("Run agent", "运行 Agent")),
                        )
                        .clicked()
                    {
                        self.run_active_task();
                        if self.remote_agent_running() {
                            context.request_repaint();
                        }
                    }
                    if ui
                        .add_enabled(
                            !remote_running && can_run,
                            egui::Button::new(language.text("Run next", "运行下一步")),
                        )
                        .clicked()
                    {
                        self.run_active_task_step();
                        if self.remote_agent_running() {
                            context.request_repaint();
                        }
                    }
                    if ui
                        .add_enabled(
                            !remote_running && can_cancel,
                            egui::Button::new(language.text("Cancel", "取消")),
                        )
                        .clicked()
                    {
                        self.cancel_active_task();
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(egui::RichText::new(language.text("Viewport", "视口")).strong());
                let mut viewport_mode = self.viewport_mode;
                ui.horizontal(|ui| {
                    for mode in ViewportMode::ALL {
                        ui.selectable_value(&mut viewport_mode, mode, mode.label());
                    }
                    if ui.button(language.text("Fit", "适应")).clicked() {
                        self.fit_view();
                    }
                });
                if viewport_mode != self.viewport_mode {
                    self.set_viewport_mode(viewport_mode);
                }
                if self.viewport_mode == ViewportMode::Drafting2d {
                    ui.label(
                        egui::RichText::new(language.text("Tool", "工具"))
                            .small()
                            .color(Color32::GRAY),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for tool in ViewportTool::ALL {
                            if ui
                                .selectable_label(
                                    self.viewport_tool == tool,
                                    viewport_tool_label(language, tool),
                                )
                                .clicked()
                            {
                                self.viewport_tool = tool;
                                self.draw_origin = None;
                                self.arc_points.clear();
                                self.dimension_points.clear();
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(
                            &mut self.snap_geometry,
                            language.text("Geometry snap", "几何捕捉"),
                        );
                        ui.checkbox(&mut self.snap_grid, language.text("Grid snap", "网格捕捉"));
                    });
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(egui::RichText::new(language.text("Task Queue", "任务队列")).strong());
                let tasks = self.workspace.tasks().values().cloned().collect::<Vec<_>>();
                for task in tasks {
                    let selected = self.active_task == Some(task.id);
                    let label = format!("#{}  {}", task.id, task.title);
                    if ui.selectable_label(selected, label).clicked() {
                        self.select_active_task(task.id);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(task_status_label(language, task.status))
                                .small()
                                .color(task_status_color(task.status)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {}",
                                task.output_commits().count(),
                                language.text("commits", "个提交")
                            ))
                            .small()
                            .color(Color32::GRAY),
                        );
                        if let Some(execution) = task.execution()
                            && !execution.is_complete()
                        {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} {}",
                                    execution.remaining_actions(),
                                    language.text("remaining", "个待执行")
                                ))
                                .small()
                                .color(Color32::GRAY),
                            );
                        }
                    });
                }
                ui.add_space(10.0);
                let mut revert_request = None;
                if let Some(task_id) = self.active_task
                    && let Some(task) = self.workspace.tasks().get(&task_id).cloned()
                {
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(language.text("Prompt History", "Prompt 历史"))
                            .strong(),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("prompt_history")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for change_set in &task.change_sets {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("CS #{}", change_set.id))
                                            .small()
                                            .strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(change_set_status_label(
                                            language,
                                            change_set.status,
                                        ))
                                        .small()
                                        .color(Color32::GRAY),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} {}",
                                            change_set.output_commits().count(),
                                            language.text("commits", "个提交")
                                        ))
                                        .small()
                                        .color(Color32::GRAY),
                                    );
                                    let can_revert = !remote_running
                                        && matches!(
                                            change_set.status,
                                            ChangeSetStatus::Completed
                                                | ChangeSetStatus::PartiallyFailed
                                                | ChangeSetStatus::Cancelled
                                        )
                                        && change_set.compensation.is_none()
                                        && change_set.reverted_by.is_none()
                                        && change_set.output_commits().next().is_some()
                                        && matches!(
                                            task.status,
                                            TaskStatus::Completed
                                                | TaskStatus::Failed
                                                | TaskStatus::Cancelled
                                        );
                                    if ui
                                        .add_enabled(
                                            can_revert,
                                            egui::Button::new(language.text("Revert", "回滚"))
                                                .small(),
                                        )
                                        .on_hover_text(language.text(
                                            "Append a conflict-aware compensation; newer edits are preserved",
                                            "追加冲突感知补偿；保留后续编辑",
                                        ))
                                        .clicked()
                                    {
                                        revert_request = Some(change_set.id);
                                    }
                                });
                                let display_prompt = change_set.compensation.as_ref().map_or_else(
                                    || change_set.prompt.clone(),
                                    |compensation| match language {
                                        UiLanguage::English => format!(
                                            "Compensate change set {}",
                                            compensation.target_change_set_id
                                        ),
                                        UiLanguage::SimplifiedChinese => format!(
                                            "补偿变更集 {}",
                                            compensation.target_change_set_id
                                        ),
                                    },
                                );
                                ui.label(
                                    egui::RichText::new(display_prompt)
                                        .small()
                                        .color(Color32::LIGHT_GRAY),
                                );
                                if let Some(reverted_by) = change_set.reverted_by {
                                    ui.label(
                                        egui::RichText::new(match language {
                                            UiLanguage::English => {
                                                format!("Reverted by CS #{reverted_by}")
                                            }
                                            UiLanguage::SimplifiedChinese => {
                                                format!("由 CS #{reverted_by} 补偿回滚")
                                            }
                                        })
                                        .small()
                                        .color(Color32::from_rgb(231, 184, 92)),
                                    );
                                }
                                if let Some(compensation) = &change_set.compensation {
                                    ui.label(
                                        egui::RichText::new(match language {
                                            UiLanguage::English => format!(
                                                "Compensates CS #{}: {} restored, {} conflict(s)",
                                                compensation.target_change_set_id,
                                                compensation.reverted_objects.len(),
                                                compensation.conflicts.len()
                                            ),
                                            UiLanguage::SimplifiedChinese => format!(
                                                "补偿 CS #{}：恢复 {} 个对象，保留 {} 个冲突",
                                                compensation.target_change_set_id,
                                                compensation.reverted_objects.len(),
                                                compensation.conflicts.len()
                                            ),
                                        })
                                        .small()
                                        .color(Color32::from_rgb(231, 184, 92)),
                                    );
                                    for conflict in &compensation.conflicts {
                                        ui.label(
                                            egui::RichText::new(revert_conflict_label(
                                                language, conflict,
                                            ))
                                            .small()
                                            .color(Color32::from_rgb(231, 112, 106)),
                                        );
                                    }
                                }
                                for run in &change_set.runs {
                                    let identity = match run.identity.kind {
                                        AgentKind::Local => run.identity.agent.clone(),
                                        AgentKind::Remote => format!(
                                            "{} / {}",
                                            run.identity.agent,
                                            run.identity.model.as_deref().unwrap_or("-")
                                        ),
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_space(12.0);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} #{} / {} {}",
                                                language.text("Run", "运行"),
                                                run.id,
                                                language.text("attempt", "尝试"),
                                                run.attempt
                                            ))
                                            .small(),
                                        );
                                        ui.label(
                                            egui::RichText::new(agent_run_status_label(
                                                language, run.status,
                                            ))
                                            .small()
                                            .color(Color32::GRAY),
                                        );
                                    });
                                    ui.label(
                                        egui::RichText::new(format!("    {identity}"))
                                            .small()
                                            .color(Color32::DARK_GRAY),
                                    );
                                }
                                ui.add_space(4.0);
                            }
                        });
                    if let Some(change_set_id) = revert_request {
                        self.revert_change_set(change_set_id);
                    }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(language.text("Run Log", "运行日志")).strong());
                    egui::ScrollArea::vertical()
                        .id_salt("task_run_log")
                        .max_height(164.0)
                        .show(ui, |ui| {
                            for event in task.events() {
                                ui.label(
                                    egui::RichText::new(event_label(language, event))
                                        .small()
                                        .color(Color32::LIGHT_GRAY),
                                );
                            }
                        });
                }
            });
    }

    pub(crate) fn ui_model_panel(&mut self, context: &egui::Context) {
        let language = self.language;
        egui::SidePanel::right("model")
            .min_width(278.0)
            .max_width(350.0)
            .show(context, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("model_panel")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.heading(language.text("Layers", "图层"));
                        ui.add_space(4.0);
                        self.ui_layers(ui);
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(language.text("Model Graph", "模型树")).strong(),
                        );
                        ui.add_space(4.0);
                        for layer in self.workspace.document().layers.values() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  {}",
                                    if layer.visible { "v" } else { "-" },
                                    layer.name
                                ))
                                .strong(),
                            );
                            for entity in self
                                .workspace
                                .document()
                                .entities
                                .values()
                                .filter(|entity| entity.layer == layer.id)
                            {
                                let selected = self.selected_entity == Some(entity.id);
                                let label = format!(
                                    "{}  {}",
                                    entity_kind_label(language, &entity.kind),
                                    entity.name
                                );
                                if ui.selectable_label(selected, label).clicked() {
                                    self.selected_entity = Some(entity.id);
                                }
                            }
                        }
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(language.text("Inspector", "属性检查器")).strong(),
                        );
                        let mut delete_requested = false;
                        let mut horizontal_constraint_requested = false;
                        let mut vertical_constraint_requested = false;
                        if let Some(id) = self.selected_entity {
                            if let Some(entity) =
                                self.workspace.document().entities.get(&id).cloned()
                            {
                                let layer_locked = self
                                    .workspace
                                    .document()
                                    .layers
                                    .get(&entity.layer)
                                    .is_some_and(|layer| layer.locked);
                                ui.label(
                                    egui::RichText::new(&entity.name)
                                        .color(Color32::from_rgb(111, 220, 196)),
                                );
                                ui.label(format!("ID: {}", entity.id));
                                ui.label(format!(
                                    "{}: {}",
                                    language.text("Domain", "领域"),
                                    domain_label(language, entity.kind.domain())
                                ));
                                ui.label(entity_description(
                                    language,
                                    &entity.kind,
                                    self.workspace.document().units,
                                ));
                                self.ui_entity_layer_picker(ui, entity.id);
                                if layer_locked {
                                    ui.label(
                                        egui::RichText::new(
                                            language.text("Locked layer", "图层已锁定"),
                                        )
                                        .small()
                                        .color(Color32::GRAY),
                                    );
                                }
                                if matches!(
                                    entity.kind,
                                    EntityKind::Line { .. } | EntityKind::Wall { .. }
                                ) {
                                    ui.add_enabled_ui(!layer_locked, |ui| {
                                        ui.horizontal(|ui| {
                                            horizontal_constraint_requested = ui
                                                .button(language.text("Horizontal", "水平"))
                                                .clicked();
                                            vertical_constraint_requested = ui
                                                .button(language.text("Vertical", "垂直"))
                                                .clicked();
                                        });
                                    });
                                }
                                ui.add_space(4.0);
                                delete_requested = ui
                                    .add_enabled(
                                        !layer_locked,
                                        egui::Button::new(language.text("Delete", "删除")),
                                    )
                                    .clicked();
                            }
                        } else {
                            ui.label(
                                egui::RichText::new(language.text(
                                    "Select an entity from the canvas or graph.",
                                    "请从画布或模型树中选择一个实体。",
                                ))
                                .color(Color32::GRAY),
                            );
                        }
                        if delete_requested {
                            self.delete_selected_entity();
                        }
                        if horizontal_constraint_requested {
                            self.add_orientation_constraint(false);
                        }
                        if vertical_constraint_requested {
                            self.add_orientation_constraint(true);
                        }
                        self.ui_parametrics(ui);
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(language.text("DXF Exchange", "DXF 交换")).strong(),
                        );
                        ui.add_sized(
                            [ui.available_width(), 22.0],
                            egui::TextEdit::singleline(&mut self.exchange_path)
                                .hint_text("drawing.dxf"),
                        );
                        ui.horizontal(|ui| {
                            if ui.button(language.text("Import", "导入")).clicked() {
                                self.import_dxf();
                            }
                            if ui.button(language.text("Export", "导出")).clicked() {
                                self.export_dxf();
                            }
                        });
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(language.text("PDF Drawing", "PDF 图纸")).strong(),
                        );
                        ui.add_sized(
                            [ui.available_width(), 22.0],
                            egui::TextEdit::singleline(&mut self.pdf_path).hint_text("drawing.pdf"),
                        );
                        ui.horizontal_wrapped(|ui| {
                            egui::ComboBox::from_id_salt("pdf_page_size")
                                .selected_text(pdf_page_size_label(language, self.pdf_page_size))
                                .show_ui(ui, |ui| {
                                    for page_size in cadx_io::PdfPageSize::ALL {
                                        ui.selectable_value(
                                            &mut self.pdf_page_size,
                                            page_size,
                                            pdf_page_size_label(language, page_size),
                                        );
                                    }
                                });
                            egui::ComboBox::from_id_salt("pdf_orientation")
                                .selected_text(pdf_orientation_label(
                                    language,
                                    self.pdf_orientation,
                                ))
                                .show_ui(ui, |ui| {
                                    for orientation in cadx_io::PdfOrientation::ALL {
                                        ui.selectable_value(
                                            &mut self.pdf_orientation,
                                            orientation,
                                            pdf_orientation_label(language, orientation),
                                        );
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label(language.text("Margin", "边距"));
                            ui.add(
                                egui::DragValue::new(&mut self.pdf_margin_mm)
                                    .range(0.0..=100.0)
                                    .suffix(" mm"),
                            );
                            if ui.button(language.text("Export PDF", "导出 PDF")).clicked() {
                                self.export_pdf_drawing();
                            }
                        });
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(language.text("Design History", "设计历史"))
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} {}",
                                    self.workspace.history().branches.len(),
                                    language.text("branches", "个分支")
                                ))
                                .small()
                                .color(Color32::GRAY),
                            );
                            if ui
                                .add_enabled(
                                    self.comparison_base.is_some(),
                                    egui::Button::new(language.text("Compare", "比较")),
                                )
                                .clicked()
                            {
                                self.compare_from_base();
                            }
                        });
                        if let Some(base) = self.comparison_base {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} #{base} -> {} #{}",
                                    language.text("Base", "基准"),
                                    language.text("current", "当前"),
                                    self.workspace.history().head()
                                ))
                                .small()
                                .color(Color32::GRAY),
                            );
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("design_history")
                            .max_height(200.0)
                            .show(ui, |ui| {
                                let commits = self
                                    .workspace
                                    .history()
                                    .ordered_commits()
                                    .into_iter()
                                    .rev()
                                    .cloned()
                                    .collect::<Vec<_>>();
                                for commit in commits {
                                    ui.horizontal(|ui| {
                                        if ui.small_button(format!("#{}", commit.id)).clicked() {
                                            self.fork_commit(commit.id);
                                        }
                                        if ui.small_button(language.text("Base", "基准")).clicked()
                                        {
                                            self.comparison_base = Some(commit.id);
                                            self.comparison = None;
                                            self.status = match language {
                                                cadx_config::UiLanguage::English => format!(
                                                    "Comparison base set to commit #{}",
                                                    commit.id
                                                ),
                                                cadx_config::UiLanguage::SimplifiedChinese => {
                                                    format!("比较基准已设为提交 #{}", commit.id)
                                                }
                                            };
                                        }
                                        ui.vertical(|ui| {
                                            ui.label(egui::RichText::new(&commit.intent).small());
                                            ui.label(
                                                egui::RichText::new(diff_summary(
                                                    language,
                                                    &commit.diff,
                                                ))
                                                .small()
                                                .color(Color32::GRAY),
                                            );
                                        });
                                    });
                                }
                                if let Some(comparison) = self.comparison.clone() {
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "#{}/#{}: {}",
                                            comparison.base_commit,
                                            comparison.target_commit,
                                            comparison_summary(language, &comparison)
                                        ))
                                        .small()
                                        .color(Color32::from_rgb(111, 220, 196)),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} +{}  -{}  ~{}",
                                            language.text("Entities", "实体"),
                                            id_list(&comparison.added_entities),
                                            id_list(&comparison.removed_entities),
                                            id_list(&comparison.modified_entities)
                                        ))
                                        .small()
                                        .color(Color32::LIGHT_GRAY),
                                    );
                                }
                            });
                    });
            });
    }
}

fn object_id_label(language: UiLanguage, object: ObjectId) -> String {
    match object {
        ObjectId::Layer(id) => format!("{} #{id}", language.text("Layer", "图层")),
        ObjectId::Entity(id) => format!("{} #{id}", language.text("Entity", "实体")),
        ObjectId::Parameter(id) => format!("{} #{id}", language.text("Parameter", "参数")),
        ObjectId::Constraint(id) => format!("{} #{id}", language.text("Constraint", "约束")),
    }
}

fn revert_conflict_label(language: UiLanguage, conflict: &RevertConflict) -> String {
    let object = object_id_label(language, conflict.object);
    match (language, conflict.reason) {
        (UiLanguage::English, RevertConflictReason::ModifiedAfterTarget) => format!(
            "Conflict: {object} changed after target #{} at revision {:?}",
            conflict.target_revision, conflict.conflicting_revision
        ),
        (UiLanguage::SimplifiedChinese, RevertConflictReason::ModifiedAfterTarget) => format!(
            "冲突：{object} 在目标版本 #{} 后又于版本 {:?} 被修改",
            conflict.target_revision, conflict.conflicting_revision
        ),
        (UiLanguage::English, RevertConflictReason::DependencyValidationFailed) => {
            format!("Conflict: {object} cannot be restored without violating a dependency")
        }
        (UiLanguage::SimplifiedChinese, RevertConflictReason::DependencyValidationFailed) => {
            format!("冲突：{object} 无法在不破坏依赖的情况下恢复")
        }
    }
}

fn id_list(ids: &[u64]) -> String {
    if ids.is_empty() {
        "-".into()
    } else {
        ids.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn task_status_color(status: TaskStatus) -> Color32 {
    match status {
        TaskStatus::Queued | TaskStatus::Paused => Color32::GRAY,
        TaskStatus::Running => Color32::from_rgb(225, 185, 71),
        TaskStatus::Completed => Color32::from_rgb(111, 220, 196),
        TaskStatus::Failed | TaskStatus::Cancelled => Color32::from_rgb(231, 112, 106),
    }
}

fn event_label(language: UiLanguage, event: &cadx_core::TaskEvent) -> String {
    match event {
        cadx_core::TaskEvent::Observed { entity_count } => match language {
            UiLanguage::English => format!("Observed {entity_count} entities"),
            UiLanguage::SimplifiedChinese => format!("已观察 {entity_count} 个实体"),
        },
        cadx_core::TaskEvent::Reobserved {
            revision,
            action_index,
            entity_count,
        } => match language {
            UiLanguage::English => format!(
                "Re-observed revision #{revision} before action {}: {entity_count} entities",
                action_index + 1
            ),
            UiLanguage::SimplifiedChinese => format!(
                "动作 {} 前已重新观察版本 #{revision}：{entity_count} 个实体",
                action_index + 1
            ),
        },
        cadx_core::TaskEvent::ProviderDisclosure {
            endpoint,
            model,
            grant_id,
            requested_capabilities,
            selected_entity_ids,
            includes_source_files,
            context_schema_version,
            data_categories,
            source_revision,
            payload_bytes,
            payload_hash,
            ..
        } => {
            let legacy = *context_schema_version == 0
                && data_categories.is_empty()
                && *payload_bytes == 0
                && payload_hash.is_empty();
            let authorization = match (language, grant_id) {
                (UiLanguage::English, Some(grant_id)) => {
                    format!("project grant #{grant_id}")
                }
                (UiLanguage::SimplifiedChinese, Some(grant_id)) => {
                    format!("项目授权 #{grant_id}")
                }
                (UiLanguage::English, None) => "legacy authorization".into(),
                (UiLanguage::SimplifiedChinese, None) => "旧版授权".into(),
            };
            match (language, legacy) {
                (UiLanguage::English, true) => format!(
                    "Legacy remote disclosure (not hash-bound): {model} at {endpoint}; {} capabilities, {} selected, source files {}",
                    requested_capabilities.len(),
                    selected_entity_ids.len(),
                    if *includes_source_files {
                        "included"
                    } else {
                        "not included"
                    }
                ),
                (UiLanguage::SimplifiedChinese, true) => format!(
                    "旧版远程披露（未绑定哈希）：{model} @ {endpoint}；{} 项能力，{} 个已选择对象，源文件{}",
                    requested_capabilities.len(),
                    selected_entity_ids.len(),
                    if *includes_source_files {
                        "已包含"
                    } else {
                        "未包含"
                    }
                ),
                (UiLanguage::English, false) => format!(
                    "Remote send under {authorization}: {model} at {endpoint}; revision #{source_revision}, {} capabilities, {} selected, {payload_bytes} bytes, SHA-256 {}, source files {}",
                    requested_capabilities.len(),
                    selected_entity_ids.len(),
                    short_hash(payload_hash),
                    if *includes_source_files {
                        "included"
                    } else {
                        "not included"
                    }
                ),
                (UiLanguage::SimplifiedChinese, false) => format!(
                    "通过{authorization}远程发送：{model} @ {endpoint}；版本 #{source_revision}，{} 项能力，{} 个已选择对象，{payload_bytes} 字节，SHA-256 {}，源文件{}",
                    requested_capabilities.len(),
                    selected_entity_ids.len(),
                    short_hash(payload_hash),
                    if *includes_source_files {
                        "已包含"
                    } else {
                        "未包含"
                    }
                ),
            }
        }
        cadx_core::TaskEvent::Planned { action_count } => match language {
            UiLanguage::English => format!("Planned {action_count} action(s)"),
            UiLanguage::SimplifiedChinese => format!("已规划 {action_count} 个动作"),
        },
        cadx_core::TaskEvent::PlanningCompleted {
            revision,
            action_count,
            summary,
        } => match language {
            UiLanguage::English => format!(
                "Planning completed at revision #{revision} after {action_count} action(s): {summary}"
            ),
            UiLanguage::SimplifiedChinese => {
                format!("在版本 #{revision} 完成规划，共执行 {action_count} 个动作：{summary}")
            }
        },
        cadx_core::TaskEvent::ActionRejected {
            feedback,
            will_retry,
        } => {
            let kind = match (language, feedback.kind) {
                (UiLanguage::English, cadx_core::ActionFailureKind::ToolRejected) => {
                    "tool rejected"
                }
                (UiLanguage::English, cadx_core::ActionFailureKind::ValidationFailed) => {
                    "validation failed"
                }
                (UiLanguage::English, cadx_core::ActionFailureKind::StaleObservation) => {
                    "observation became stale"
                }
                (UiLanguage::SimplifiedChinese, cadx_core::ActionFailureKind::ToolRejected) => {
                    "工具拒绝"
                }
                (UiLanguage::SimplifiedChinese, cadx_core::ActionFailureKind::ValidationFailed) => {
                    "验证失败"
                }
                (UiLanguage::SimplifiedChinese, cadx_core::ActionFailureKind::StaleObservation) => {
                    "观察已过期"
                }
            };
            match (language, will_retry) {
                (UiLanguage::English, true) => format!(
                    "Action {} {kind}; automatic repair {}/{}: {}",
                    feedback.action_index + 1,
                    feedback.repair_attempt,
                    cadx_core::MAX_AUTOMATIC_REPAIR_ATTEMPTS,
                    feedback.message
                ),
                (UiLanguage::English, false) => format!(
                    "Action {} {kind}; automatic repair limit exhausted: {}",
                    feedback.action_index + 1,
                    feedback.message
                ),
                (UiLanguage::SimplifiedChinese, true) => format!(
                    "动作 {} {kind}；自动修复 {}/{}：{}",
                    feedback.action_index + 1,
                    feedback.repair_attempt,
                    cadx_core::MAX_AUTOMATIC_REPAIR_ATTEMPTS,
                    feedback.message
                ),
                (UiLanguage::SimplifiedChinese, false) => format!(
                    "动作 {} {kind}；已用尽自动修复次数：{}",
                    feedback.action_index + 1,
                    feedback.message
                ),
            }
        }
        cadx_core::TaskEvent::ToolCall { name, detail } => format!("{name}: {detail}"),
        cadx_core::TaskEvent::Committed { commit_id, summary } => match language {
            UiLanguage::English => format!("Saved #{commit_id}: {summary}"),
            UiLanguage::SimplifiedChinese => format!("已保存 #{commit_id}：{summary}"),
        },
        cadx_core::TaskEvent::Validation { summary, passed } => {
            format!(
                "{}: {summary}",
                if *passed {
                    language.text("Validated", "已验证")
                } else {
                    language.text("Attention", "需注意")
                }
            )
        }
        cadx_core::TaskEvent::Validated {
            validator_id,
            validator_version,
            candidate_state_hash,
            summary,
        } => match language {
            UiLanguage::English => format!(
                "Validated by {validator_id}@{validator_version} [{}]: {summary}",
                &candidate_state_hash[..candidate_state_hash.len().min(12)]
            ),
            UiLanguage::SimplifiedChinese => format!(
                "由 {validator_id}@{validator_version} 验证 [{}]：{summary}",
                &candidate_state_hash[..candidate_state_hash.len().min(12)]
            ),
        },
        cadx_core::TaskEvent::Paused {
            completed_actions,
            remaining_actions,
            reason,
        } => match language {
            UiLanguage::English => format!(
                "Paused after {completed_actions} action(s), {remaining_actions} remaining: {reason}"
            ),
            UiLanguage::SimplifiedChinese => format!(
                "执行 {completed_actions} 个动作后暂停，剩余 {remaining_actions} 个：{reason}"
            ),
        },
        cadx_core::TaskEvent::Resumed {
            completed_actions,
            remaining_actions,
        } => match language {
            UiLanguage::English => {
                format!("Resumed at {completed_actions} action(s), {remaining_actions} remaining")
            }
            UiLanguage::SimplifiedChinese => {
                format!("已从第 {completed_actions} 个动作恢复，剩余 {remaining_actions} 个")
            }
        },
        cadx_core::TaskEvent::Failed { message } => match language {
            UiLanguage::English => format!("Stopped: {message}"),
            UiLanguage::SimplifiedChinese => format!("已停止：{message}"),
        },
        cadx_core::TaskEvent::Cancelled { reason } => match language {
            UiLanguage::English => format!("Cancelled: {reason}"),
            UiLanguage::SimplifiedChinese => format!("已取消：{reason}"),
        },
    }
}

fn remote_payload_summary(
    language: UiLanguage,
    disclosure: &cadx_agent::ProviderDisclosure,
) -> String {
    let context = &disclosure.context;
    match language {
        UiLanguage::English => format!(
            "Task goal and document metadata; entity count: {}; {} selected entity identifier(s); geometry, attachments, and source files: {}.",
            context.entity_count,
            context.selected_entity_ids.len(),
            if context.includes_source_files {
                "included"
            } else {
                "not included"
            }
        ),
        UiLanguage::SimplifiedChinese => format!(
            "任务目标和文档元数据；仅发送实体数量 {}；{} 个已选择实体标识；几何、附件和源文件：{}。",
            context.entity_count,
            context.selected_entity_ids.len(),
            if context.includes_source_files {
                "已包含"
            } else {
                "未包含"
            }
        ),
    }
}

fn remote_grant_duration_label(language: UiLanguage, duration_seconds: u64) -> &'static str {
    match duration_seconds {
        3_600 => language.text("1 hour", "1 小时"),
        86_400 => language.text("24 hours", "24 小时"),
        604_800 => language.text("7 days", "7 天"),
        0 => language.text("Until revoked", "直到撤销"),
        _ => language.text("Custom", "自定义"),
    }
}

fn remote_grant_status_label(language: UiLanguage, grant: &cadx_core::RemoteAccessGrant) -> String {
    let scope_count = match &grant.object_scope {
        cadx_core::RemoteObjectScope::ProjectSummary => 0,
        cadx_core::RemoteObjectScope::SelectedEntities { entity_ids } => entity_ids.len(),
    };
    let expiry = match (unix_time_now().ok(), grant.expires_at_unix_seconds) {
        (_, None) => language.text("until revoked", "直到撤销").into(),
        (Some(now), Some(expires_at)) if expires_at > now => {
            let remaining = expires_at - now;
            match language {
                UiLanguage::English if remaining >= 86_400 => {
                    format!("{} day(s) remaining", remaining.div_ceil(86_400))
                }
                UiLanguage::SimplifiedChinese if remaining >= 86_400 => {
                    format!("剩余 {} 天", remaining.div_ceil(86_400))
                }
                UiLanguage::English => {
                    format!("{} hour(s) remaining", remaining.div_ceil(3_600))
                }
                UiLanguage::SimplifiedChinese => {
                    format!("剩余 {} 小时", remaining.div_ceil(3_600))
                }
            }
        }
        _ => language.text("expired", "已过期").into(),
    };
    match language {
        UiLanguage::English => format!(
            "Project grant #{} active; {expiry}; {scope_count} selected identifier(s); payload <= {} KiB",
            grant.id,
            grant.max_payload_bytes / 1_024
        ),
        UiLanguage::SimplifiedChinese => format!(
            "项目授权 #{} 有效；{expiry}；{} 个已选择标识；payload <= {} KiB",
            grant.id,
            scope_count,
            grant.max_payload_bytes / 1_024
        ),
    }
}

fn short_hash(hash: &str) -> &str {
    if hash.is_empty() {
        "-"
    } else {
        &hash[..hash.len().min(12)]
    }
}

fn entity_description(language: UiLanguage, kind: &EntityKind, units: Units) -> String {
    let unit = unit_label(units);
    match kind {
        EntityKind::Line { .. } => language
            .text("Editable drafting line", "可编辑制图直线")
            .into(),
        EntityKind::Circle { radius, .. } => match language {
            UiLanguage::English => format!("Radius: {radius:.1} {unit}"),
            UiLanguage::SimplifiedChinese => format!("半径：{radius:.1} {unit}"),
        },
        EntityKind::Arc {
            radius,
            sweep_angle,
            ..
        } => match language {
            UiLanguage::English => format!(
                "Radius: {radius:.1} {unit}, sweep: {:.1} deg",
                sweep_angle.to_degrees()
            ),
            UiLanguage::SimplifiedChinese => format!(
                "半径：{radius:.1} {unit}，扫掠角：{:.1} 度",
                sweep_angle.to_degrees()
            ),
        },
        EntityKind::AlignedDimension {
            start,
            end,
            offset,
            text_override,
        } => match language {
            UiLanguage::English => format!(
                "Aligned: {:.2} {unit}, offset: {offset:.2} {unit}{}",
                (end.x - start.x).hypot(end.y - start.y),
                text_override
                    .as_ref()
                    .map_or(String::new(), |text| format!(", text: {text}"))
            ),
            UiLanguage::SimplifiedChinese => format!(
                "对齐尺寸：{:.2} {unit}，偏移：{offset:.2} {unit}{}",
                (end.x - start.x).hypot(end.y - start.y),
                text_override
                    .as_ref()
                    .map_or(String::new(), |text| format!("，文本：{text}"))
            ),
        },
        EntityKind::Rectangle { width, height, .. } => {
            format!("{width:.1} x {height:.1} {unit}")
        }
        EntityKind::SketchProfile { points, closed } => match language {
            UiLanguage::English => format!("{} points, closed: {closed}", points.len()),
            UiLanguage::SimplifiedChinese => format!(
                "{} 个点，闭合：{}",
                points.len(),
                if *closed { "是" } else { "否" }
            ),
        },
        EntityKind::Extrude { profile, distance } => match language {
            UiLanguage::English => {
                format!("Profile #{profile}, depth {distance:.1} {unit}")
            }
            UiLanguage::SimplifiedChinese => {
                format!("轮廓 #{profile}，深度 {distance:.1} {unit}")
            }
        },
        EntityKind::Wall { thickness, .. } => match language {
            UiLanguage::English => format!("Wall thickness: {thickness:.1} {unit}"),
            UiLanguage::SimplifiedChinese => format!("墙体厚度：{thickness:.1} {unit}"),
        },
        EntityKind::Room { area, .. } => match language {
            UiLanguage::English => format!("Area: {area:.1}"),
            UiLanguage::SimplifiedChinese => format!("面积：{area:.1}"),
        },
        EntityKind::Text { content, .. } => content.clone(),
    }
}

fn diff_summary(language: UiLanguage, diff: &DocumentDiff) -> String {
    let changes = diff.created_entities.len()
        + diff.updated_entities.len()
        + diff.deleted_entities.len()
        + diff.created_layers.len()
        + diff.updated_layers.len()
        + diff.deleted_layers.len()
        + diff.updated_parameters.len()
        + diff.deleted_parameters.len()
        + diff.created_constraints.len()
        + diff.updated_constraints.len()
        + diff.deleted_constraints.len();
    match language {
        UiLanguage::English => format!("{changes} model changes"),
        UiLanguage::SimplifiedChinese => format!("{changes} 项模型更改"),
    }
}

fn comparison_summary(language: UiLanguage, comparison: &HistoryComparison) -> String {
    let additions = comparison.added_entities.len()
        + comparison.added_layers.len()
        + comparison.added_parameters.len()
        + comparison.added_constraints.len();
    let removals = comparison.removed_entities.len()
        + comparison.removed_layers.len()
        + comparison.removed_parameters.len()
        + comparison.removed_constraints.len();
    let modifications = comparison.modified_entities.len()
        + comparison.modified_layers.len()
        + comparison.modified_parameters.len()
        + comparison.modified_constraints.len()
        + usize::from(comparison.metadata_changed)
        + usize::from(comparison.units_changed);
    match language {
        UiLanguage::English => {
            format!("{additions} added, {removals} removed, {modifications} modified")
        }
        UiLanguage::SimplifiedChinese => {
            format!("新增 {additions}，删除 {removals}，修改 {modifications}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use cadx_core::{
        ActionFailureFeedback, ActionFailureKind, CadDocument, Capability,
        RemoteAccessGrantRequest, RemoteDataCategory, RemoteObjectScope, TaskEvent, TaskWorkspace,
    };

    use super::*;

    fn legacy_remote_disclosure() -> TaskEvent {
        TaskEvent::ProviderDisclosure {
            endpoint: "https://provider.example/v1".into(),
            model: "legacy-model".into(),
            project_id: None,
            grant_id: None,
            sent_at_unix_seconds: None,
            requested_capabilities: BTreeSet::from([Capability::Drafting]),
            selected_entity_ids: vec![42],
            includes_source_files: false,
            payload_summary: "Legacy context summary".into(),
            context_schema_version: 0,
            source_revision: 0,
            data_categories: BTreeSet::new(),
            payload_bytes: 0,
            payload_hash: String::new(),
        }
    }

    #[test]
    fn legacy_remote_audit_is_explicitly_unbound_in_both_languages() {
        let event = legacy_remote_disclosure();

        assert!(event_label(UiLanguage::English, &event).contains("not hash-bound"));
        assert!(event_label(UiLanguage::SimplifiedChinese, &event).contains("未绑定哈希"));
    }

    #[test]
    fn project_grant_controls_and_status_are_bilingual() {
        let mut workspace = TaskWorkspace::new(CadDocument::new("Grant labels"));
        let grant_id = workspace
            .kernel()
            .create_remote_access_grant(RemoteAccessGrantRequest {
                endpoint: "https://provider.example/v1".into(),
                model: "cad-model".into(),
                allowed_data_categories: BTreeSet::from([RemoteDataCategory::TaskGoal]),
                allowed_capabilities: BTreeSet::from([Capability::Drafting]),
                object_scope: RemoteObjectScope::ProjectSummary,
                max_payload_bytes: cadx_core::MAX_REMOTE_CONTEXT_BYTES,
                granted_at_unix_seconds: 1,
                expires_at_unix_seconds: None,
            })
            .unwrap();
        let grant = &workspace.remote_access_grants()[&grant_id];

        assert_eq!(
            remote_grant_duration_label(UiLanguage::English, 86_400),
            "24 hours"
        );
        assert_eq!(
            remote_grant_duration_label(UiLanguage::SimplifiedChinese, 0),
            "直到撤销"
        );
        assert!(remote_grant_status_label(UiLanguage::English, grant).contains("Project grant"));
        assert!(
            remote_grant_status_label(UiLanguage::SimplifiedChinese, grant).contains("项目授权")
        );
    }

    #[test]
    fn compensation_conflict_object_labels_are_bilingual() {
        assert_eq!(
            object_id_label(UiLanguage::English, ObjectId::Entity(42)),
            "Entity #42"
        );
        assert_eq!(
            object_id_label(UiLanguage::SimplifiedChinese, ObjectId::Constraint(7)),
            "约束 #7"
        );
    }

    #[test]
    fn iterative_repair_events_are_bilingual() {
        let event = TaskEvent::ActionRejected {
            feedback: ActionFailureFeedback {
                action_index: 1,
                observed_revision: 7,
                repair_attempt: 2,
                kind: ActionFailureKind::ValidationFailed,
                intent: "Create feature".into(),
                tool_name: "mechanical.create_feature".into(),
                message: "candidate did not pass local validation".into(),
            },
            will_retry: true,
        };

        let english = event_label(UiLanguage::English, &event);
        let chinese = event_label(UiLanguage::SimplifiedChinese, &event);
        assert!(english.contains("automatic repair 2/3"));
        assert!(english.contains("validation failed"));
        assert!(chinese.contains("自动修复 2/3"));
        assert!(chinese.contains("验证失败"));
    }
}
