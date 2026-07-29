use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cadx_agent::{
    AgentError, AgentRunReport, ExecutionBudget, GenAiRemotePlanner, HeuristicPlanner,
    ProviderConfig, ProviderDisclosure, TaskAgent,
};
use cadx_config::UiLanguage;
use cadx_config::{CadxConfig, default_config_path, default_project_path};
use cadx_core::{
    AgentRunIdentity, CadCommand, CadDocument, CommandTransaction, ConstraintDiagnostic, Entity,
    EntityKind, HistoryComparison, LayerId, Point2, PromptChangeSetId, RemoteGrantId,
    TaskAuthority, TaskId, TaskStatus, TaskWorkspace, ValidationReport,
};
use cadx_io::{PROJECT_EXTENSION, PdfOrientation, PdfPageSize, load_workspace, save_workspace};
use cadx_render::{
    MechanicalScene, OrbitCamera, RenderScene, ViewTransform, ViewportSize,
    aligned_dimension_offset,
};
use eframe::egui;

use crate::drawing::{arc_from_three_points, point_distance};
use crate::exchange::{default_exchange_path, default_pdf_path};
use crate::gpu_viewport::MechanicalGpuScene;
use crate::localization::DEFAULT_TASK_GOAL_EN;
use crate::recovery::RecoveryController;
use crate::viewport::{ViewportMode, ViewportTool};

const REMOTE_AGENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const DEFAULT_REMOTE_GRANT_DURATION_SECONDS: u64 = 24 * 60 * 60;

pub(crate) struct RemoteAgentJob {
    task_id: TaskId,
    base_workspace: TaskWorkspace,
    receiver: Receiver<RemoteAgentOutput>,
    handle: JoinHandle<()>,
}

struct RemoteAgentOutput {
    workspace: TaskWorkspace,
    result: Result<AgentRunReport, String>,
}

pub(crate) struct CadxApp {
    pub(crate) language: UiLanguage,
    pub(crate) workspace: TaskWorkspace,
    pub(crate) recovery: RecoveryController,
    pub(crate) planner: TaskAgent<HeuristicPlanner>,
    pub(crate) task_goal: String,
    pub(crate) direct_write: bool,
    pub(crate) remote_enabled: bool,
    pub(crate) remote_config_path: String,
    pub(crate) remote_disclosure: Option<ProviderDisclosure>,
    pub(crate) remote_grant_id: Option<RemoteGrantId>,
    pub(crate) remote_grant_duration_seconds: u64,
    pub(crate) remote_agent_job: Option<RemoteAgentJob>,
    pub(crate) active_task: Option<TaskId>,
    pub(crate) selected_entity: Option<u64>,
    pub(crate) active_layer: LayerId,
    pub(crate) new_layer_name: String,
    pub(crate) new_layer_color: [u8; 4],
    pub(crate) layer_edit_id: Option<LayerId>,
    pub(crate) layer_name_edit: String,
    pub(crate) layer_color_edit: [u8; 4],
    pub(crate) pending_layer_delete: Option<LayerId>,
    pub(crate) delete_target_layer: Option<LayerId>,
    pub(crate) status: String,
    pub(crate) next_branch_number: u64,
    pub(crate) project_path: String,
    pub(crate) current_project_path: Option<PathBuf>,
    pub(crate) exchange_path: String,
    pub(crate) pdf_path: String,
    pub(crate) pdf_page_size: PdfPageSize,
    pub(crate) pdf_orientation: PdfOrientation,
    pub(crate) pdf_margin_mm: f32,
    pub(crate) is_dirty: bool,
    pub(crate) comparison_base: Option<u64>,
    pub(crate) comparison: Option<HistoryComparison>,
    pub(crate) view_transform: ViewTransform,
    pub(crate) viewport_size: ViewportSize,
    pub(crate) viewport_mode: ViewportMode,
    pub(crate) viewport_tool: ViewportTool,
    pub(crate) orbit_camera: OrbitCamera,
    pub(crate) mechanical_scene: MechanicalScene,
    pub(crate) mechanical_scene_head: Option<u64>,
    pub(crate) mechanical_scene_error: Option<String>,
    pub(crate) mechanical_gpu_scene: Arc<MechanicalGpuScene>,
    pub(crate) mechanical_gpu_revision: u64,
    pub(crate) mechanical_gpu_error: Option<String>,
    pub(crate) mechanical_fit_pending: bool,
    pub(crate) gpu_adapter: String,
    pub(crate) snap_geometry: bool,
    pub(crate) snap_grid: bool,
    pub(crate) draw_origin: Option<Point2>,
    pub(crate) arc_points: Vec<Point2>,
    pub(crate) dimension_points: Vec<Point2>,
    pub(crate) parameter_name: String,
    pub(crate) parameter_value: f64,
    pub(crate) parameter_expression: String,
    pub(crate) constraint_diagnostics: Vec<ConstraintDiagnostic>,
}

impl Default for CadxApp {
    fn default() -> Self {
        let workspace = TaskWorkspace::new(CadDocument::new("Untitled CADX project"));
        let recovery = RecoveryController::new(&workspace);
        let project_path = default_project_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| format!("Untitled.{PROJECT_EXTENSION}"));
        let exchange_path = default_exchange_path(&project_path);
        let pdf_path = default_pdf_path(&project_path);
        let mut app = Self {
            language: UiLanguage::English,
            workspace,
            recovery,
            planner: TaskAgent::new(HeuristicPlanner),
            task_goal: DEFAULT_TASK_GOAL_EN.into(),
            direct_write: true,
            remote_enabled: false,
            remote_config_path: display_default_config_path(),
            remote_disclosure: None,
            remote_grant_id: None,
            remote_grant_duration_seconds: DEFAULT_REMOTE_GRANT_DURATION_SECONDS,
            remote_agent_job: None,
            active_task: None,
            selected_entity: None,
            active_layer: 1,
            new_layer_name: String::new(),
            new_layer_color: [90, 160, 235, 255],
            layer_edit_id: Some(1),
            layer_name_edit: "Concept".into(),
            layer_color_edit: [73, 184, 165, 255],
            pending_layer_delete: None,
            delete_target_layer: None,
            status: "Ready for a design task".into(),
            next_branch_number: 1,
            project_path,
            current_project_path: None,
            exchange_path,
            pdf_path,
            pdf_page_size: PdfPageSize::A4,
            pdf_orientation: PdfOrientation::Landscape,
            pdf_margin_mm: 12.0,
            is_dirty: true,
            comparison_base: None,
            comparison: None,
            view_transform: ViewTransform::default(),
            viewport_size: ViewportSize::new(900.0, 700.0),
            viewport_mode: ViewportMode::Drafting2d,
            viewport_tool: ViewportTool::Select,
            orbit_camera: OrbitCamera::default(),
            mechanical_scene: MechanicalScene::default(),
            mechanical_scene_head: None,
            mechanical_scene_error: None,
            mechanical_gpu_scene: Arc::new(MechanicalGpuScene::default()),
            mechanical_gpu_revision: 0,
            mechanical_gpu_error: None,
            mechanical_fit_pending: true,
            gpu_adapter: "Unavailable".into(),
            snap_geometry: true,
            snap_grid: true,
            draw_origin: None,
            arc_points: Vec::new(),
            dimension_points: Vec::new(),
            parameter_name: String::new(),
            parameter_value: 0.0,
            parameter_expression: String::new(),
            constraint_diagnostics: Vec::new(),
        };
        let project_path = app.project_path.clone();
        app.offer_recovery(&project_path, false);
        app
    }
}

impl CadxApp {
    pub(crate) fn create_task(&mut self) {
        let goal = self.task_goal.trim();
        if goal.is_empty() {
            self.status = self
                .language
                .text(
                    "Enter a design goal before creating a task.",
                    "创建任务前请输入设计目标。",
                )
                .into();
            return;
        }
        let authority = if self.direct_write {
            TaskAuthority::all_direct()
        } else {
            TaskAuthority::ReviewOnly
        };
        let title = goal.chars().take(42).collect::<String>();
        let task_id = self.workspace.kernel().create_task(title, goal, authority);
        self.active_task = Some(task_id);
        self.clear_remote_context_review();
        self.is_dirty = true;
        self.status = match self.language {
            UiLanguage::English => format!(
                "Task {task_id} created on {}",
                self.workspace.history().active_branch
            ),
            UiLanguage::SimplifiedChinese => format!(
                "任务 {task_id} 已创建于分支 {}",
                self.workspace.history().active_branch
            ),
        };
    }

