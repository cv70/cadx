use cadx_agent::{HeuristicPlanner, TaskAgent};
use cadx_core::{CadDocument, EntityKind, TaskAuthority, TaskId, TaskStatus, TaskWorkspace};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 920.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CADX",
        options,
        Box::new(|creation_context| {
            configure_style(&creation_context.egui_ctx);
            Ok(Box::new(CadxApp::default()))
        }),
    )
}

fn configure_style(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(20, 25, 29);
    style.visuals.window_fill = Color32::from_rgb(24, 31, 35);
    style.visuals.extreme_bg_color = Color32::from_rgb(14, 18, 21);
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(27, 35, 39);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(33, 43, 48);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(47, 71, 74);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(59, 113, 108);
    style.visuals.selection.bg_fill = Color32::from_rgb(36, 120, 111);
    context.set_style(style);
}

struct CadxApp {
    workspace: TaskWorkspace,
    planner: TaskAgent<HeuristicPlanner>,
    task_goal: String,
    direct_write: bool,
    active_task: Option<TaskId>,
    selected_entity: Option<u64>,
    status: String,
    next_branch_number: u64,
}

impl Default for CadxApp {
    fn default() -> Self {
        Self {
            workspace: TaskWorkspace::new(CadDocument::new("Untitled CADX project")),
            planner: TaskAgent::new(HeuristicPlanner),
            task_goal: "Create a mechanical mounting bracket".into(),
            direct_write: true,
            active_task: None,
            selected_entity: None,
            status: "Ready for a design task".into(),
            next_branch_number: 1,
        }
    }
}

impl CadxApp {
    fn create_task(&mut self) {
        let goal = self.task_goal.trim();
        if goal.is_empty() {
            self.status = "Enter a design goal before creating a task.".into();
            return;
        }
        let authority = if self.direct_write {
            TaskAuthority::all_direct()
        } else {
            TaskAuthority::ReviewOnly
        };
        let title = goal.chars().take(42).collect::<String>();
        let task_id = self.workspace.create_task(title, goal, authority);
        self.active_task = Some(task_id);
        self.status = format!(
            "Task {task_id} created on {}",
            self.workspace.history.active_branch
        );
    }

    fn run_active_task(&mut self) {
        let Some(task_id) = self.active_task else {
            self.create_task();
            return self.run_active_task();
        };
        match self.planner.run(&mut self.workspace, task_id) {
            Ok(report) => {
                self.status = format!(
                    "Task {} saved {} semantic commit(s) on {}",
                    report.task_id,
                    report.commit_ids.len(),
                    self.workspace.history.active_branch
                );
            }
            Err(error) => self.status = format!("Task {task_id} stopped: {error}"),
        }
    }

    fn fork_commit(&mut self, commit_id: u64) {
        let branch = format!("option-{}", self.next_branch_number);
        self.next_branch_number += 1;
        match self.workspace.checkout_as_branch(branch.clone(), commit_id) {
            Ok(()) => self.status = format!("Opened commit {commit_id} on branch {branch}"),
            Err(error) => self.status = format!("Cannot open commit {commit_id}: {error}"),
        }
    }

