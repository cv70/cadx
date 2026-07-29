use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cadx_config::UiLanguage;
use cadx_core::TaskWorkspace;
use cadx_io::{discard_recovery, load_recovery, recovery_exists, save_recovery};
use eframe::egui::{self, Color32};

use crate::app::{CadxApp, ensure_project_extension};

const RECOVERY_DEBOUNCE: Duration = Duration::from_secs(2);
const RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(30);
const RECOVERY_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct RecoveryController {
    baseline: TaskWorkspace,
    dirty_since: Option<Instant>,
    retry_at: Option<Instant>,
    job: Option<RecoveryJob>,
    last_recovery_project: Option<PathBuf>,
    pending: Option<PendingRecovery>,
    state: RecoveryState,
}

struct RecoveryJob {
    project_path: PathBuf,
    snapshot: TaskWorkspace,
    receiver: Receiver<Result<u64, String>>,
    handle: JoinHandle<()>,
}

#[derive(Clone)]
struct PendingRecovery {
    project_path: PathBuf,
    open_requested: bool,
}

enum RecoveryState {
    Clean,
    Pending,
    Saving,
    Saved,
    Available,
    Failed(String),
}

impl RecoveryController {
    pub(crate) fn new(workspace: &TaskWorkspace) -> Self {
        Self {
            baseline: workspace.clone(),
            dirty_since: None,
            retry_at: None,
            job: None,
            last_recovery_project: None,
            pending: None,
            state: RecoveryState::Clean,
        }
    }

    pub(crate) fn reset(&mut self, workspace: &TaskWorkspace) {
        self.baseline = workspace.clone();
        self.dirty_since = None;
        self.retry_at = None;
        self.last_recovery_project = None;
        self.pending = None;
        self.state = RecoveryState::Clean;
    }

    pub(crate) fn presentation(
        &self,
        language: UiLanguage,
    ) -> Option<(&'static str, Color32, Option<&str>)> {
        match &self.state {
            RecoveryState::Clean => None,
            RecoveryState::Pending => Some((
                language.text("Recovery pending", "恢复副本待保存"),
                Color32::from_rgb(225, 185, 71),
                None,
            )),
            RecoveryState::Saving => Some((
                language.text("Saving recovery", "正在保存恢复副本"),
                Color32::from_rgb(113, 183, 255),
                None,
            )),
            RecoveryState::Saved => Some((
                language.text("Recovery saved", "恢复副本已保存"),
                Color32::from_rgb(111, 220, 196),
                None,
            )),
            RecoveryState::Available => Some((
                language.text("Recovery available", "发现恢复副本"),
                Color32::from_rgb(225, 185, 71),
                None,
            )),
            RecoveryState::Failed(error) => Some((
                language.text("Recovery failed", "恢复失败"),
                Color32::from_rgb(231, 112, 106),
                Some(error),
            )),
        }
    }