    pub(crate) fn add_prompt_to_active_task(&mut self) {
        let Some(task_id) = self.active_task else {
            self.create_task();
            return;
        };
        let prompt = self.task_goal.trim();
        if prompt.is_empty() {
            self.status = self
                .language
                .text("Enter a prompt before adding it.", "添加前请输入 Prompt。")
                .into();
            return;
        }
        let authority = if self.direct_write {
            TaskAuthority::all_direct()
        } else {
            TaskAuthority::ReviewOnly
        };
        match self
            .workspace
            .kernel()
            .add_prompt(task_id, prompt, authority)
        {
            Ok(change_set_id) => {
                self.clear_remote_context_review();
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Prompt change set {change_set_id} added to task {task_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已向任务 {task_id} 添加 Prompt 变更集 {change_set_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot add prompt: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法添加 Prompt：{error}"),
                };
            }
        }
    }

    pub(crate) fn retry_active_change_set(&mut self) {
        let Some(task_id) = self.active_task else {
            return;
        };
        match self.workspace.kernel().retry_active_change_set(task_id) {
            Ok(run_id) => {
                self.clear_remote_context_review();
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => format!("Agent run {run_id} queued for retry"),
                    UiLanguage::SimplifiedChinese => {
                        format!("Agent 运行 {run_id} 已进入重试队列")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot retry prompt: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法重试 Prompt：{error}"),
                };
            }
        }
    }

    pub(crate) fn revert_change_set(&mut self, change_set_id: PromptChangeSetId) {
        let Some(task_id) = self.active_task else {
            return;
        };
        match self
            .workspace
            .kernel()
            .revert_change_set(task_id, change_set_id)
        {
            Ok(report) => {
                self.after_history_navigation();
                self.status = match self.language {
                    UiLanguage::English if report.conflicts.is_empty() => format!(
                        "Reverted change set {change_set_id}: {} object(s) restored in compensation change set {}",
                        report.reverted_objects.len(),
                        report.compensation_change_set_id
                    ),
                    UiLanguage::SimplifiedChinese if report.conflicts.is_empty() => format!(
                        "已回滚变更集 {change_set_id}：在补偿变更集 {} 中恢复 {} 个对象",
                        report.compensation_change_set_id,
                        report.reverted_objects.len()
                    ),
                    UiLanguage::English => format!(
                        "Reverted change set {change_set_id} with {} conflict(s); {} unchanged object(s) restored",
                        report.conflicts.len(),
                        report.reverted_objects.len()
                    ),
                    UiLanguage::SimplifiedChinese => format!(
                        "变更集 {change_set_id} 已带冲突回滚：保留 {} 个冲突，恢复 {} 个未变对象",
                        report.conflicts.len(),
                        report.reverted_objects.len()
                    ),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot revert change set: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法回滚变更集：{error}"),
                };
            }
        }
    }

    pub(crate) fn cancel_active_task(&mut self) {
        let Some(task_id) = self.active_task else {
            return;
        };
        let reason = self.language.text("Cancelled by the user", "由用户取消");
        match self.workspace.kernel().cancel_task(task_id, reason) {
            Ok(()) => {
                self.clear_remote_context_review();
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => format!("Task {task_id} cancelled"),
                    UiLanguage::SimplifiedChinese => format!("任务 {task_id} 已取消"),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot cancel task: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法取消任务：{error}"),
                };
            }
        }
    }

    pub(crate) fn run_active_task(&mut self) {
        if self.remote_agent_job.is_some() {
            self.status = self
                .language
                .text(
                    "A remote planner request is already running.",
                    "已有远程规划器请求正在运行。",
                )
                .into();
            return;
        }
        if self.remote_enabled {
            self.run_active_remote_task(ExecutionBudget::default().max_actions_per_run);
            return;
        }
        self.run_active_task_with_budget(None);
    }

    pub(crate) fn run_active_task_step(&mut self) {
        if self.remote_agent_job.is_some() {
            self.status = self
                .language
                .text(
                    "A remote planner request is already running.",
                    "已有远程规划器请求正在运行。",
                )
                .into();
            return;
        }
        if self.remote_enabled {
            self.run_active_remote_task(1);
            return;
        }
        self.run_active_task_with_budget(Some(1));
    }

