use cadx_core::{Point2, Units};
use cadx_render::{
    AlignedDimensionGeometry, RenderItem, RenderPrimitive, ScreenPoint, SnapKind, ViewTransform,
    ViewportSize, aligned_dimension_geometry, aligned_dimension_offset, format_dimension_text,
};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::viewport::ViewportTool;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ArcGeometry {
    pub(crate) center: Point2,
    pub(crate) radius: f64,
    pub(crate) start_angle: f64,
    pub(crate) sweep_angle: f64,
}

pub(crate) fn draw_grid(painter: &egui::Painter, rect: Rect, transform: ViewTransform) {
    let viewport = viewport_size(rect);
    let top_left = transform.unproject(ScreenPoint::new(0.0, 0.0), viewport);
    let bottom_right =
        transform.unproject(ScreenPoint::new(viewport.width, viewport.height), viewport);
    let min_x = top_left.x.min(bottom_right.x);
    let max_x = top_left.x.max(bottom_right.x);
    let min_y = top_left.y.min(bottom_right.y);
    let max_y = top_left.y.max(bottom_right.y);
    let step = grid_step(32.0 / transform.pixels_per_unit);
    let grid_stroke = Stroke::new(1.0, Color32::from_rgb(28, 40, 44));
    let axis_stroke = Stroke::new(1.0, Color32::from_rgb(53, 88, 89));
    let mut x = (min_x / step).floor() * step;
    let mut vertical_lines = 0;
    while x <= max_x + step * 0.5 && vertical_lines < 512 {
        let position = project_point(rect, transform, Point2::new(x, 0.0)).x;
        painter.line_segment(
            [
                Pos2::new(position, rect.top()),
                Pos2::new(position, rect.bottom()),
            ],
            if x.abs() < step * 1e-9 {
                axis_stroke
            } else {
                grid_stroke
            },
        );
        x += step;
        vertical_lines += 1;
    }
    let mut y = (min_y / step).floor() * step;
    let mut horizontal_lines = 0;
    while y <= max_y + step * 0.5 && horizontal_lines < 512 {
        let position = project_point(rect, transform, Point2::new(0.0, y)).y;
        painter.line_segment(
            [
                Pos2::new(rect.left(), position),
                Pos2::new(rect.right(), position),
            ],
            if y.abs() < step * 1e-9 {
                axis_stroke
            } else {
                grid_stroke
            },
        );
        y += step;
        horizontal_lines += 1;
    }
}

pub(crate) fn draw_render_item(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    item: &RenderItem,
    selected: bool,
) {
    let color = if selected {
        Color32::from_rgb(255, 205, 94)
    } else if item.locked {
        Color32::from_rgba_unmultiplied(
            item.color[0],
            item.color[1],
            item.color[2],
            item.color[3].min(145),
        )
    } else {
        Color32::from_rgba_unmultiplied(item.color[0], item.color[1], item.color[2], item.color[3])
    };
    let stroke = Stroke::new(if selected { 2.8 } else { 1.8 }, color);
    match &item.primitive {
        RenderPrimitive::Line { start, end } => {
            painter.line_segment(
                [
                    project_point(rect, transform, *start),
                    project_point(rect, transform, *end),
                ],
                stroke,
            );
        }
        RenderPrimitive::Circle { center, radius } => {
            painter.circle_stroke(
                project_point(rect, transform, *center),
                screen_radius(*radius, transform),
                stroke,
            );
        }
        RenderPrimitive::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            draw_arc_curve(
                painter,
                rect,
                transform,
                ArcGeometry {
                    center: *center,
                    radius: *radius,
                    start_angle: *start_angle,
                    sweep_angle: *sweep_angle,
                },
                stroke,
            );
        }
        RenderPrimitive::AlignedDimension {
            start,
            end,
            offset,
            label,
        } => {
            if let Some(geometry) = aligned_dimension_geometry(*start, *end, *offset) {
                draw_aligned_dimension(painter, rect, transform, geometry, label, stroke);
            }
        }
        RenderPrimitive::Rectangle {
            origin,
            width,
            height,
        } => {
            painter.rect_stroke(
                Rect::from_two_pos(
                    project_point(rect, transform, *origin),
                    project_point(
                        rect,
                        transform,
                        Point2::new(origin.x + width, origin.y + height),
                    ),
                ),
                0.0,
                stroke,
                StrokeKind::Middle,
            );
        }
        RenderPrimitive::SketchProfile { points, closed } => {
            draw_polyline(painter, rect, transform, points, *closed, stroke);
        }
        RenderPrimitive::Extrude { .. } => {}
        RenderPrimitive::Wall {
            start,
            end,
            thickness,
        } => {
            painter.line_segment(
                [
                    project_point(rect, transform, *start),
                    project_point(rect, transform, *end),
                ],
                Stroke::new(screen_radius(*thickness * 0.5, transform).max(2.0), color),
            );
        }
        RenderPrimitive::Room { boundary } => {
            draw_polyline(painter, rect, transform, boundary, true, stroke);
        }
        RenderPrimitive::Text { position, content } => {
            painter.text(
                project_point(rect, transform, *position),
                Align2::LEFT_CENTER,
                content,
                FontId::proportional(15.0),
                color,
            );
        }
    }
}