    pub(crate) fn decision_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl CadxApp {
    pub(crate) fn offer_recovery(&mut self, path: &str, open_requested: bool) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        let project_path = PathBuf::from(ensure_project_extension(path.trim()));
        match recovery_exists(&project_path) {
            Ok(true) => {
                self.recovery.pending = Some(PendingRecovery {
                    project_path: project_path.clone(),
                    open_requested,
                });
                self.recovery.state = RecoveryState::Available;
                self.status = match self.language {
                    UiLanguage::English => format!(
                        "A recovery decision is required for {}",
                        project_path.display()
                    ),
                    UiLanguage::SimplifiedChinese => {
                        format!("需要决定如何处理恢复副本：{}", project_path.display())
                    }
                };
                true
            }
            Ok(false) => false,
            Err(error) => {
                let message = error.to_string();
                self.recovery.state = RecoveryState::Failed(message.clone());
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot inspect project recovery: {message}"),
                    UiLanguage::SimplifiedChinese => {
                        format!("无法检查工程恢复副本：{message}")
                    }
                };
                true
            }
        }
    }

    pub(crate) fn update_recovery(&mut self, context: &egui::Context) {
        self.poll_recovery_job();
        if self.recovery.pending.is_some() {
            return;
        }
        if self.recovery.job.is_some() {
            context.request_repaint_after(RECOVERY_POLL_INTERVAL);
            return;
        }
        if self.workspace == self.recovery.baseline {
            self.recovery.dirty_since = None;
            if matches!(self.recovery.state, RecoveryState::Pending) {
                self.recovery.state = RecoveryState::Clean;
            }
            return;
        }

        let now = Instant::now();
        if let Some(retry_at) = self.recovery.retry_at {
            if retry_at > now {
                context.request_repaint_after(retry_at.duration_since(now));
                return;
            }
            self.recovery.retry_at = None;
        }
        let dirty_since = *self.recovery.dirty_since.get_or_insert_with(|| {
            self.recovery.state = RecoveryState::Pending;
            now
        });
        let ready_at = dirty_since + RECOVERY_DEBOUNCE;
        if ready_at > now {
            context.request_repaint_after(ready_at.duration_since(now));
            return;
        }
        self.start_recovery_job();
        context.request_repaint_after(RECOVERY_POLL_INTERVAL);
    }

    pub(crate) fn primary_save_completed(&mut self) {
        let current_project = self.recovery_project_path();
        let mut cleanup_error = discard_recovery(&current_project).err();
        if let Some(previous_project) = self.recovery.last_recovery_project.take()
            && previous_project != current_project
            && let Err(error) = discard_recovery(previous_project)
        {
            cleanup_error = Some(error);
        }
        self.recovery.baseline = self.workspace.clone();
        self.recovery.dirty_since = None;
        self.recovery.retry_at = None;
        self.recovery.state = match cleanup_error {
            Some(error) => RecoveryState::Failed(error.to_string()),
            None => RecoveryState::Clean,
        };
    }

    pub(crate) fn ui_recovery_dialog(&mut self, context: &egui::Context) {
        let Some(pending) = self.recovery.pending.clone() else {
            return;
        };
        let language = self.language;
        egui::Window::new(language.text("Project Recovery", "工程恢复"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(context, |ui| {
                ui.label(
                    egui::RichText::new(
                        language.text("Unsaved workspace available", "发现未保存的工作区"),
                    )
                    .strong(),
                );
                ui.label(pending.project_path.display().to_string());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(language.text("Recover", "恢复")).clicked() {
                        self.recover_pending_workspace();
                    }
                    if ui.button(language.text("Discard", "丢弃")).clicked() {
                        self.discard_pending_workspace();
                    }
                });
            });
    }

    pub(crate) fn flush_recovery_on_exit(&mut self) {
        let _ = self.checkpoint_recovery_now();
    }

    pub(crate) fn checkpoint_recovery_now(&mut self) -> bool {
        self.finish_recovery_job_blocking();
        if self.recovery.pending.is_some() {
            return false;
        }
        if self.workspace == self.recovery.baseline {
            return true;
        }
        let project_path = self.recovery_project_path();
        if let Err(error) = save_recovery(&self.workspace, &project_path) {
            self.recovery.state = RecoveryState::Failed(error.to_string());
            false
        } else {
            self.recovery.baseline = self.workspace.clone();
            self.recovery.last_recovery_project = Some(project_path);
            self.recovery.state = RecoveryState::Saved;
            true
        }
    }

    fn start_recovery_job(&mut self) {
        let snapshot = self.workspace.clone();
        let project_path = self.recovery_project_path();
        let worker_snapshot = snapshot.clone();
        let worker_path = project_path.clone();
        let (sender, receiver) = mpsc::channel();
        match thread::Builder::new()
            .name("cadx-recovery".into())
            .spawn(move || {
                let result = save_recovery(&worker_snapshot, &worker_path)
                    .map(|report| report.workspace_bytes)
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            }) {
            Ok(handle) => {
                self.recovery.job = Some(RecoveryJob {
                    project_path,
                    snapshot,
                    receiver,
                    handle,
                });
                self.recovery.dirty_since = None;
                self.recovery.state = RecoveryState::Saving;
            }
            Err(error) => {
                self.recovery.retry_at = Some(Instant::now() + RECOVERY_RETRY_DELAY);
                self.recovery.state = RecoveryState::Failed(error.to_string());
            }
        }
    }

    fn poll_recovery_job(&mut self) {
        let result = match self
            .recovery
            .job
            .as_ref()
            .map(|job| job.receiver.try_recv())
        {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => Some(Err(
                "recovery worker stopped before reporting a result".into(),
            )),
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        let Some(result) = result else {
            return;
        };
        let Some(job) = self.recovery.job.take() else {
            return;
        };
        let joined = job.handle.join().is_ok();
        self.finish_recovery_job(job.project_path, job.snapshot, result, joined);
    }

    pub(crate) fn finish_recovery_job_blocking(&mut self) {
        let Some(job) = self.recovery.job.take() else {
            return;
        };
        let joined = job.handle.join().is_ok();
        let result = job
            .receiver
            .try_recv()
            .unwrap_or_else(|_| Err("recovery worker did not return a result".into()));
        self.finish_recovery_job(job.project_path, job.snapshot, result, joined);
    }

    fn finish_recovery_job(
        &mut self,
        project_path: PathBuf,
        snapshot: TaskWorkspace,
        result: Result<u64, String>,
        joined: bool,
    ) {
        if !joined {
            self.recovery.retry_at = Some(Instant::now() + RECOVERY_RETRY_DELAY);
            self.recovery.state = RecoveryState::Failed("recovery worker panicked".into());
            return;
        }
        match result {
            Ok(_) => {
                if let Some(previous_project) = self.recovery.last_recovery_project.take()
                    && previous_project != project_path
                {
                    let _ = discard_recovery(previous_project);
                }
                self.recovery.baseline = snapshot;
                self.recovery.last_recovery_project = Some(project_path);
                self.recovery.retry_at = None;
                self.recovery.state = RecoveryState::Saved;
            }
            Err(error) => {
                self.recovery.retry_at = Some(Instant::now() + RECOVERY_RETRY_DELAY);
                self.recovery.state = RecoveryState::Failed(error);
            }
        }
    }

    fn recover_pending_workspace(&mut self) {
        let Some(pending) = self.recovery.pending.clone() else {
            return;
        };
        match load_recovery(&pending.project_path) {
            Ok(loaded) => {
                self.project_path = pending.project_path.display().to_string();
                self.current_project_path = Some(pending.project_path.clone());
                self.install_workspace(loaded.workspace, true);
                self.recovery.reset(&self.workspace);
                self.recovery.last_recovery_project = Some(pending.project_path);
                self.recovery.state = RecoveryState::Saved;
                self.status = self
                    .language
                    .text(
                        "Recovered the validated autosave workspace. Save to keep it.",
                        "已恢复并验证自动保存的工作区，请保存工程以保留更改。",
                    )
                    .into();
            }
            Err(error) => {
                let message = error.to_string();
                self.recovery.state = RecoveryState::Failed(message.clone());
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot recover project: {message}"),
                    UiLanguage::SimplifiedChinese => format!("无法恢复工程：{message}"),
                };
            }
        }
    }

    fn discard_pending_workspace(&mut self) {
        let Some(pending) = self.recovery.pending.clone() else {
            return;
        };
        match discard_recovery(&pending.project_path) {
            Ok(_) => {
                self.recovery.pending = None;
                self.recovery.state = RecoveryState::Clean;
                if pending.open_requested {
                    self.load_primary_project(pending.project_path.display().to_string());
                } else {
                    self.status = self
                        .language
                        .text(
                            "Discarded the stale recovery workspace.",
                            "已丢弃过期的恢复工作区。",
                        )
                        .into();
                }
            }
            Err(error) => {
                let message = error.to_string();
                self.recovery.state = RecoveryState::Failed(message.clone());
                self.status = match self.language {
                    UiLanguage::English => format!("Cannot discard recovery: {message}"),
                    UiLanguage::SimplifiedChinese => format!("无法丢弃恢复副本：{message}"),
                };
            }
        }
    }

    fn recovery_project_path(&self) -> PathBuf {
        self.current_project_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(ensure_project_extension(self.project_path.trim())))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use cadx_core::Point2;
    use cadx_io::{load_workspace, recovery_exists, recovery_path};

    use super::*;
    use crate::viewport::ViewportTool;

    fn test_project_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cadx-app-{label}-{}-{nonce}.cadx",
            std::process::id()
        ))
    }

    #[test]
    fn background_recovery_is_lossless_and_primary_save_cleans_it() {
        let project_path = test_project_path("background-recovery");
        let mut app = CadxApp {
            project_path: project_path.display().to_string(),
            viewport_tool: ViewportTool::Line,
            ..Default::default()
        };
        app.recovery.reset(&app.workspace);
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(12.0, 0.0));

        app.recovery.dirty_since = Some(Instant::now() - RECOVERY_DEBOUNCE);
        app.update_recovery(&egui::Context::default());
        assert!(app.recovery.job.is_some());
        app.finish_recovery_job_blocking();

        assert!(recovery_exists(&project_path).unwrap());
        assert_eq!(
            load_recovery(&project_path).unwrap().workspace,
            app.workspace
        );

        app.start_recovery_job();
        assert!(app.recovery.job.is_some());
        app.save_project();

        assert!(!app.is_dirty);
        assert!(app.recovery.job.is_none());
        assert!(!recovery_exists(&project_path).unwrap());
        assert_eq!(
            load_workspace(&project_path).unwrap().workspace,
            app.workspace
        );
        fs::remove_file(project_path).unwrap();
    }

    #[test]
    fn discovered_recovery_requires_a_decision_and_restores_dirty_workspace() {
        let project_path = test_project_path("recovery-decision");
        let mut source = CadxApp {
            viewport_tool: ViewportTool::Rectangle,
            ..Default::default()
        };
        source.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(8.0, 6.0));
        save_recovery(&source.workspace, &project_path).unwrap();

        let mut recovered = CadxApp {
            project_path: project_path.display().to_string(),
            ..Default::default()
        };
        recovered.recovery.reset(&recovered.workspace);

        assert!(recovered.offer_recovery(&project_path.display().to_string(), true));
        assert!(recovered.recovery.decision_pending());
        recovered.recover_pending_workspace();

        assert_eq!(recovered.workspace, source.workspace);
        assert!(recovered.is_dirty);
        assert!(!recovered.recovery.decision_pending());
        assert!(recovery_exists(&project_path).unwrap());

        recovered.save_project();
        assert!(!recovery_exists(&project_path).unwrap());
        fs::remove_file(project_path).unwrap();
    }

    #[test]
    fn editing_the_path_field_does_not_redirect_an_open_projects_recovery() {
        let open_project = test_project_path("open-project");
        let path_field = test_project_path("path-field");
        let mut app = CadxApp {
            current_project_path: Some(open_project.clone()),
            project_path: path_field.display().to_string(),
            viewport_tool: ViewportTool::Circle,
            ..Default::default()
        };
        app.recovery.reset(&app.workspace);
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0));

        app.start_recovery_job();
        app.finish_recovery_job_blocking();

        assert!(recovery_exists(&open_project).unwrap());
        assert!(!recovery_exists(&path_field).unwrap());
        discard_recovery(open_project).unwrap();
    }

    #[test]
    fn recovery_write_failure_preserves_dirty_workspace_and_blocks_checkpoint() {
        let missing_parent = test_project_path("missing-parent").with_extension("");
        let project_path = missing_parent.join("project.cadx");
        let mut app = CadxApp {
            project_path: project_path.display().to_string(),
            viewport_tool: ViewportTool::Line,
            ..Default::default()
        };
        app.recovery.reset(&app.workspace);
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0));

        assert!(!app.checkpoint_recovery_now());

        assert_eq!(app.workspace.document().entities.len(), 1);
        assert!(matches!(
            app.recovery.presentation(UiLanguage::English),
            Some(("Recovery failed", _, Some(_)))
        ));
        assert!(!missing_parent.exists());
    }

    #[test]
    fn invalid_recovery_never_replaces_the_current_workspace() {
        let project_path = test_project_path("invalid-recovery");
        let recovery_path = recovery_path(&project_path).unwrap();
        fs::write(&recovery_path, b"not a CADX archive").unwrap();
        let mut app = CadxApp {
            project_path: project_path.display().to_string(),
            ..Default::default()
        };
        app.recovery.reset(&app.workspace);
        let original = app.workspace.clone();

        assert!(app.offer_recovery(&project_path.display().to_string(), true));
        app.recover_pending_workspace();

        assert_eq!(app.workspace, original);
        assert!(app.recovery.decision_pending());
        assert!(matches!(
            app.recovery.presentation(UiLanguage::English),
            Some(("Recovery failed", _, Some(_)))
        ));
        fs::remove_file(recovery_path).unwrap();
    }

    #[test]
    fn final_checkpoint_cannot_be_overwritten_by_an_older_background_snapshot() {
        let project_path = test_project_path("recovery-ordering");
        let mut app = CadxApp {
            project_path: project_path.display().to_string(),
            viewport_tool: ViewportTool::Line,
            ..Default::default()
        };
        app.recovery.reset(&app.workspace);
        app.commit_draw_gesture(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0));
        app.start_recovery_job();
        app.commit_draw_gesture(Point2::new(0.0, 2.0), Point2::new(5.0, 2.0));

        assert!(app.checkpoint_recovery_now());

        assert_eq!(
            load_recovery(&project_path).unwrap().workspace,
            app.workspace
        );
        assert_eq!(app.workspace.document().entities.len(), 2);
        discard_recovery(project_path).unwrap();
    }
}
