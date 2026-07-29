use std::sync::Arc;

use cadx_render::{RenderScene, ScreenPoint, SnapSettings, ViewportSize};
use eframe::egui::{self, Align2, Color32, FontId, Sense, Vec2};

use crate::app::CadxApp;
use crate::drawing::{
    draw_gesture_preview, draw_grid, draw_render_item, draw_snap_hint, grid_step, screen_at,
    world_at,
};
use crate::gpu_viewport::MechanicalGpuCallback;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ViewportMode {
    #[default]
    Drafting2d,
    Mechanical3d,
}

impl ViewportMode {
    pub(crate) const ALL: [Self; 2] = [Self::Drafting2d, Self::Mechanical3d];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Drafting2d => "2D",
            Self::Mechanical3d => "3D",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewportTool {
    Select,
    Pan,
    Line,
    Rectangle,
    Circle,
    Arc,
    Dimension,
}

impl ViewportTool {
    pub(crate) const ALL: [Self; 7] = [
        Self::Select,
        Self::Pan,
        Self::Line,
        Self::Rectangle,
        Self::Circle,
        Self::Arc,
        Self::Dimension,
    ];

    pub(crate) const fn creates_geometry(self) -> bool {
        matches!(
            self,
            Self::Line | Self::Rectangle | Self::Circle | Self::Arc | Self::Dimension
        )
    }
}

impl CadxApp {
    pub(crate) fn ui_viewport(&mut self, context: &egui::Context) {
        match self.viewport_mode {
            ViewportMode::Drafting2d => self.ui_drafting_viewport(context),
            ViewportMode::Mechanical3d => self.ui_mechanical_viewport(context),
        }
    }

    fn ui_drafting_viewport(&mut self, context: &egui::Context) {
        egui::CentralPanel::default().show(context, |ui| {
            let available = ui.available_size();
            let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
            let rect = response.rect;
            self.viewport_size = ViewportSize::new(rect.width() as f64, rect.height() as f64);
            let pointer_position = response
                .interact_pointer_pos()
                .or_else(|| response.hover_pos());

            if response.hovered()
                && let Some(position) = response.hover_pos()
            {
                let scroll = context.input(|input| input.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    let anchor = screen_at(rect, position);
                    self.view_transform.zoom_at(
                        anchor,
                        self.viewport_size,
                        (f64::from(scroll) * 0.01).exp(),
                    );
                }
            }

            let scene = RenderScene::from_document(self.workspace.document());
            let raw_pointer_world =
                pointer_position.map(|position| world_at(rect, self.view_transform, position));
            let snap_tolerance = 10.0 / self.view_transform.pixels_per_unit;
            let snap_settings = SnapSettings::new(
                self.snap_geometry,
                self.snap_grid,
                grid_step(32.0 / self.view_transform.pixels_per_unit),
            );
            let snap_hit = self
                .viewport_tool
                .creates_geometry()
                .then(|| {
                    raw_pointer_world
                        .and_then(|point| scene.snap(point, snap_tolerance, snap_settings))
                })
                .flatten();
            let pointer_world = snap_hit.map(|hit| hit.point).or(raw_pointer_world);

            if self.viewport_tool == ViewportTool::Pan && response.dragged() {
                let delta = context.input(|input| input.pointer.delta());
                self.view_transform
                    .pan_pixels(ScreenPoint::new(f64::from(delta.x), f64::from(delta.y)));
            }
            if self.viewport_tool == ViewportTool::Arc {
                if response.clicked()
                    && let Some(point) = pointer_world
                {
                    if self.arc_points.len() == 2 {
                        let start = self.arc_points[0];
                        let through = self.arc_points[1];
                        self.arc_points.clear();
                        self.commit_three_point_arc(start, through, point);
                    } else {
                        self.arc_points.push(point);
                    }
                }
            } else if self.viewport_tool == ViewportTool::Dimension {
                if response.clicked()
                    && let Some(point) = pointer_world
                {
                    if self.dimension_points.len() == 2 {
                        let start = self.dimension_points[0];
                        let end = self.dimension_points[1];
                        self.dimension_points.clear();
                        self.commit_aligned_dimension(start, end, point);
                    } else {
                        self.dimension_points.push(point);
                    }
                }
            } else if self.viewport_tool.creates_geometry() {
                if response.drag_started()
                    && let Some(position) = context.input(|input| input.pointer.press_origin())
                {
                    let origin = world_at(rect, self.view_transform, position);
                    self.draw_origin = Some(
                        scene
                            .snap(origin, snap_tolerance, snap_settings)
                            .map_or(origin, |hit| hit.point),
                    );
                }
                if response.drag_stopped() {
                    if let (Some(start), Some(end)) = (self.draw_origin, pointer_world) {
                        self.commit_draw_gesture(start, end);
                    }
                    self.draw_origin = None;
                }
            } else if self.viewport_tool == ViewportTool::Select && response.clicked() {
                self.selected_entity = raw_pointer_world
                    .and_then(|point| scene.pick(point, 8.0 / self.view_transform.pixels_per_unit))
                    .map(|hit| hit.entity_id);
            }

            painter.rect_filled(rect, 0.0, Color32::from_rgb(13, 18, 21));
            draw_grid(&painter, rect, self.view_transform);
            painter.text(
                rect.left_top() + Vec2::new(16.0, 14.0),
                Align2::LEFT_TOP,
                self.language.text("MODEL SPACE", "模型空间"),
                FontId::proportional(12.0),
                Color32::from_gray(130),
            );
            for item in &scene.items {
                let selected = self.selected_entity == Some(item.entity_id);
                draw_render_item(&painter, rect, self.view_transform, item, selected);
            }
            if let Some(hit) = snap_hit
                && self.viewport_tool.creates_geometry()
            {
                draw_snap_hint(&painter, rect, self.view_transform, hit.point, hit.kind);
            }
            if let (Some(start), Some(end)) = (self.draw_origin, pointer_world)
                && self.viewport_tool.creates_geometry()
            {
                draw_gesture_preview(
                    &painter,
                    rect,
                    self.view_transform,
                    self.viewport_tool,
                    start,
                    end,
                );
            }
            if self.viewport_tool == ViewportTool::Arc
                && let Some(pointer) = pointer_world
            {
                crate::drawing::draw_arc_gesture_preview(
                    &painter,
                    rect,
                    self.view_transform,
                    &self.arc_points,
                    pointer,
                );
            }
            if self.viewport_tool == ViewportTool::Dimension
                && let Some(pointer) = pointer_world
            {
                crate::drawing::draw_dimension_gesture_preview(
                    &painter,
                    rect,
                    self.view_transform,
                    self.workspace.document().units,
                    &self.dimension_points,
                    pointer,
                );
            }
            if self.workspace.document().entities.is_empty() {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    self.language
                        .text("Create a design task to begin", "创建设计任务以开始"),
                    FontId::proportional(18.0),
                    Color32::from_gray(135),
                );
            }
        });
    }