    fn ui_top_bar(&mut self, context: &egui::Context) {
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
                    ui.label(egui::RichText::new(&self.workspace.document.metadata.title).strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "Branch: {}",
                            self.workspace.history.active_branch
                        ))
                        .color(Color32::LIGHT_GRAY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} entities",
                                self.workspace.document.entities.len()
                            ))
                            .color(Color32::LIGHT_GRAY),
                        );
                        ui.separator();
                        ui.label("Local workspace");
                    });
                });
            });
    }

    fn ui_task_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("tasks")
            .min_width(264.0)
            .max_width(320.0)
            .show(context, |ui| {
                ui.heading("Design Tasks");
                ui.add_space(4.0);
                ui.label("Goal");
                ui.add(
                    egui::TextEdit::multiline(&mut self.task_goal)
                        .desired_rows(5)
                        .hint_text("Describe the intended design"),
                );
                ui.checkbox(&mut self.direct_write, "Allow this task to save changes");
                ui.horizontal(|ui| {
                    if ui.button("Create task").clicked() {
                        self.create_task();
                    }
                    if ui.button("Run agent").clicked() {
                        self.run_active_task();
                    }
                });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Task Queue").strong());
                let tasks = self.workspace.tasks.values().cloned().collect::<Vec<_>>();
                for task in tasks {
                    let selected = self.active_task == Some(task.id);
                    let label = format!("#{}  {}", task.id, task.title);
                    if ui.selectable_label(selected, label).clicked() {
                        self.active_task = Some(task.id);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(task_status_label(task.status))
                                .small()
                                .color(task_status_color(task.status)),
                        );
                        ui.label(
                            egui::RichText::new(format!("{} commits", task.output_commits.len()))
                                .small()
                                .color(Color32::GRAY),
                        );
                    });
                }
                ui.add_space(10.0);
                if let Some(task_id) = self.active_task
                    && let Some(task) = self.workspace.tasks.get(&task_id)
                {
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Run Log").strong());
                    egui::ScrollArea::vertical()
                        .max_height(164.0)
                        .show(ui, |ui| {
                            for event in &task.events {
                                ui.label(
                                    egui::RichText::new(event_label(event))
                                        .small()
                                        .color(Color32::LIGHT_GRAY),
                                );
                            }
                        });
                }
            });
    }

    fn ui_model_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::right("model")
            .min_width(278.0)
            .max_width(350.0)
            .show(context, |ui| {
                ui.heading("Model Graph");
                ui.add_space(4.0);
                for layer in self.workspace.document.layers.values() {
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
                        .document
                        .entities
                        .values()
                        .filter(|entity| entity.layer == layer.id)
                    {
                        let selected = self.selected_entity == Some(entity.id);
                        let label = format!("{}  {}", entity_icon(&entity.kind), entity.name);
                        if ui.selectable_label(selected, label).clicked() {
                            self.selected_entity = Some(entity.id);
                        }
                    }
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Inspector").strong());
                if let Some(id) = self.selected_entity {
                    if let Some(entity) = self.workspace.document.entities.get(&id) {
                        ui.label(
                            egui::RichText::new(&entity.name)
                                .color(Color32::from_rgb(111, 220, 196)),
                        );
                        ui.label(format!("ID: {}", entity.id));
                        ui.label(format!("Domain: {:?}", entity.kind.domain()));
                        ui.label(entity_description(&entity.kind));
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Select an entity from the canvas or graph.")
                            .color(Color32::GRAY),
                    );
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Design History").strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "{} branches",
                            self.workspace.history.branches.len()
                        ))
                        .small()
                        .color(Color32::GRAY),
                    );
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let commits = self
                        .workspace
                        .history
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
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&commit.intent).small());
                                ui.label(
                                    egui::RichText::new(commit.diff.summary())
                                        .small()
                                        .color(Color32::GRAY),
                                );
                            });
                        });
                    }
                });
            });
    }

    fn ui_viewport(&mut self, context: &egui::Context) {
        egui::CentralPanel::default().show(context, |ui| {
            let available = ui.available_size();
            let (response, painter) = ui.allocate_painter(available, Sense::click());
            let rect = response.rect;
            painter.rect_filled(rect, 0.0, Color32::from_rgb(13, 18, 21));
            draw_grid(&painter, rect);
            painter.text(
                rect.left_top() + Vec2::new(16.0, 14.0),
                Align2::LEFT_TOP,
                "MODEL SPACE",
                FontId::proportional(12.0),
                Color32::from_gray(130),
            );
            for entity in self.workspace.document.entities.values() {
                let selected = self.selected_entity == Some(entity.id);
                draw_entity(&painter, rect, entity, selected);
            }
            if response.clicked() {
                self.selected_entity = self
                    .workspace
                    .document
                    .entities
                    .values()
                    .next_back()
                    .map(|entity| entity.id);
            }
            if self.workspace.document.entities.is_empty() {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Create a design task to begin",
                    FontId::proportional(18.0),
                    Color32::from_gray(135),
                );
            }
        });
    }

    fn ui_status_bar(&mut self, context: &egui::Context) {
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
                            egui::RichText::new("Units: mm")
                                .small()
                                .color(Color32::GRAY),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new("History auto-saved")
                                .small()
                                .color(Color32::from_rgb(111, 220, 196)),
                        );
                    });
                });
            });
    }
}

impl eframe::App for CadxApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui_top_bar(context);
        self.ui_status_bar(context);
        self.ui_task_panel(context);
        self.ui_model_panel(context);
        self.ui_viewport(context);
    }
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "Queued",
        TaskStatus::Running => "Running",
        TaskStatus::Paused => "Paused",
        TaskStatus::Completed => "Saved",
        TaskStatus::Failed => "Stopped",
    }
}

fn task_status_color(status: TaskStatus) -> Color32 {
    match status {
        TaskStatus::Queued | TaskStatus::Paused => Color32::GRAY,
        TaskStatus::Running => Color32::from_rgb(225, 185, 71),
        TaskStatus::Completed => Color32::from_rgb(111, 220, 196),
        TaskStatus::Failed => Color32::from_rgb(231, 112, 106),
    }
}

fn event_label(event: &cadx_core::TaskEvent) -> String {
    match event {
        cadx_core::TaskEvent::Observed { entity_count } => {
            format!("Observed {entity_count} entities")
        }
        cadx_core::TaskEvent::Planned { action_count } => {
            format!("Planned {action_count} action(s)")
        }
        cadx_core::TaskEvent::ToolCall { name, detail } => format!("{name}: {detail}"),
        cadx_core::TaskEvent::Committed { commit_id, summary } => {
            format!("Saved #{commit_id}: {summary}")
        }
        cadx_core::TaskEvent::Validation { summary, passed } => {
            format!(
                "{}: {summary}",
                if *passed { "Validated" } else { "Attention" }
            )
        }
        cadx_core::TaskEvent::Failed { message } => format!("Stopped: {message}"),
    }
}

