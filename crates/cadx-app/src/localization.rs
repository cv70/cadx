use cadx_config::{CadxPreferences, UiLanguage};
use cadx_core::{AgentRunStatus, ChangeSetStatus, Domain, EntityKind, TaskStatus, Units};
use cadx_io::{PdfOrientation, PdfPageSize};

use crate::app::CadxApp;
use crate::viewport::ViewportTool;

pub(crate) const DEFAULT_TASK_GOAL_EN: &str = "Create a mechanical mounting bracket";
pub(crate) const DEFAULT_TASK_GOAL_ZH: &str = "创建一个机械安装支架";

impl CadxApp {
    pub(crate) fn apply_interface_language(&mut self, language: UiLanguage, persist: bool) {
        if self.task_goal == DEFAULT_TASK_GOAL_EN || self.task_goal == DEFAULT_TASK_GOAL_ZH {
            self.task_goal = language
                .text(DEFAULT_TASK_GOAL_EN, DEFAULT_TASK_GOAL_ZH)
                .into();
        }
        self.language = language;
        self.status = if persist {
            language
                .text("Interface language changed.", "界面语言已切换。")
                .into()
        } else {
            language
                .text("Ready for a design task", "已准备好接收设计任务")
                .into()
        };
        if persist && let Err(error) = CadxPreferences::for_language(language).save_default() {
            self.status = match language {
                UiLanguage::English => {
                    format!("Interface language changed, but the preference was not saved: {error}")
                }
                UiLanguage::SimplifiedChinese => {
                    format!("界面语言已切换，但无法保存语言偏好：{error}")
                }
            }
        }
    }
}

pub(crate) const fn task_status_label(language: UiLanguage, status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => language.text("Queued", "等待中"),
        TaskStatus::Running => language.text("Running", "运行中"),
        TaskStatus::Paused => language.text("Paused", "已暂停"),
        TaskStatus::Completed => language.text("Saved", "已保存"),
        TaskStatus::Failed => language.text("Stopped", "已停止"),
        TaskStatus::Cancelled => language.text("Cancelled", "已取消"),
    }
}

pub(crate) const fn change_set_status_label(
    language: UiLanguage,
    status: ChangeSetStatus,
) -> &'static str {
    match status {
        ChangeSetStatus::Running => language.text("Running", "运行中"),
        ChangeSetStatus::Completed => language.text("Completed", "已完成"),
        ChangeSetStatus::PartiallyFailed => language.text("Partially failed", "部分失败"),
        ChangeSetStatus::Cancelled => language.text("Cancelled", "已取消"),
        ChangeSetStatus::Reverted => language.text("Reverted", "已回滚"),
        ChangeSetStatus::RevertedWithConflicts => {
            language.text("Reverted with conflicts", "回滚有冲突")
        }
    }
}

pub(crate) const fn agent_run_status_label(
    language: UiLanguage,
    status: AgentRunStatus,
) -> &'static str {
    match status {
        AgentRunStatus::Queued => language.text("Queued", "等待中"),
        AgentRunStatus::Running => language.text("Running", "运行中"),
        AgentRunStatus::Paused => language.text("Paused", "已暂停"),
        AgentRunStatus::Completed => language.text("Completed", "已完成"),
        AgentRunStatus::Failed => language.text("Failed", "失败"),
        AgentRunStatus::Cancelled => language.text("Cancelled", "已取消"),
    }
}

pub(crate) const fn viewport_tool_label(language: UiLanguage, tool: ViewportTool) -> &'static str {
    match tool {
        ViewportTool::Select => language.text("Select", "选择"),
        ViewportTool::Pan => language.text("Pan", "平移"),
        ViewportTool::Line => language.text("Line", "直线"),
        ViewportTool::Rectangle => language.text("Rectangle", "矩形"),
        ViewportTool::Circle => language.text("Circle", "圆"),
        ViewportTool::Arc => language.text("Arc", "圆弧"),
        ViewportTool::Dimension => language.text("Dimension", "标注"),
    }
}

pub(crate) const fn domain_label(language: UiLanguage, domain: Domain) -> &'static str {
    match domain {
        Domain::Drafting => language.text("Drafting", "制图"),
        Domain::Mechanical => language.text("Mechanical", "机械"),
        Domain::Architecture => language.text("Architecture", "建筑"),
    }
}

pub(crate) const fn entity_kind_label(language: UiLanguage, kind: &EntityKind) -> &'static str {
    match kind {
        EntityKind::Line { .. } => language.text("Line", "直线"),
        EntityKind::Circle { .. } => language.text("Circle", "圆"),
        EntityKind::Arc { .. } => language.text("Arc", "圆弧"),
        EntityKind::AlignedDimension { .. } => language.text("Dim", "标注"),
        EntityKind::Rectangle { .. } => language.text("Rect", "矩形"),
        EntityKind::SketchProfile { .. } => language.text("Sketch", "草图"),
        EntityKind::Extrude { .. } => language.text("Solid", "实体"),
        EntityKind::Wall { .. } => language.text("Wall", "墙体"),
        EntityKind::Room { .. } => language.text("Room", "房间"),
        EntityKind::Text { .. } => language.text("Text", "文本"),
    }
}

pub(crate) const fn pdf_page_size_label(
    language: UiLanguage,
    page_size: PdfPageSize,
) -> &'static str {
    match page_size {
        PdfPageSize::A4 => "A4",
        PdfPageSize::A3 => "A3",
        PdfPageSize::Letter => language.text("Letter", "信纸"),
    }
}

pub(crate) const fn pdf_orientation_label(
    language: UiLanguage,
    orientation: PdfOrientation,
) -> &'static str {
    match orientation {
        PdfOrientation::Portrait => language.text("Portrait", "纵向"),
        PdfOrientation::Landscape => language.text("Landscape", "横向"),
    }
}

pub(crate) const fn unit_label(units: Units) -> &'static str {
    match units {
        Units::Millimeters => "mm",
        Units::Meters => "m",
        Units::Inches => "in",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_labels_have_english_and_chinese_variants() {
        assert_eq!(
            viewport_tool_label(UiLanguage::English, ViewportTool::Dimension),
            "Dimension"
        );
        assert_eq!(
            viewport_tool_label(UiLanguage::SimplifiedChinese, ViewportTool::Dimension),
            "标注"
        );
        assert_eq!(
            domain_label(UiLanguage::SimplifiedChinese, Domain::Mechanical),
            "机械"
        );
        assert_eq!(
            change_set_status_label(UiLanguage::English, ChangeSetStatus::RevertedWithConflicts),
            "Reverted with conflicts"
        );
        assert_eq!(
            change_set_status_label(UiLanguage::SimplifiedChinese, ChangeSetStatus::Reverted),
            "已回滚"
        );
    }

    #[test]
    fn runtime_language_switch_updates_ui_defaults_without_mutating_project_data() {
        let mut app = CadxApp::default();
        let document = app.workspace.document().clone();

        app.apply_interface_language(UiLanguage::SimplifiedChinese, false);

        assert_eq!(app.language, UiLanguage::SimplifiedChinese);
        assert_eq!(app.task_goal, DEFAULT_TASK_GOAL_ZH);
        assert_eq!(app.status, "已准备好接收设计任务");
        assert_eq!(app.workspace.document(), &document);

        app.apply_interface_language(UiLanguage::English, false);
        assert_eq!(app.task_goal, DEFAULT_TASK_GOAL_EN);
        assert_eq!(app.status, "Ready for a design task");
    }
}