pub(crate) fn draw_gesture_preview(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    tool: ViewportTool,
    start: Point2,
    end: Point2,
) {
    let stroke = Stroke::new(1.4, Color32::from_rgba_unmultiplied(255, 205, 94, 180));
    match tool {
        ViewportTool::Line => {
            painter.line_segment(
                [
                    project_point(rect, transform, start),
                    project_point(rect, transform, end),
                ],
                stroke,
            );
        }
        ViewportTool::Rectangle => {
            painter.rect_stroke(
                Rect::from_two_pos(
                    project_point(rect, transform, start),
                    project_point(rect, transform, end),
                ),
                0.0,
                stroke,
                StrokeKind::Middle,
            );
        }
        ViewportTool::Circle => {
            painter.circle_stroke(
                project_point(rect, transform, start),
                screen_radius(point_distance(start, end), transform),
                stroke,
            );
        }
        ViewportTool::Arc | ViewportTool::Dimension => {}
        ViewportTool::Select | ViewportTool::Pan => {}
    }
}

pub(crate) fn draw_arc_gesture_preview(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    points: &[Point2],
    pointer: Point2,
) {
    let stroke = Stroke::new(1.4, Color32::from_rgba_unmultiplied(255, 205, 94, 180));
    match points {
        [] => {}
        [start] => {
            painter.line_segment(
                [
                    project_point(rect, transform, *start),
                    project_point(rect, transform, pointer),
                ],
                stroke,
            );
        }
        [start, through] => {
            if let Some(arc) = arc_from_three_points(*start, *through, pointer) {
                draw_arc_curve(painter, rect, transform, arc, stroke);
            } else {
                draw_polyline(
                    painter,
                    rect,
                    transform,
                    &[*start, *through, pointer],
                    false,
                    stroke,
                );
            }
        }
        _ => {}
    }
}

pub(crate) fn draw_dimension_gesture_preview(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    units: Units,
    points: &[Point2],
    pointer: Point2,
) {
    let stroke = Stroke::new(1.4, Color32::from_rgba_unmultiplied(255, 205, 94, 180));
    match points {
        [] => {}
        [start] => {
            painter.line_segment(
                [
                    project_point(rect, transform, *start),
                    project_point(rect, transform, pointer),
                ],
                stroke,
            );
        }
        [start, end] => {
            let Some(offset) = aligned_dimension_offset(*start, *end, pointer) else {
                return;
            };
            let Some(geometry) = aligned_dimension_geometry(*start, *end, offset) else {
                return;
            };
            let label = format_dimension_text(geometry.measurement, units, None);
            draw_aligned_dimension(painter, rect, transform, geometry, &label, stroke);
        }
        _ => {}
    }
}