fn entity_icon(kind: &EntityKind) -> &'static str {
    match kind {
        EntityKind::Line { .. } => "Line",
        EntityKind::Circle { .. } => "Circle",
        EntityKind::Rectangle { .. } => "Rect",
        EntityKind::SketchProfile { .. } => "Sketch",
        EntityKind::Extrude { .. } => "Solid",
        EntityKind::Wall { .. } => "Wall",
        EntityKind::Room { .. } => "Room",
        EntityKind::Text { .. } => "Text",
    }
}

fn entity_description(kind: &EntityKind) -> String {
    match kind {
        EntityKind::Line { .. } => "Editable drafting line".into(),
        EntityKind::Circle { radius, .. } => format!("Radius: {radius:.1} mm"),
        EntityKind::Rectangle { width, height, .. } => format!("{width:.1} x {height:.1} mm"),
        EntityKind::SketchProfile { points, closed } => {
            format!("{} points, closed: {closed}", points.len())
        }
        EntityKind::Extrude { profile, distance } => {
            format!("Profile #{profile}, depth {distance:.1} mm")
        }
        EntityKind::Wall { thickness, .. } => format!("Wall thickness: {thickness:.1} mm"),
        EntityKind::Room { area, .. } => format!("Area: {area:.1}"),
        EntityKind::Text { content, .. } => content.clone(),
    }
}

fn draw_grid(painter: &egui::Painter, rect: Rect) {
    let spacing = 32.0;
    let stroke = Stroke::new(1.0, Color32::from_rgb(28, 40, 44));
    let mut x = rect.left();
    while x < rect.right() {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
        x += spacing;
    }
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            stroke,
        );
        y += spacing;
    }
    painter.line_segment(
        [
            Pos2::new(rect.center().x, rect.top()),
            Pos2::new(rect.center().x, rect.bottom()),
        ],
        Stroke::new(1.0, Color32::from_rgb(53, 88, 89)),
    );
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.center().y),
            Pos2::new(rect.right(), rect.center().y),
        ],
        Stroke::new(1.0, Color32::from_rgb(53, 88, 89)),
    );
}

fn project(rect: Rect, point: cadx_core::Point2) -> Pos2 {
    Pos2::new(
        rect.center().x + (point.x as f32) * 3.2,
        rect.center().y - (point.y as f32) * 3.2,
    )
}

fn draw_entity(painter: &egui::Painter, rect: Rect, entity: &cadx_core::Entity, selected: bool) {
    let color = if selected {
        Color32::from_rgb(255, 205, 94)
    } else {
        Color32::from_rgb(111, 220, 196)
    };
    let stroke = Stroke::new(if selected { 2.8 } else { 1.8 }, color);
    match &entity.kind {
        EntityKind::Line { start, end } => {
            painter.line_segment([project(rect, *start), project(rect, *end)], stroke);
        }
        EntityKind::Circle { center, radius } => {
            painter.circle_stroke(project(rect, *center), (*radius as f32) * 3.2, stroke);
        }
        EntityKind::Rectangle {
            origin,
            width,
            height,
        } => {
            let min = project(rect, *origin);
            let max = project(
                rect,
                cadx_core::Point2::new(origin.x + width, origin.y + height),
            );
            painter.rect_stroke(
                Rect::from_two_pos(min, max),
                0.0,
                stroke,
                StrokeKind::Middle,
            );
        }
        EntityKind::SketchProfile { points, closed } => {
            for window in points.windows(2) {
                painter.line_segment([project(rect, window[0]), project(rect, window[1])], stroke);
            }
            if *closed && points.len() > 2 {
                painter.line_segment(
                    [
                        project(rect, points[points.len() - 1]),
                        project(rect, points[0]),
                    ],
                    stroke,
                );
            }
        }
        EntityKind::Extrude { profile, .. } => {
            painter.text(
                rect.left_bottom() + Vec2::new(16.0, -18.0),
                Align2::LEFT_BOTTOM,
                format!("Solid feature from profile #{profile}"),
                FontId::proportional(12.0),
                color,
            );
        }
        EntityKind::Wall {
            start,
            end,
            thickness,
        } => {
            painter.line_segment(
                [project(rect, *start), project(rect, *end)],
                Stroke::new((*thickness as f32 / 120.0).max(2.0), color),
            );
        }
        EntityKind::Room { boundary, .. } => {
            let points = boundary
                .iter()
                .map(|point| project(rect, *point))
                .collect::<Vec<_>>();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        EntityKind::Text { position, content } => {
            painter.text(
                project(rect, *position),
                Align2::LEFT_CENTER,
                content,
                FontId::proportional(15.0),
                color,
            );
        }
    };
}