    pub(crate) fn run_active_task_with_budget(&mut self, action_budget: Option<usize>) {
        let Some(task_id) = self.active_task else {
            self.create_task();
            if self.active_task.is_none() {
                return;
            }
            return self.run_active_task_with_budget(action_budget);
        };
        match self
            .planner
            .run_with_action_budget(&mut self.workspace, task_id, action_budget)
        {
            Ok(report) => {
                self.is_dirty = true;
                self.status = task_run_status(
                    self.language,
                    false,
                    &report,
                    &self.workspace.history().active_branch,
                );
            }
            Err(error) => {
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => format!("Task {task_id} stopped: {error}"),
                    UiLanguage::SimplifiedChinese => format!("任务 {task_id} 已停止：{error}"),
                };
            }
        }
    }

    pub(crate) fn fork_commit(&mut self, commit_id: u64) {
        while self
            .workspace
            .history()
            .branches
            .contains_key(&format!("option-{}", self.next_branch_number))
        {
            self.next_branch_number += 1;
        }
        let branch = format!("option-{}", self.next_branch_number);
        self.next_branch_number += 1;
        match self
            .workspace
            .kernel()
            .checkout_as_branch(branch.clone(), commit_id)
        {
            Ok(()) => {
                self.sync_layer_state();
                self.is_dirty = true;
                self.comparison = None;
                self.constraint_diagnostics.clear();
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Opened commit {commit_id} on branch {branch}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已在分支 {branch} 上打开提交 {commit_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot open commit {commit_id}: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法打开提交 {commit_id}：{error}")
                    }
                }
            }
        }
    }

    pub(crate) fn save_project(&mut self) {
        let path = self.project_path.trim();
        if path.is_empty() {
            self.status = self
                .language
                .text(
                    "Enter a project path before saving.",
                    "保存前请输入工程路径。",
                )
                .into();
            return;
        }
        let path = ensure_project_extension(path);
        self.finish_recovery_job_blocking();
        match save_workspace(&self.workspace, &path) {
            Ok(report) => {
                self.project_path = report.path.display().to_string();
                self.current_project_path = Some(report.path.clone());
                self.is_dirty = false;
                self.primary_save_completed();
                self.status = match self.language {
                    UiLanguage::English => format!(
                        "Saved {} bytes to {}",
                        report.workspace_bytes,
                        report.path.display()
                    ),
                    UiLanguage::SimplifiedChinese => format!(
                        "已保存 {} 字节至 {}",
                        report.workspace_bytes,
                        report.path.display()
                    ),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot save project: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法保存工程：{error}"),
                }
            }
        }
    }

    pub(crate) fn open_project(&mut self) {
        let path = self.project_path.trim();
        if path.is_empty() {
            self.status = self
                .language
                .text(
                    "Enter a project path before opening.",
                    "打开前请输入工程路径。",
                )
                .into();
            return;
        }
        let path = ensure_project_extension(path);
        if !self.checkpoint_recovery_now() {
            self.status = self
                .language
                .text(
                    "Cannot open another project because recovery checkpointing failed.",
                    "恢复检查点保存失败，无法打开其他工程。",
                )
                .into();
            return;
        }
        if self.offer_recovery(&path, true) {
            return;
        }
        self.load_primary_project(path);
    }

    pub(crate) fn load_primary_project(&mut self, path: String) {
        self.finish_recovery_job_blocking();
        match load_workspace(&path) {
            Ok(loaded) => {
                let migrated = loaded.migrated;
                self.project_path = path.clone();
                self.current_project_path = Some(PathBuf::from(path));
                self.install_workspace(loaded.workspace, migrated);
                self.recovery.reset(&self.workspace);
                self.status = if migrated {
                    self.language.text(
                        "Opened and migrated project. Save to update its native format.",
                        "工程已打开并迁移，请保存以更新其原生格式。",
                    )
                } else {
                    self.language
                        .text("Opened validated local project.", "已打开并验证本地工程。")
                }
                .into();
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot open project: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法打开工程：{error}"),
                }
            }
        }
    }

    pub(crate) fn install_workspace(&mut self, workspace: TaskWorkspace, dirty: bool) {
        self.workspace = workspace;
        self.sync_layer_state();
        self.active_task = self.workspace.tasks().keys().next_back().copied();
        self.selected_entity = None;
        self.comparison_base = None;
        self.comparison = None;
        self.constraint_diagnostics.clear();
        self.draw_origin = None;
        self.arc_points.clear();
        self.dimension_points.clear();
        self.orbit_camera = OrbitCamera::default();
        self.mechanical_scene = MechanicalScene::default();
        self.mechanical_scene_head = None;
        self.mechanical_scene_error = None;
        self.mechanical_gpu_scene = Arc::new(MechanicalGpuScene::default());
        self.mechanical_gpu_error = None;
        self.mechanical_fit_pending = true;
        self.clear_remote_context_review();
        self.is_dirty = dirty;
        self.next_branch_number = next_available_branch_number(&self.workspace);
    }

    pub(crate) fn compare_from_base(&mut self) {
        let Some(base) = self.comparison_base else {
            self.status = self
                .language
                .text(
                    "Choose a history commit as the comparison base.",
                    "请选择一个历史提交作为比较基准。",
                )
                .into();
            return;
        };
        let target = self.workspace.history().head();
        match self.workspace.history().compare(base, target) {
            Ok(comparison) => {
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Compared #{base} with #{target}: {}", comparison.summary())
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已比较 #{base} 与 #{target}：{}", comparison.summary())
                    }
                };
                self.comparison = Some(comparison);
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot compare history: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法比较历史：{error}"),
                }
            }
        }
    }

    pub(crate) fn fit_view(&mut self) {
        if self.viewport_mode == ViewportMode::Mechanical3d {
            self.refresh_mechanical_scene();
            if let Some(bounds) = self.mechanical_scene.bounds {
                self.orbit_camera
                    .fit_bounds(bounds, self.viewport_size, 0.12);
                self.mechanical_fit_pending = false;
                self.status = self
                    .language
                    .text(
                        "Fitted visible solids to the viewport.",
                        "已将可见实体适应到视口。",
                    )
                    .into();
            } else if let Some(error) = &self.mechanical_scene_error {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot fit mechanical viewport: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法适应机械视口：{error}"),
                };
            } else {
                self.status = self
                    .language
                    .text(
                        "No visible extrusion solids to fit.",
                        "没有可用于适应视口的可见拉伸实体。",
                    )
                    .into();
            }
            return;
        }
        let scene = RenderScene::from_document(self.workspace.document());
        if let Some(bounds) = scene.bounds {
            self.view_transform
                .fit_bounds(bounds, self.viewport_size, 0.12);
            self.status = self
                .language
                .text(
                    "Fitted visible geometry to the viewport.",
                    "已将可见几何图形适应到视口。",
                )
                .into();
        }
    }

    pub(crate) fn set_viewport_mode(&mut self, mode: ViewportMode) {
        if self.viewport_mode == mode {
            return;
        }
        self.viewport_mode = mode;
        self.draw_origin = None;
        self.arc_points.clear();
        self.dimension_points.clear();
        if mode == ViewportMode::Mechanical3d {
            self.mechanical_fit_pending = true;
        }
    }

    pub(crate) fn refresh_mechanical_scene(&mut self) {
        let head = self.workspace.history().head();
        if self.mechanical_scene_head == Some(head) {
            return;
        }
        match MechanicalScene::from_document(self.workspace.document()) {
            Ok(scene) => {
                match MechanicalGpuScene::from_scene(&scene) {
                    Ok(gpu_scene) => {
                        self.mechanical_gpu_scene = Arc::new(gpu_scene);
                        self.mechanical_gpu_error = None;
                    }
                    Err(error) => {
                        self.mechanical_gpu_scene = Arc::new(MechanicalGpuScene::default());
                        self.mechanical_gpu_error = Some(error.to_string());
                    }
                }
                self.mechanical_scene = scene;
                self.mechanical_scene_error = None;
            }
            Err(error) => {
                self.mechanical_scene = MechanicalScene::default();
                self.mechanical_scene_error = Some(error.to_string());
                self.mechanical_gpu_scene = Arc::new(MechanicalGpuScene::default());
                self.mechanical_gpu_error = None;
            }
        }
        self.mechanical_gpu_revision = self.mechanical_gpu_revision.wrapping_add(1);
        if self.mechanical_gpu_revision == 0 {
            self.mechanical_gpu_revision = 1;
        }
        self.mechanical_scene_head = Some(head);
    }

    pub(crate) fn commit_draw_gesture(&mut self, start: Point2, end: Point2) {
        let id = self.workspace.document().next_entity_id();
        let (intent, name, kind) = match self.viewport_tool {
            ViewportTool::Line => {
                if point_distance(start, end) < 0.001 {
                    self.status = self
                        .language
                        .text("Line needs two distinct points.", "直线需要两个不同的点。")
                        .into();
                    return;
                }
                (
                    "Draw editable line",
                    format!("Line {id}"),
                    EntityKind::Line { start, end },
                )
            }
            ViewportTool::Rectangle => {
                let width = (end.x - start.x).abs();
                let height = (end.y - start.y).abs();
                if width < 0.001 || height < 0.001 {
                    self.status = self
                        .language
                        .text(
                            "Rectangle needs positive width and height.",
                            "矩形的宽度和高度必须为正数。",
                        )
                        .into();
                    return;
                }
                (
                    "Draw editable rectangle",
                    format!("Rectangle {id}"),
                    EntityKind::Rectangle {
                        origin: Point2::new(start.x.min(end.x), start.y.min(end.y)),
                        width,
                        height,
                    },
                )
            }
            ViewportTool::Circle => {
                let radius = point_distance(start, end);
                if radius < 0.001 {
                    self.status = self
                        .language
                        .text("Circle needs a positive radius.", "圆的半径必须为正数。")
                        .into();
                    return;
                }
                (
                    "Draw editable circle",
                    format!("Circle {id}"),
                    EntityKind::Circle {
                        center: start,
                        radius,
                    },
                )
            }
            ViewportTool::Arc
            | ViewportTool::Dimension
            | ViewportTool::Select
            | ViewportTool::Pan => return,
        };
        self.commit_created_entity(id, intent, name, kind);
    }

    pub(crate) fn commit_three_point_arc(&mut self, start: Point2, through: Point2, end: Point2) {
        let Some(arc) = arc_from_three_points(start, through, end) else {
            self.status = self
                .language
                .text(
                    "Arc needs three distinct, non-collinear points.",
                    "圆弧需要三个不同且不共线的点。",
                )
                .into();
            return;
        };
        let id = self.workspace.document().next_entity_id();
        self.commit_created_entity(
            id,
            "Draw editable three-point arc",
            format!("Arc {id}"),
            EntityKind::Arc {
                center: arc.center,
                radius: arc.radius,
                start_angle: arc.start_angle,
                sweep_angle: arc.sweep_angle,
            },
        );
    }

    pub(crate) fn commit_aligned_dimension(
        &mut self,
        start: Point2,
        end: Point2,
        line_point: Point2,
    ) {
        if point_distance(start, end) < 0.001 {
            self.status = self
                .language
                .text(
                    "Dimension needs two distinct measured points.",
                    "标注需要两个不同的测量点。",
                )
                .into();
            return;
        }
        let Some(offset) = aligned_dimension_offset(start, end, line_point) else {
            self.status = self
                .language
                .text(
                    "Dimension line position must be finite.",
                    "尺寸线位置必须是有限值。",
                )
                .into();
            return;
        };
        if offset.abs() < 0.001 {
            self.status = self
                .language
                .text(
                    "Dimension line must be offset from the measured points.",
                    "尺寸线必须与测量点保持偏移。",
                )
                .into();
            return;
        }
        let id = self.workspace.document().next_entity_id();
        self.commit_created_entity(
            id,
            "Draw editable aligned dimension",
            format!("Dimension {id}"),
            EntityKind::AlignedDimension {
                start,
                end,
                offset,
                text_override: None,
            },
        );
    }

    fn commit_created_entity(
        &mut self,
        id: u64,
        intent: &'static str,
        name: String,
        kind: EntityKind,
    ) {
        let Some(layer) = self.workspace.document().layers.get(&self.active_layer) else {
            self.sync_layer_state();
            self.status = self
                .language
                .text(
                    "Select an available layer before drawing.",
                    "绘图前请选择可用图层。",
                )
                .into();
            return;
        };
        if !layer.visible {
            self.status = match self.language {
                UiLanguage::English => format!("Show layer {} before drawing on it.", layer.name),
                UiLanguage::SimplifiedChinese => {
                    format!("在图层 {} 上绘图前请先将其显示。", layer.name)
                }
            };
            return;
        }
        if layer.locked {
            self.status = match self.language {
                UiLanguage::English => {
                    format!("Unlock layer {} before drawing on it.", layer.name)
                }
                UiLanguage::SimplifiedChinese => {
                    format!("在图层 {} 上绘图前请先将其解锁。", layer.name)
                }
            };
            return;
        }
        let entity = Entity {
            id,
            layer: self.active_layer,
            name,
            visible: true,
            kind,
            parameter_refs: Default::default(),
        };
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            intent,
            CommandTransaction::new(vec![CadCommand::CreateEntity { entity }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.selected_entity = Some(id);
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Saved user edit as semantic commit #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已将用户编辑保存为语义提交 #{commit_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot save drawing: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法保存绘图：{error}"),
                }
            }
        }
    }

    pub(crate) fn delete_selected_entity(&mut self) {
        let Some(id) = self.selected_entity else {
            return;
        };
        let Some(entity) = self.workspace.document().entities.get(&id) else {
            self.selected_entity = None;
            return;
        };
        let name = entity.name.clone();
        let expected_revision = self.workspace.revision();
        match self.workspace.kernel().apply_user_transaction(
            expected_revision,
            "Delete editable entity",
            CommandTransaction::new(vec![CadCommand::DeleteEntity { id }]),
            ValidationReport::default(),
        ) {
            Ok(commit_id) => {
                self.selected_entity = None;
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Deleted {name} in semantic commit #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已在语义提交 #{commit_id} 中删除 {name}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot delete {name}: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法删除 {name}：{error}"),
                }
            }
        }
    }

    pub(crate) fn undo_latest_change(&mut self) {
        let undone_head = self.workspace.history().head();
        let undone_intent = self
            .workspace
            .history()
            .commits
            .get(&undone_head)
            .map(|commit| commit.intent.clone())
            .unwrap_or_else(|| "change".into());
        match self.workspace.kernel().undo() {
            Ok(commit_id) => {
                self.selected_entity = self
                    .selected_entity
                    .filter(|id| self.workspace.document().entities.contains_key(id));
                self.after_history_navigation();
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Undid {undone_intent}; branch restored to #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已撤销 {undone_intent}；分支已恢复到 #{commit_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot undo: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法撤销：{error}"),
                }
            }
        }
    }

    pub(crate) fn redo_latest_change(&mut self) {
        match self.workspace.kernel().redo() {
            Ok(commit_id) => {
                let intent = self.workspace.history().commits[&commit_id].intent.clone();
                self.after_history_navigation();
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Redid {intent} as branch head #{commit_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已重做 {intent}，分支头为 #{commit_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot redo: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法重做：{error}"),
                }
            }
        }
    }

    fn after_history_navigation(&mut self) {
        self.sync_layer_state();
        self.draw_origin = None;
        self.arc_points.clear();
        self.dimension_points.clear();
        self.comparison = None;
        self.constraint_diagnostics.clear();
        self.clear_remote_context_review();
        self.is_dirty = true;
    }

    pub(crate) fn handle_shortcuts(&mut self, context: &egui::Context) {
        if context.wants_keyboard_input() {
            return;
        }
        let (undo, redo, delete, escape) = context.input(|input| {
            let redo = input.modifiers.command
                && (input.key_pressed(egui::Key::Y)
                    || (input.modifiers.shift && input.key_pressed(egui::Key::Z)));
            let undo = input.modifiers.command
                && !input.modifiers.shift
                && input.key_pressed(egui::Key::Z);
            let delete =
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace);
            let escape = input.key_pressed(egui::Key::Escape);
            (undo, redo, delete, escape)
        });
        if escape {
            self.draw_origin = None;
            self.arc_points.clear();
            self.dimension_points.clear();
        } else if redo {
            self.redo_latest_change();
        } else if undo {
            self.undo_latest_change();
        } else if delete && self.selected_entity.is_some() {
            self.delete_selected_entity();
        }
    }

    pub(crate) fn select_active_task(&mut self, task_id: TaskId) {
        if self.active_task != Some(task_id) {
            self.active_task = Some(task_id);
            if let Some(prompt) = self
                .workspace
                .task(task_id)
                .and_then(cadx_core::DesignTask::active_prompt)
            {
                self.task_goal = prompt.to_owned();
            }
            self.clear_remote_context_review();
        }
    }

    pub(crate) fn prepare_remote_disclosure(&mut self) {
        let Some(task_id) = self.ensure_active_task() else {
            return;
        };
        let agent = match self.remote_agent() {
            Ok(agent) => agent,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot prepare remote context: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法准备远程上下文：{error}")
                    }
                };
                return;
            }
        };
        match agent.remote_disclosure(&self.workspace, task_id) {
            Ok(disclosure) => {
                self.remote_grant_id = unix_time_now().ok().and_then(|unix_seconds| {
                    agent.matching_remote_access_grant(&self.workspace, &disclosure, unix_seconds)
                });
                self.remote_disclosure = Some(disclosure);
                self.status = match self.language {
                    UiLanguage::English if self.remote_grant_id.is_some() => {
                        format!("Remote context for task {task_id} is covered by a project grant")
                    }
                    UiLanguage::SimplifiedChinese if self.remote_grant_id.is_some() => {
                        format!("任务 {task_id} 的远程上下文已由项目授权覆盖")
                    }
                    UiLanguage::English => {
                        format!("Remote context for task {task_id} is ready for project approval")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("任务 {task_id} 的远程上下文已可供项目授权")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot prepare remote context: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法准备远程上下文：{error}")
                    }
                };
            }
        }
    }

    pub(crate) fn create_project_remote_grant(&mut self) {
        let Some(task_id) = self.active_task else {
            self.status = self
                .language
                .text(
                    "Create or select a task before approving remote context.",
                    "批准远程上下文前请创建或选择任务。",
                )
                .into();
            return;
        };
        let Some(disclosure) = self.remote_disclosure.clone() else {
            self.status = self
                .language
                .text(
                    "Review remote context before approving it.",
                    "批准前请先审查远程上下文。",
                )
                .into();
            return;
        };
        let now = match unix_time_now() {
            Ok(now) => now,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot create project grant: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法创建项目授权：{error}"),
                };
                return;
            }
        };
        let expires_at = if self.remote_grant_duration_seconds == 0 {
            None
        } else {
            match now.checked_add(self.remote_grant_duration_seconds) {
                Some(expires_at) => Some(expires_at),
                None => {
                    self.status = self
                        .language
                        .text("Project grant expiry is invalid.", "项目授权有效期无效。")
                        .into();
                    return;
                }
            }
        };
        let agent = match self.remote_agent() {
            Ok(agent) => agent,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot create project grant: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法创建项目授权：{error}"),
                };
                return;
            }
        };
        match agent.create_remote_access_grant(
            &mut self.workspace,
            task_id,
            &disclosure,
            now,
            expires_at,
        ) {
            Ok(grant_id) => {
                self.remote_grant_id = Some(grant_id);
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Created project remote-access grant #{grant_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("已创建项目远程访问授权 #{grant_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot create project grant: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法创建项目授权：{error}"),
                };
            }
        }
    }

    pub(crate) fn revoke_remote_access_grant(&mut self) {
        let Some(grant_id) = self.remote_grant_id else {
            return;
        };
        let now = match unix_time_now() {
            Ok(now) => now,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot revoke project grant: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法撤销项目授权：{error}"),
                };
                return;
            }
        };
        match self
            .workspace
            .kernel()
            .revoke_remote_access_grant(grant_id, now)
        {
            Ok(()) => {
                self.remote_grant_id = None;
                self.is_dirty = true;
                self.status = match self.language {
                    UiLanguage::English => format!("Revoked project grant #{grant_id}"),
                    UiLanguage::SimplifiedChinese => format!("已撤销项目授权 #{grant_id}"),
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot revoke project grant: {error}"),
                    UiLanguage::SimplifiedChinese => format!("无法撤销项目授权：{error}"),
                };
            }
        }
    }

    fn ensure_active_task(&mut self) -> Option<TaskId> {
        if self.active_task.is_none() {
            self.create_task();
        }
        self.active_task
    }

    pub(crate) fn clear_remote_context_review(&mut self) {
        self.remote_disclosure = None;
        self.remote_grant_id = None;
    }

    fn remote_agent(&self) -> Result<TaskAgent<GenAiRemotePlanner>, AgentError> {
        let settings = CadxConfig::load_default().map_err(|error| {
            AgentError::Provider(format!("cannot load provider configuration: {error}"))
        })?;
        let planner = GenAiRemotePlanner::new(
            ProviderConfig {
                endpoint: settings.provider.endpoint.clone(),
                model: settings.provider.model.clone(),
                enabled_capabilities: BTreeSet::from([
                    cadx_core::Capability::Drafting,
                    cadx_core::Capability::Mechanical,
                    cadx_core::Capability::Architecture,
                    cadx_core::Capability::Parameters,
                ]),
            },
            settings.provider.api_key(),
        )?
        .with_timeout(settings.provider.timeout())?
        .with_selected_entity_ids(self.selected_entity);
        Ok(TaskAgent::new(planner))
    }

    fn run_active_remote_task(&mut self, max_actions_per_run: usize) {
        if self.remote_agent_job.is_some() {
            self.status = self
                .language
                .text(
                    "A remote planner request is already running.",
                    "已有远程规划器请求正在运行。",
                )
                .into();
            return;
        }
        let Some(task_id) = self.ensure_active_task() else {
            return;
        };
        let task_status = self.workspace.task(task_id).map(|task| task.status);
        if matches!(task_status, Some(TaskStatus::Paused | TaskStatus::Running)) {
            let result = self
                .planner
                .run_with_action_budget(&mut self.workspace, task_id, Some(max_actions_per_run))
                .map_err(|error| error.to_string());
            self.is_dirty = true;
            self.finish_remote_agent_result(task_id, result);
            return;
        }
        let Some(grant_id) = self.remote_grant_id else {
            self.status = self
                .language
                .text(
                    "Review the remote context and create a project grant before running the remote planner.",
                    "运行远程规划器前请审查远程上下文并创建项目授权。",
                )
                .into();
            return;
        };
        let agent = match self.remote_agent() {
            Ok(agent) => agent,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot run remote planner: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法运行远程规划器：{error}")
                    }
                };
                return;
            }
        };
        let send_time = match unix_time_now() {
            Ok(send_time) => send_time,
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot run remote planner: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法运行远程规划器：{error}")
                    }
                };
                return;
            }
        };
        let disclosure =
            match agent.validate_remote_access_grant(&self.workspace, task_id, grant_id, send_time)
            {
                Ok(disclosure) => disclosure,
                Err(error) => {
                    self.status = match self.language {
                        UiLanguage::English => {
                            format!("Project remote-access grant is no longer valid: {error}")
                        }
                        UiLanguage::SimplifiedChinese => {
                            format!("项目远程访问授权已失效：{error}")
                        }
                    };
                    self.clear_remote_context_review();
                    return;
                }
            };
        let budget = ExecutionBudget {
            max_planned_actions: ExecutionBudget::default().max_planned_actions,
            max_actions_per_run,
        };
        let mut worker_workspace = self.workspace.clone();
        let (sender, receiver) = mpsc::channel();
        let (start_sender, start_receiver) = mpsc::sync_channel(0);
        let spawn = thread::Builder::new()
            .name(format!("cadx-remote-task-{task_id}"))
            .spawn(move || {
                if start_receiver.recv().is_err() {
                    return;
                }
                let result = agent
                    .run_remote_with_grant(
                        &mut worker_workspace,
                        task_id,
                        grant_id,
                        send_time,
                        budget,
                    )
                    .map_err(|error| error.to_string());
                let _ = sender.send(RemoteAgentOutput {
                    workspace: worker_workspace,
                    result,
                });
            });
        match spawn {
            Ok(handle) => {
                let identity = AgentRunIdentity::remote(
                    "cadx-agent.remote-planner",
                    disclosure.config.endpoint.clone(),
                    disclosure.config.model.clone(),
                );
                let audit_result = self
                    .workspace
                    .kernel()
                    .begin_task_as(task_id, identity)
                    .and_then(|()| {
                        self.workspace.kernel().record_event(
                            task_id,
                            disclosure.granted_audit_event(grant_id, send_time),
                        )
                    });
                if let Err(error) = audit_result {
                    drop(start_sender);
                    let _ = handle.join();
                    self.status = match self.language {
                        UiLanguage::English => {
                            format!("Cannot record remote-send audit for task {task_id}: {error}")
                        }
                        UiLanguage::SimplifiedChinese => {
                            format!("无法记录任务 {task_id} 的远程发送审计：{error}")
                        }
                    };
                    return;
                }
                self.is_dirty = true;
                let base_workspace = self.workspace.clone();
                if start_sender.send(()).is_err() {
                    let _ = handle.join();
                    let message =
                        "remote planner worker stopped before the audited request could start";
                    let _ = self.workspace.kernel().fail_task(task_id, message);
                    self.status = match self.language {
                        UiLanguage::English => format!("Remote task {task_id} stopped: {message}"),
                        UiLanguage::SimplifiedChinese => {
                            format!("远程任务 {task_id} 在已审计请求启动前停止")
                        }
                    };
                    return;
                }
                self.clear_remote_context_review();
                self.remote_agent_job = Some(RemoteAgentJob {
                    task_id,
                    base_workspace,
                    receiver,
                    handle,
                });
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Remote planner is working on task {task_id}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("远程规划器正在处理任务 {task_id}")
                    }
                };
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => {
                        format!("Cannot start remote planner worker: {error}")
                    }
                    UiLanguage::SimplifiedChinese => {
                        format!("无法启动远程规划器工作线程：{error}")
                    }
                };
            }
        }
    }

    pub(crate) fn remote_agent_running(&self) -> bool {
        self.remote_agent_job.is_some()
    }

    fn update_remote_agent(&mut self, context: &egui::Context) {
        let received = match self
            .remote_agent_job
            .as_ref()
            .map(|job| job.receiver.try_recv())
        {
            Some(Ok(output)) => Some(Ok(output)),
            Some(Err(TryRecvError::Disconnected)) => Some(Err(
                "remote planner worker stopped before reporting a result".to_string(),
            )),
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        let Some(received) = received else {
            if self.remote_agent_job.is_some() {
                context.request_repaint_after(REMOTE_AGENT_POLL_INTERVAL);
            }
            return;
        };
        let Some(job) = self.remote_agent_job.take() else {
            return;
        };
        let joined = job.handle.join().is_ok();
        match received {
            Ok(output) => {
                self.finish_remote_agent_output(job.task_id, job.base_workspace, output, joined)
            }
            Err(error) => {
                let failure = if joined {
                    error.clone()
                } else {
                    "remote planner worker stopped unexpectedly".into()
                };
                self.fail_unchanged_remote_task(job.task_id, &job.base_workspace, &failure);
                self.status = match (self.language, joined) {
                    (UiLanguage::English, true) => {
                        format!("Remote task {} stopped: {error}", job.task_id)
                    }
                    (UiLanguage::English, false) => {
                        format!("Remote task {} worker stopped unexpectedly", job.task_id)
                    }
                    (UiLanguage::SimplifiedChinese, true) => {
                        format!("远程任务 {} 已停止：{error}", job.task_id)
                    }
                    (UiLanguage::SimplifiedChinese, false) => {
                        format!("远程任务 {} 的工作线程意外停止", job.task_id)
                    }
                };
            }
        }
    }

    fn finish_remote_agent_output(
        &mut self,
        task_id: TaskId,
        base_workspace: TaskWorkspace,
        output: RemoteAgentOutput,
        worker_joined: bool,
    ) {
        if !worker_joined {
            self.fail_unchanged_remote_task(
                task_id,
                &base_workspace,
                "remote planner worker stopped unexpectedly",
            );
            self.status = match self.language {
                UiLanguage::English => {
                    format!("Remote task {task_id} worker stopped unexpectedly")
                }
                UiLanguage::SimplifiedChinese => {
                    format!("远程任务 {task_id} 的工作线程意外停止")
                }
            };
            return;
        }
        self.clear_remote_context_review();
        if output.workspace == base_workspace {
            self.finish_remote_agent_result(task_id, output.result);
            return;
        }

        let base_revision = base_workspace.revision();
        let base_state_matches = self
            .workspace
            .history()
            .is_ancestor(base_revision, self.workspace.revision())
            .is_ok_and(|is_ancestor| is_ancestor)
            && self
                .workspace
                .history()
                .restore(base_revision)
                .is_ok_and(|document| document == *base_workspace.document());
        let task_unchanged = self.workspace.task(task_id) == base_workspace.task(task_id);
        if !base_state_matches || !task_unchanged {
            self.discard_stale_remote_output(task_id);
            return;
        }

        let Some(worker_task) = output.workspace.task(task_id).cloned() else {
            self.discard_stale_remote_output(task_id);
            return;
        };
        let base_event_count = base_workspace
            .task(task_id)
            .map_or(0, |task| task.events().len());
        let disclosure_events = worker_task
            .events()
            .iter()
            .skip(base_event_count)
            .filter(|event| matches!(event, cadx_core::TaskEvent::ProviderDisclosure { .. }))
            .cloned()
            .collect::<Vec<_>>();
        let base_status = base_workspace
            .task(task_id)
            .map(|task| task.status)
            .unwrap_or(TaskStatus::Failed);
        if base_status == TaskStatus::Queued {
            let identity = worker_task
                .active_run()
                .map(|run| run.identity.clone())
                .unwrap_or_else(|| AgentRunIdentity::local("recovered-worker"));
            if let Err(error) = self.workspace.kernel().begin_task_as(task_id, identity) {
                self.finish_remote_agent_result(task_id, Err(error.to_string()));
                return;
            }
            self.is_dirty = true;
            for event in disclosure_events {
                if let Err(error) = self.workspace.kernel().record_event(task_id, event) {
                    self.finish_remote_agent_result(task_id, Err(error.to_string()));
                    return;
                }
            }
        } else if base_status != TaskStatus::Paused && base_status != TaskStatus::Running {
            self.discard_stale_remote_output(task_id);
            return;
        }

        let current_has_plan = self
            .workspace
            .task(task_id)
            .and_then(cadx_core::DesignTask::execution)
            .is_some();
        if !current_has_plan {
            let Some(execution) = worker_task.execution() else {
                let message = output
                    .result
                    .err()
                    .unwrap_or_else(|| "remote planner returned no durable action plan".into());
                let _ = self.workspace.kernel().fail_task(task_id, message.clone());
                self.is_dirty = true;
                self.finish_remote_agent_result(task_id, Err(message));
                return;
            };
            let Some(planning_revision) = execution.base_revision() else {
                let message = "remote planner returned an unbound action plan".to_string();
                let _ = self.workspace.kernel().fail_task(task_id, message.clone());
                self.is_dirty = true;
                self.finish_remote_agent_result(task_id, Err(message));
                return;
            };
            if let Err(error) = self.workspace.kernel().set_task_plan(
                task_id,
                planning_revision,
                execution.actions().to_vec(),
            ) {
                self.is_dirty = true;
                self.finish_remote_agent_result(task_id, Err(error.to_string()));
                return;
            }
        }

        let action_budget = output.result.as_ref().ok().and_then(|report| {
            (report.status == TaskStatus::Paused).then_some(report.commit_ids.len())
        });
        let result = self
            .planner
            .run_with_action_budget(&mut self.workspace, task_id, action_budget)
            .map_err(|error| error.to_string());
        self.is_dirty = true;
        self.finish_remote_agent_result(task_id, result);
    }

    fn fail_unchanged_remote_task(
        &mut self,
        task_id: TaskId,
        base_workspace: &TaskWorkspace,
        message: &str,
    ) {
        if self.workspace.task(task_id) != base_workspace.task(task_id) {
            return;
        }
        if self.workspace.kernel().fail_task(task_id, message).is_ok() {
            self.is_dirty = true;
        }
    }

    fn finish_remote_agent_result(
        &mut self,
        task_id: TaskId,
        result: Result<AgentRunReport, String>,
    ) {
        match result {
            Ok(report) => {
                self.status = task_run_status(
                    self.language,
                    true,
                    &report,
                    &self.workspace.history().active_branch,
                );
            }
            Err(error) => {
                self.status = match self.language {
                    UiLanguage::English => format!("Remote task {task_id} stopped: {error}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("远程任务 {task_id} 已停止：{error}")
                    }
                };
            }
        }
    }

    fn discard_stale_remote_output(&mut self, task_id: TaskId) {
        self.status = match self.language {
            UiLanguage::English => format!(
                "Discarded stale remote task {task_id} result because its base or task state changed while it was running"
            ),
            UiLanguage::SimplifiedChinese => {
                format!("远程任务 {task_id} 运行期间基线或任务状态已更改，已丢弃过期结果")
            }
        };
    }
}