pub(crate) fn arc_from_three_points(
    start: Point2,
    through: Point2,
    end: Point2,
) -> Option<ArcGeometry> {
    let first = Point2::new(through.x - start.x, through.y - start.y);
    let second = Point2::new(end.x - start.x, end.y - start.y);
    let cross = first.x * second.y - first.y * second.x;
    let scale = first.x.hypot(first.y) * second.x.hypot(second.y);
    if !cross.is_finite() || cross.abs() <= scale.max(1.0) * 1.0e-12 {
        return None;
    }
    let first_squared = first.x.mul_add(first.x, first.y * first.y);
    let second_squared = second.x.mul_add(second.x, second.y * second.y);
    let denominator = 2.0 * cross;
    let center = Point2::new(
        start.x + (first_squared * second.y - second_squared * first.y) / denominator,
        start.y + (first.x * second_squared - second.x * first_squared) / denominator,
    );
    let radius = point_distance(center, start);
    if !center.x.is_finite() || !center.y.is_finite() || !radius.is_finite() || radius < 0.001 {
        return None;
    }

    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let through_angle = (through.y - center.y).atan2(through.x - center.x);
    let end_angle = (end.y - center.y).atan2(end.x - center.x);
    let forward_sweep = positive_angle_delta(start_angle, end_angle);
    let through_delta = positive_angle_delta(start_angle, through_angle);
    let (start_angle, sweep_angle) = if through_delta <= forward_sweep + 1.0e-12 {
        (start_angle, forward_sweep)
    } else {
        (end_angle, positive_angle_delta(end_angle, start_angle))
    };
    if !sweep_angle.is_finite() || sweep_angle <= 0.0 || sweep_angle >= std::f64::consts::TAU {
        return None;
    }
    Some(ArcGeometry {
        center,
        radius,
        start_angle,
        sweep_angle,
    })
}

fn positive_angle_delta(from: f64, to: f64) -> f64 {
    (to - from).rem_euclid(std::f64::consts::TAU)
}

fn draw_arc_curve(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    arc: ArcGeometry,
    stroke: Stroke,
) {
    let screen_length = arc.radius * transform.pixels_per_unit * arc.sweep_angle;
    let segments = ((screen_length / 8.0).ceil() as usize).clamp(8, 512);
    let points = (0..=segments)
        .map(|index| {
            let angle = arc.start_angle + arc.sweep_angle * index as f64 / segments as f64;
            project_point(
                rect,
                transform,
                Point2::new(
                    angle.cos().mul_add(arc.radius, arc.center.x),
                    angle.sin().mul_add(arc.radius, arc.center.y),
                ),
            )
        })
        .collect::<Vec<_>>();
    painter.add(egui::Shape::line(points, stroke));
}

fn draw_aligned_dimension(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    geometry: AlignedDimensionGeometry,
    label: &str,
    stroke: Stroke,
) {
    let source_start = project_point(rect, transform, geometry.start);
    let source_end = project_point(rect, transform, geometry.end);
    let dimension_start = project_point(rect, transform, geometry.dimension_start);
    let dimension_end = project_point(rect, transform, geometry.dimension_end);
    painter.line_segment([source_start, dimension_start], stroke);
    painter.line_segment([source_end, dimension_end], stroke);
    painter.line_segment([dimension_start, dimension_end], stroke);
    draw_arrowhead(painter, dimension_start, dimension_end, stroke.color);
    draw_arrowhead(painter, dimension_end, dimension_start, stroke.color);

    let midpoint = project_point(rect, transform, geometry.dimension_midpoint);
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(14.0), stroke.color);
    let text_rect = Rect::from_center_size(midpoint, galley.size() + Vec2::new(8.0, 4.0));
    painter.rect_filled(
        text_rect,
        0.0,
        Color32::from_rgba_unmultiplied(13, 18, 21, 235),
    );
    painter.galley(
        text_rect.center() - galley.size() * 0.5,
        galley,
        stroke.color,
    );
}