    fn ui_mechanical_viewport(&mut self, context: &egui::Context) {
        egui::CentralPanel::default().show(context, |ui| {
            let available = ui.available_size();
            let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
            let rect = response.rect;
            self.viewport_size = ViewportSize::new(rect.width() as f64, rect.height() as f64);
            self.refresh_mechanical_scene();
            if self.mechanical_fit_pending {
                if let Some(bounds) = self.mechanical_scene.bounds {
                    self.orbit_camera
                        .fit_bounds(bounds, self.viewport_size, 0.12);
                }
                self.mechanical_fit_pending = false;
            }

            if response.hovered() {
                let scroll = context.input(|input| input.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.orbit_camera.zoom((f64::from(scroll) * 0.01).exp());
                }
            }
            if response.dragged() {
                let delta = context.input(|input| input.pointer.delta());
                self.orbit_camera
                    .orbit_pixels(f64::from(delta.x), f64::from(delta.y));
            }
            if response.clicked()
                && let Some(position) = response.interact_pointer_pos()
            {
                let point = ScreenPoint::new(
                    f64::from(position.x - rect.left()),
                    f64::from(position.y - rect.top()),
                );
                self.selected_entity = self
                    .mechanical_scene
                    .pick(self.orbit_camera, self.viewport_size, point)
                    .map(|hit| hit.entity_id);
            }

            painter.rect_filled(rect, 0.0, Color32::from_rgb(14, 18, 20));
            let mut gpu_frame_error = None;
            if self.mechanical_scene_error.is_none()
                && self.mechanical_gpu_error.is_none()
                && !self.mechanical_scene.items.is_empty()
            {
                match MechanicalGpuCallback::new(
                    Arc::clone(&self.mechanical_gpu_scene),
                    self.mechanical_gpu_revision,
                    self.orbit_camera,
                    self.viewport_size,
                    self.selected_entity,
                ) {
                    Ok(callback) => {
                        painter.add(callback.paint_callback(rect));
                    }
                    Err(error) => gpu_frame_error = Some(error.to_string()),
                }
            }
            painter.text(
                rect.left_top() + Vec2::new(16.0, 14.0),
                Align2::LEFT_TOP,
                self.language.text("MECHANICAL 3D", "机械 3D"),
                FontId::proportional(12.0),
                Color32::from_gray(140),
            );

            if let Some(error) = self
                .mechanical_scene_error
                .as_ref()
                .or(self.mechanical_gpu_error.as_ref())
                .or(gpu_frame_error.as_ref())
            {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    match self.language {
                        cadx_config::UiLanguage::English => {
                            format!("Cannot display solids: {error}")
                        }
                        cadx_config::UiLanguage::SimplifiedChinese => {
                            format!("无法显示实体：{error}")
                        }
                    },
                    FontId::proportional(15.0),
                    Color32::from_rgb(235, 150, 132),
                );
            } else if self.mechanical_scene.items.is_empty() {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    self.language
                        .text("No visible extrusion solids", "没有可见的拉伸实体"),
                    FontId::proportional(17.0),
                    Color32::from_gray(135),
                );
            }
        });
    }
}