fn task_run_status(
    language: UiLanguage,
    remote: bool,
    report: &AgentRunReport,
    branch: &str,
) -> String {
    match (language, remote, report.status) {
        (UiLanguage::English, false, TaskStatus::Paused) => format!(
            "Task {} paused after {} saved action(s)",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::English, false, TaskStatus::Completed) => format!(
            "Task {} saved {} semantic commit(s) on {branch}",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::English, true, TaskStatus::Paused) => format!(
            "Remote task {} paused after {} saved action(s)",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::English, true, TaskStatus::Completed) => format!(
            "Remote task {} saved {} semantic commit(s) on {branch}",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::English, false, _) => format!("Task {} changed status", report.task_id),
        (UiLanguage::English, true, _) => {
            format!("Remote task {} changed status", report.task_id)
        }
        (UiLanguage::SimplifiedChinese, false, TaskStatus::Paused) => format!(
            "任务 {} 在保存 {} 个动作后暂停",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::SimplifiedChinese, false, TaskStatus::Completed) => format!(
            "任务 {} 已在 {branch} 上保存 {} 个语义提交",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::SimplifiedChinese, true, TaskStatus::Paused) => format!(
            "远程任务 {} 在保存 {} 个动作后暂停",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::SimplifiedChinese, true, TaskStatus::Completed) => format!(
            "远程任务 {} 已在 {branch} 上保存 {} 个语义提交",
            report.task_id,
            report.commit_ids.len()
        ),
        (UiLanguage::SimplifiedChinese, false, _) => {
            format!("任务 {} 状态已更改", report.task_id)
        }
        (UiLanguage::SimplifiedChinese, true, _) => {
            format!("远程任务 {} 状态已更改", report.task_id)
        }
    }
}