fn draw_arrowhead(painter: &egui::Painter, tip: Pos2, target: Pos2, color: Color32) {
    let delta = target - tip;
    let length = delta.length();
    if length <= f32::EPSILON {
        return;
    }
    let direction = delta / length;
    let normal = Vec2::new(-direction.y, direction.x);
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip,
            tip - direction * 8.0 + normal * 3.5,
            tip - direction * 8.0 - normal * 3.5,
        ],
        color,
        Stroke::NONE,
    ));
}

pub(crate) fn draw_snap_hint(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    point: Point2,
    kind: SnapKind,
) {
    let position = project_point(rect, transform, point);
    let color = match kind {
        SnapKind::Vertex => Color32::from_rgb(111, 220, 196),
        SnapKind::Midpoint => Color32::from_rgb(255, 205, 94),
        SnapKind::Center => Color32::from_rgb(113, 183, 255),
        SnapKind::Quadrant => Color32::from_rgb(225, 145, 207),
        SnapKind::Insertion => Color32::from_rgb(225, 145, 207),
        SnapKind::Grid => Color32::from_rgb(170, 188, 188),
    };
    painter.circle_filled(
        position,
        4.0,
        Color32::from_rgba_unmultiplied(13, 18, 21, 230),
    );
    match kind {
        SnapKind::Grid => painter.rect_stroke(
            Rect::from_center_size(position, Vec2::splat(8.0)),
            0.0,
            Stroke::new(1.4, color),
            StrokeKind::Middle,
        ),
        _ => painter.circle_stroke(position, 4.0, Stroke::new(1.6, color)),
    };
}

fn draw_polyline(
    painter: &egui::Painter,
    rect: Rect,
    transform: ViewTransform,
    points: &[Point2],
    closed: bool,
    stroke: Stroke,
) {
    for window in points.windows(2) {
        painter.line_segment(
            [
                project_point(rect, transform, window[0]),
                project_point(rect, transform, window[1]),
            ],
            stroke,
        );
    }
    if closed && points.len() > 2 {
        painter.line_segment(
            [
                project_point(rect, transform, points[points.len() - 1]),
                project_point(rect, transform, points[0]),
            ],
            stroke,
        );
    }
}

fn viewport_size(rect: Rect) -> ViewportSize {
    ViewportSize::new(f64::from(rect.width()), f64::from(rect.height()))
}

pub(crate) fn screen_at(rect: Rect, point: Pos2) -> ScreenPoint {
    ScreenPoint::new(
        f64::from(point.x - rect.left()),
        f64::from(point.y - rect.top()),
    )
}

pub(crate) fn world_at(rect: Rect, transform: ViewTransform, point: Pos2) -> Point2 {
    transform.unproject(screen_at(rect, point), viewport_size(rect))
}

fn project_point(rect: Rect, transform: ViewTransform, point: Point2) -> Pos2 {
    let screen = transform.project(point, viewport_size(rect));
    Pos2::new(rect.left() + screen.x as f32, rect.top() + screen.y as f32)
}

fn screen_radius(radius: f64, transform: ViewTransform) -> f32 {
    (radius * transform.pixels_per_unit).clamp(0.0, 100_000.0) as f32
}

pub(crate) fn point_distance(left: Point2, right: Point2) -> f64 {
    (left.x - right.x).hypot(left.y - right.y)
}

pub(crate) fn grid_step(target: f64) -> f64 {
    let target = target.max(f64::MIN_POSITIVE);
    let exponent = target.log10().floor();
    let scale = 10_f64.powf(exponent);
    let normalized = target / scale;
    let multiplier = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    multiplier * scale
}
