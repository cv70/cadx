mod app;
mod drawing;
mod exchange;
mod gpu_viewport;
mod layers;
mod localization;
mod panels;
mod parametrics;
mod recovery;
mod status;
mod theme;
mod viewport;

use std::ffi::OsString;
use std::path::PathBuf;

use app::{CadxApp, ensure_project_extension};
use cadx_config::{CadxPreferences, UiLanguage, initialize_default_config_if_missing};
use eframe::egui;

fn main() -> eframe::Result<()> {
    let configuration_error = initialize_default_config_if_missing().err();
    let project_argument = parse_project_argument(std::env::args_os().skip(1));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 920.0]),
        depth_buffer: gpu_viewport::GPU_DEPTH_BITS,
        multisampling: gpu_viewport::GPU_MSAA_SAMPLES,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "CADX",
        options,
        Box::new(move |creation_context| {
            theme::configure_style(&creation_context.egui_ctx);
            let mut app = CadxApp::default();
            match CadxPreferences::load_default() {
                Ok(preferences) => app.apply_interface_language(preferences.language, false),
                Err(error) => {
                    let language = UiLanguage::detect_system();
                    app.apply_interface_language(language, false);
                    app.status = match language {
                        UiLanguage::English => {
                            format!("Cannot load interface preferences: {error}")
                        }
                        UiLanguage::SimplifiedChinese => {
                            format!("无法加载界面偏好设置：{error}")
                        }
                    };
                }
            }
            if let Some(render_state) = creation_context.wgpu_render_state.as_ref() {
                app.gpu_adapter = gpu_viewport::install_gpu_resources(render_state);
            } else {
                app.status = app
                    .language
                    .text(
                        "WGPU renderer initialization is unavailable.",
                        "WGPU 渲染器不可用。",
                    )
                    .into();
            }
            match &project_argument {
                Ok(Some(path)) => match path.to_str() {
                    Some(path) => app.load_primary_project(ensure_project_extension(path)),
                    None => {
                        app.status = app
                            .language
                            .text(
                                "Project path must be valid UTF-8.",
                                "工程路径必须是有效的 UTF-8。",
                            )
                            .into();
                    }
                },
                Ok(None) => {}
                Err(error) => app.status = localize_argument_error(app.language, error),
            }
            if let Some(error) = configuration_error.as_ref() {
                app.status = if app.language == cadx_config::UiLanguage::SimplifiedChinese {
                    format!("CADX 工作目录不可用：{error}")
                } else {
                    format!("CADX working directory is unavailable: {error}")
                };
            }
            Ok(Box::new(app))
        }),
    )
}

fn localize_argument_error(language: UiLanguage, error: &str) -> String {
    if language == UiLanguage::English {
        return error.into();
    }
    if error == "--project requires a local project path." {
        "--project 需要本地工程路径。".into()
    } else if error == "Only one project path can be opened at startup." {
        "启动时只能打开一个工程路径。".into()
    } else if let Some(option) = error.strip_prefix("Unknown command-line option: ") {
        format!("未知命令行选项：{option}")
    } else {
        format!("无法处理命令行参数：{error}")
    }
}

fn parse_project_argument(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<PathBuf>, String> {
    let mut arguments = arguments.into_iter();
    let Some(first) = arguments.next() else {
        return Ok(None);
    };
    let project = if first == "--project" || first == "-p" {
        arguments
            .next()
            .ok_or_else(|| "--project requires a local project path.".to_string())?
    } else if first.to_string_lossy().starts_with('-') {
        return Err(format!(
            "Unknown command-line option: {}",
            first.to_string_lossy()
        ));
    } else {
        first
    };
    if arguments.next().is_some() {
        return Err("Only one project path can be opened at startup.".into());
    }
    Ok(Some(PathBuf::from(project)))
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn startup_accepts_positional_and_flagged_project_paths() {
        assert_eq!(
            parse_project_argument([OsString::from("sample.cadx")]).unwrap(),
            Some(PathBuf::from("sample.cadx"))
        );
        assert_eq!(
            parse_project_argument([OsString::from("--project"), OsString::from("sample.cadx")])
                .unwrap(),
            Some(PathBuf::from("sample.cadx"))
        );
        assert_eq!(parse_project_argument([]).unwrap(), None);
    }

    #[test]
    fn startup_rejects_unknown_or_ambiguous_arguments() {
        assert!(parse_project_argument([OsString::from("--unknown")]).is_err());
        assert!(parse_project_argument([OsString::from("--project")]).is_err());
        assert!(
            parse_project_argument([OsString::from("one.cadx"), OsString::from("two.cadx")])
                .is_err()
        );
    }

    #[test]
    fn startup_argument_errors_are_localized_without_changing_options() {
        assert_eq!(
            localize_argument_error(
                UiLanguage::SimplifiedChinese,
                "Unknown command-line option: --bad"
            ),
            "未知命令行选项：--bad"
        );
        assert_eq!(
            localize_argument_error(
                UiLanguage::English,
                "Only one project path can be opened at startup."
            ),
            "Only one project path can be opened at startup."
        );
    }
}