pub(crate) fn ensure_project_extension(path: &str) -> String {
    let project_path = std::path::Path::new(path);
    if project_path.extension().is_none() {
        format!("{path}.{PROJECT_EXTENSION}")
    } else {
        path.into()
    }
}

fn display_default_config_path() -> String {
    default_config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "~/.cadx/config.yaml".into())
}

pub(crate) fn unix_time_now() -> Result<u64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "system clock is before the Unix epoch")
}

fn next_available_branch_number(workspace: &TaskWorkspace) -> u64 {
    let mut number = 1;
    while workspace
        .history()
        .branches
        .contains_key(&format!("option-{number}"))
    {
        number += 1;
    }
    number
}

impl eframe::App for CadxApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_remote_agent(context);
        self.update_recovery(context);
        if self.recovery.decision_pending() {
            self.ui_status_bar(context);
            egui::CentralPanel::default().show(context, |_ui| {});
            self.ui_recovery_dialog(context);
            return;
        }
        self.handle_shortcuts(context);
        self.ui_top_bar(context);
        self.ui_status_bar(context);
        self.ui_task_panel(context);
        self.ui_model_panel(context);
        self.ui_viewport(context);
        self.ui_layer_delete_dialog(context);
        self.ui_recovery_dialog(context);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush_recovery_on_exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drafting_line_action(id: u64, y: f64) -> cadx_core::TaskAction {
        cadx_core::TaskAction {
            intent: format!("Create line {id}"),
            tool_name: "drafting.create_line".into(),
            detail: format!("Create editable line {id}."),
            transaction: CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: Entity {
                    id,
                    layer: 1,
                    name: format!("Line {id}"),
                    visible: true,
                    kind: EntityKind::Line {
                        start: Point2::new(0.0, y),
                        end: Point2::new(10.0, y),
                    },
                    parameter_refs: Default::default(),
                },
            }]),
            validation: ValidationReport::default(),
        }
    }

    fn provider_disclosure_event(source_revision: u64) -> cadx_core::TaskEvent {
        cadx_core::TaskEvent::ProviderDisclosure {
            endpoint: "https://provider.example/v1".into(),
            model: "recorded-model".into(),
            project_id: None,
            grant_id: None,
            sent_at_unix_seconds: None,
            requested_capabilities: BTreeSet::from([cadx_core::Capability::Drafting]),
            selected_entity_ids: Vec::new(),
            includes_source_files: false,
            payload_summary: "Task goal and bounded document statistics.".into(),
            context_schema_version: cadx_core::REMOTE_CONTEXT_SCHEMA_VERSION,
            source_revision,
            data_categories: BTreeSet::from([
                cadx_core::RemoteDataCategory::TaskGoal,
                cadx_core::RemoteDataCategory::DocumentStatistics,
            ]),
            payload_bytes: 128,
            payload_hash: "0".repeat(64),
        }
    }

    #[test]
    fn workbench_adds_prompts_and_retries_runs_inside_the_selected_task() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        app.workspace.kernel().begin_task(task_id).unwrap();
        let revision = app.workspace.revision();
        app.workspace
            .kernel()
            .set_task_plan(task_id, revision, Vec::new())
            .unwrap();
        app.workspace.kernel().complete_task(task_id).unwrap();

        app.task_goal = "Add a second bracket feature".into();
        app.add_prompt_to_active_task();
        assert_eq!(app.workspace.task(task_id).unwrap().change_sets.len(), 2);
        app.workspace.kernel().begin_task(task_id).unwrap();
        app.workspace
            .kernel()
            .fail_task(task_id, "Planner failed")
            .unwrap();
        app.retry_active_change_set();

        let task = app.workspace.task(task_id).unwrap();
        assert_eq!(task.change_sets.len(), 2);
        assert_eq!(task.active_change_set().unwrap().runs.len(), 2);
        assert_eq!(task.status, TaskStatus::Queued);
        assert!(app.status.contains("queued for retry"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn workbench_compensating_revert_updates_model_and_chinese_status() {
        let mut app = CadxApp {
            language: UiLanguage::SimplifiedChinese,
            ..Default::default()
        };
        let task_id = app.workspace.kernel().create_task(
            "Create line",
            "Create one line",
            TaskAuthority::all_direct(),
        );
        app.active_task = Some(task_id);
        let change_set_id = app.workspace.task(task_id).unwrap().active_change_set_id;
        app.workspace.kernel().begin_task(task_id).unwrap();
        let revision = app.workspace.revision();
        app.workspace
            .kernel()
            .set_task_plan(task_id, revision, vec![drafting_line_action(1, 0.0)])
            .unwrap();
        app.workspace
            .kernel()
            .apply_next_task_action(task_id)
            .unwrap();
        app.workspace.kernel().complete_task(task_id).unwrap();

        app.revert_change_set(change_set_id);

        assert!(app.workspace.document().entities.is_empty());
        assert!(app.status.contains("已回滚变更集"));
        assert!(app.is_dirty);
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn parametric_workbench_actions_commit_through_history() {
        let mut app = CadxApp {
            parameter_name: "target_length".into(),
            parameter_value: 40.0,
            ..Default::default()
        };
        app.save_parameter();
        assert_eq!(app.workspace.document().parameters.len(), 1);

        app.viewport_tool = ViewportTool::Line;
        app.commit_draw_gesture(Point2::new(0.0, 1.0), Point2::new(12.0, 6.0));
        app.add_orientation_constraint(false);
        assert_eq!(app.workspace.document().constraints.len(), 1);

        app.solve_active_constraints();

        let EntityKind::Line { start, end } = &app.workspace.document().entities[&1].kind else {
            panic!("expected a line created by the viewport tool");
        };
        assert!((start.y - end.y).abs() < 1e-7);
        assert!(app.workspace.history().commits.len() >= 5);
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn workbench_undo_and_redo_restore_selection_and_history_state() {
        let mut app = CadxApp {
            viewport_tool: ViewportTool::Line,
            ..Default::default()
        };
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        assert_eq!(app.selected_entity, Some(1));

        app.undo_latest_change();
        assert!(app.workspace.document().entities.is_empty());
        assert_eq!(app.selected_entity, None);
        assert!(app.workspace.can_redo());

        app.redo_latest_change();
        assert!(app.workspace.document().entities.contains_key(&1));
        assert!(!app.workspace.can_redo());
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn workbench_three_point_arc_commits_exact_geometry_and_rejects_collinearity() {
        let mut app = CadxApp {
            viewport_tool: ViewportTool::Arc,
            ..Default::default()
        };
        let before = app.workspace.history().commits.len();

        app.commit_three_point_arc(
            Point2::new(10.0, 0.0),
            Point2::new(0.0, 10.0),
            Point2::new(-10.0, 0.0),
        );

        assert_eq!(app.workspace.history().commits.len(), before + 1);
        let EntityKind::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } = app.workspace.document().entities[&1].kind
        else {
            panic!("expected a three-point arc");
        };
        assert!(center.x.abs() < 1.0e-9);
        assert!(center.y.abs() < 1.0e-9);
        assert!((radius - 10.0).abs() < 1.0e-9);
        assert!(start_angle.abs() < 1.0e-9);
        assert!((sweep_angle - std::f64::consts::PI).abs() < 1.0e-9);

        let head = app.workspace.history().head();
        app.commit_three_point_arc(
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(2.0, 0.0),
        );
        assert_eq!(app.workspace.history().head(), head);
        assert_eq!(app.workspace.document().entities.len(), 1);

        let major = arc_from_three_points(
            Point2::new(1.0, 0.0),
            Point2::new(0.0, -1.0),
            Point2::new(0.0, 1.0),
        )
        .unwrap();
        assert!((major.sweep_angle - 1.5 * std::f64::consts::PI).abs() < 1.0e-9);
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn workbench_three_point_dimension_commits_signed_offset_and_rejects_degeneracy() {
        let mut app = CadxApp {
            viewport_tool: ViewportTool::Dimension,
            ..Default::default()
        };
        let before = app.workspace.history().commits.len();

        app.commit_aligned_dimension(
            Point2::new(0.0, 0.0),
            Point2::new(30.0, 0.0),
            Point2::new(8.0, -12.0),
        );

        assert_eq!(app.workspace.history().commits.len(), before + 1);
        let EntityKind::AlignedDimension {
            start,
            end,
            offset,
            text_override,
        } = &app.workspace.document().entities[&1].kind
        else {
            panic!("expected an aligned dimension");
        };
        assert_eq!(*start, Point2::new(0.0, 0.0));
        assert_eq!(*end, Point2::new(30.0, 0.0));
        assert!((*offset + 12.0).abs() < 1.0e-9);
        assert_eq!(text_override, &None);

        let head = app.workspace.history().head();
        app.commit_aligned_dimension(
            Point2::new(1.0, 1.0),
            Point2::new(1.0, 1.0),
            Point2::new(2.0, 2.0),
        );
        app.commit_aligned_dimension(
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(5.0, 0.0),
        );
        assert_eq!(app.workspace.history().head(), head);
        assert_eq!(app.workspace.document().entities.len(), 1);
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn mechanical_scene_cache_tracks_history_and_workspace_installation() {
        let mut app = CadxApp::default();
        let profile = Entity {
            id: 1,
            layer: 1,
            name: "Bracket profile".into(),
            visible: true,
            kind: EntityKind::SketchProfile {
                points: vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(20.0, 0.0),
                    Point2::new(20.0, 12.0),
                    Point2::new(0.0, 12.0),
                ],
                closed: true,
            },
            parameter_refs: Default::default(),
        };
        let extrusion = Entity {
            id: 2,
            layer: 1,
            name: "Bracket solid".into(),
            visible: true,
            kind: EntityKind::Extrude {
                profile: 1,
                distance: 5.0,
            },
            parameter_refs: Default::default(),
        };
        let expected_revision = app.workspace.revision();
        app.workspace
            .kernel()
            .apply_user_transaction(
                expected_revision,
                "Create test solid",
                CommandTransaction::new(vec![
                    CadCommand::CreateEntity { entity: profile },
                    CadCommand::CreateEntity { entity: extrusion },
                ]),
                ValidationReport::default(),
            )
            .unwrap();

        app.refresh_mechanical_scene();
        let first_head = app.workspace.history().head();
        let first_gpu_revision = app.mechanical_gpu_revision;
        assert_eq!(app.mechanical_scene_head, Some(first_head));
        assert_eq!(app.mechanical_scene.items.len(), 1);
        assert_eq!(app.mechanical_scene.bounds.unwrap().max.z, 5.0);
        assert_eq!(app.mechanical_gpu_scene.counts(), (8, 36, 24));

        app.mechanical_scene.items.clear();
        app.refresh_mechanical_scene();
        assert!(app.mechanical_scene.items.is_empty());
        assert_eq!(app.mechanical_gpu_revision, first_gpu_revision);

        let mut updated = app.workspace.document().entities[&2].clone();
        updated.kind = EntityKind::Extrude {
            profile: 1,
            distance: 9.0,
        };
        let expected_revision = app.workspace.revision();
        app.workspace
            .kernel()
            .apply_user_transaction(
                expected_revision,
                "Increase extrusion",
                CommandTransaction::new(vec![CadCommand::UpdateEntity { entity: updated }]),
                ValidationReport::default(),
            )
            .unwrap();
        app.refresh_mechanical_scene();
        assert_ne!(app.mechanical_scene_head, Some(first_head));
        assert_ne!(app.mechanical_gpu_revision, first_gpu_revision);
        assert_eq!(app.mechanical_scene.items.len(), 1);
        assert_eq!(app.mechanical_scene.bounds.unwrap().max.z, 9.0);

        app.install_workspace(
            TaskWorkspace::new(CadDocument::new("Replacement workspace")),
            false,
        );
        assert_eq!(app.mechanical_scene_head, None);
        assert!(app.mechanical_scene.items.is_empty());
        assert!(app.mechanical_fit_pending);
        let installed_revision = app.mechanical_gpu_revision;
        app.refresh_mechanical_scene();
        assert_ne!(app.mechanical_gpu_revision, installed_revision);
        assert_eq!(app.mechanical_gpu_scene.counts(), (0, 0, 0));
    }

    #[test]
    fn current_remote_agent_output_is_replayed_without_installing_other_worker_state() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let base_workspace = app.workspace.clone();
        let mut worker_workspace = base_workspace.clone();
        let report = TaskAgent::new(HeuristicPlanner)
            .run(&mut worker_workspace, task_id)
            .unwrap();
        worker_workspace.kernel().create_task(
            "Worker-only task",
            "This task must not escape the worker clone",
            TaskAuthority::ReviewOnly,
        );

        app.finish_remote_agent_output(
            task_id,
            base_workspace,
            RemoteAgentOutput {
                workspace: worker_workspace,
                result: Ok(report),
            },
            true,
        );

        assert_eq!(
            app.workspace.tasks()[&task_id].status,
            TaskStatus::Completed
        );
        assert_eq!(app.workspace.document().entities.len(), 3);
        assert_eq!(app.workspace.tasks().len(), 1);
        assert!(app.status.contains("saved 1 semantic commit"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn audited_remote_dispatch_replays_the_worker_plan_without_duplicate_disclosure() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let mut worker_workspace = app.workspace.clone();
        let report = TaskAgent::new(HeuristicPlanner)
            .run(&mut worker_workspace, task_id)
            .unwrap();

        let source_revision = app.workspace.revision();
        app.workspace.kernel().begin_task(task_id).unwrap();
        app.workspace
            .kernel()
            .record_event(task_id, provider_disclosure_event(source_revision))
            .unwrap();
        let audited_base = app.workspace.clone();

        app.finish_remote_agent_output(
            task_id,
            audited_base,
            RemoteAgentOutput {
                workspace: worker_workspace,
                result: Ok(report),
            },
            true,
        );

        let task = &app.workspace.tasks()[&task_id];
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(app.workspace.document().entities.len(), 3);
        assert_eq!(
            task.events()
                .iter()
                .filter(|event| matches!(event, cadx_core::TaskEvent::ProviderDisclosure { .. }))
                .count(),
            1
        );
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn remote_worker_error_without_workspace_effects_keeps_the_task_queued() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let base_workspace = app.workspace.clone();
        let dirty_before = app.is_dirty;

        app.finish_remote_agent_output(
            task_id,
            base_workspace.clone(),
            RemoteAgentOutput {
                workspace: base_workspace,
                result: Err("provider unavailable".into()),
            },
            true,
        );

        assert_eq!(app.workspace.tasks()[&task_id].status, TaskStatus::Queued);
        assert_eq!(app.is_dirty, dirty_before);
        assert!(app.workspace.document().entities.is_empty());
        assert!(app.status.contains("provider unavailable"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn remote_worker_crash_fails_an_unchanged_audited_task() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let source_revision = app.workspace.revision();
        app.workspace.kernel().begin_task(task_id).unwrap();
        app.workspace
            .kernel()
            .record_event(task_id, provider_disclosure_event(source_revision))
            .unwrap();
        let audited_base = app.workspace.clone();

        app.finish_remote_agent_output(
            task_id,
            audited_base.clone(),
            RemoteAgentOutput {
                workspace: audited_base,
                result: Err("worker panic".into()),
            },
            false,
        );

        let task = &app.workspace.tasks()[&task_id];
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(matches!(
            task.events().last(),
            Some(cadx_core::TaskEvent::Failed { message })
                if message.contains("worker stopped unexpectedly")
        ));
        assert!(app.status.contains("worker stopped unexpectedly"));
        assert!(app.workspace.document().entities.is_empty());
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn paused_remote_plan_resumes_locally_without_rechecking_a_project_grant() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        app.workspace.kernel().begin_task(task_id).unwrap();
        let planning_revision = app.workspace.revision();
        app.workspace
            .kernel()
            .set_task_plan(
                task_id,
                planning_revision,
                vec![drafting_line_action(1, 0.0), drafting_line_action(2, 10.0)],
            )
            .unwrap();
        app.workspace
            .kernel()
            .apply_next_task_action(task_id)
            .unwrap();
        app.workspace
            .kernel()
            .pause_task(task_id, "Awaiting the next run")
            .unwrap();
        app.remote_enabled = true;
        app.clear_remote_context_review();

        app.run_active_remote_task(1);

        assert!(!app.remote_agent_running());
        assert_eq!(
            app.workspace.tasks()[&task_id].status,
            TaskStatus::Completed
        );
        assert_eq!(app.workspace.document().entities.len(), 2);
        assert!(app.is_dirty);
        assert!(app.status.contains("Remote task"));
        assert!(app.status.contains("saved 1 semantic commit"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn conflicting_remote_agent_output_never_overwrites_a_newer_user_edit() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let mut worker_workspace = app.workspace.clone();
        let source_revision = app.workspace.revision();
        app.workspace.kernel().begin_task(task_id).unwrap();
        app.workspace
            .kernel()
            .record_event(task_id, provider_disclosure_event(source_revision))
            .unwrap();
        let base_workspace = app.workspace.clone();
        let report = TaskAgent::new(HeuristicPlanner)
            .run(&mut worker_workspace, task_id)
            .unwrap();

        app.viewport_tool = ViewportTool::Line;
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(12.0, 0.0));
        let user_head = app.workspace.history().head();

        app.finish_remote_agent_output(
            task_id,
            base_workspace,
            RemoteAgentOutput {
                workspace: worker_workspace,
                result: Ok(report),
            },
            true,
        );

        assert_eq!(app.workspace.history().head(), user_head);
        assert_eq!(app.workspace.tasks()[&task_id].status, TaskStatus::Failed);
        assert_eq!(app.workspace.document().entities.len(), 1);
        assert!(matches!(
            app.workspace.document().entities[&1].kind,
            EntityKind::Line { .. }
        ));
        assert!(app.status.contains("Remote task"));
        assert!(app.status.contains("stopped"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn remote_agent_output_merges_after_an_unrelated_user_edit() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let mut worker_workspace = app.workspace.clone();
        let source_revision = app.workspace.revision();
        app.workspace.kernel().begin_task(task_id).unwrap();
        app.workspace
            .kernel()
            .record_event(task_id, provider_disclosure_event(source_revision))
            .unwrap();
        let base_workspace = app.workspace.clone();
        let report = TaskAgent::new(HeuristicPlanner)
            .run(&mut worker_workspace, task_id)
            .unwrap();

        let user_entity = Entity {
            id: 100,
            layer: 1,
            name: "User reference line".into(),
            visible: true,
            kind: EntityKind::Line {
                start: Point2::new(0.0, 20.0),
                end: Point2::new(12.0, 20.0),
            },
            parameter_refs: Default::default(),
        };
        let expected_revision = app.workspace.revision();
        app.workspace
            .kernel()
            .apply_user_transaction(
                expected_revision,
                "Create unrelated user reference",
                CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: user_entity,
                }]),
                ValidationReport::default(),
            )
            .unwrap();
        let user_head = app.workspace.history().head();

        app.finish_remote_agent_output(
            task_id,
            base_workspace,
            RemoteAgentOutput {
                workspace: worker_workspace,
                result: Ok(report),
            },
            true,
        );

        let merged_head = app.workspace.history().head();
        assert_ne!(merged_head, user_head);
        assert_eq!(
            app.workspace.history().commits[&merged_head].parent,
            Some(user_head)
        );
        assert_eq!(
            app.workspace.tasks()[&task_id].status,
            TaskStatus::Completed
        );
        assert_eq!(app.workspace.document().entities.len(), 4);
        assert!(app.workspace.document().entities.contains_key(&100));
        assert!(app.status.contains("saved 1 semantic commit"));
        app.workspace.validate_integrity().unwrap();
    }

    #[test]
    fn remote_agent_output_is_discarded_when_the_target_task_changes() {
        let mut app = CadxApp::default();
        app.create_task();
        let task_id = app.active_task.unwrap();
        let mut worker_workspace = app.workspace.clone();
        let source_revision = app.workspace.revision();
        app.workspace.kernel().begin_task(task_id).unwrap();
        app.workspace
            .kernel()
            .record_event(task_id, provider_disclosure_event(source_revision))
            .unwrap();
        let base_workspace = app.workspace.clone();
        let report = TaskAgent::new(HeuristicPlanner)
            .run(&mut worker_workspace, task_id)
            .unwrap();

        app.workspace
            .kernel()
            .fail_task(task_id, "User canceled the remote task")
            .unwrap();
        let user_head = app.workspace.history().head();

        app.finish_remote_agent_output(
            task_id,
            base_workspace,
            RemoteAgentOutput {
                workspace: worker_workspace,
                result: Ok(report),
            },
            true,
        );

        assert_eq!(app.workspace.history().head(), user_head);
        assert_eq!(app.workspace.tasks()[&task_id].status, TaskStatus::Failed);
        assert!(app.workspace.document().entities.is_empty());
        assert!(app.status.contains("Discarded stale remote task"));
        app.workspace.validate_integrity().unwrap();
    }
}
