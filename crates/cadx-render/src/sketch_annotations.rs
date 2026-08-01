use cadx_core::{
    domain::{Constraint, FeatureId, SketchDimensionKind},
    kernel::{EvaluatedScene, EvaluatedSketch},
};
use egui::{Color32, FontId, Pos2, Rect, Stroke, Ui, Vec2};
use glam::Mat4;

use super::{OrbitCamera, project_topology_point};

const DIMENSION_OFFSET: f32 = 26.0;
const LABEL_HEIGHT: f32 = 18.0;
const LABEL_PADDING: f32 = 8.0;
const COLLISION_STEP: f32 = 20.0;
const NORMAL_COLOR: Color32 = Color32::from_rgb(103, 177, 218);
const GLYPH_COLOR: Color32 = Color32::from_rgb(92, 196, 143);
const HOVER_COLOR: Color32 = Color32::from_rgb(255, 183, 77);
const REDUNDANT_COLOR: Color32 = Color32::from_rgb(230, 164, 91);
const CONFLICT_COLOR: Color32 = Color32::from_rgb(224, 103, 103);
const LABEL_BACKGROUND: Color32 = Color32::from_rgba_premultiplied(14, 15, 17, 232);

#[derive(Debug, Clone)]
enum ScreenGeometry {
    Glyph {
        anchor: Pos2,
    },
    Linear {
        witnesses: [Pos2; 2],
        dimension: [Pos2; 2],
    },
    Angular {
        center: Pos2,
        rays: [Pos2; 2],
        arc: Vec<Pos2>,
    },
    Radial {
        center: Pos2,
        rim: Pos2,
    },
}

/// One deterministic screen-space sketch annotation. The label rectangle is
/// public so the desktop adapter can perform direct dimension editing without
/// duplicating projection or collision-layout logic.
#[derive(Debug, Clone)]
pub struct ScreenSketchAnnotation {
    pub feature_id: FeatureId,
    pub constraint_index: u32,
    pub constraint: Constraint,
    pub label_rect: Rect,
    label: String,
    leader: Option<[Pos2; 2]>,
    geometry: ScreenGeometry,
}

impl ScreenSketchAnnotation {
    #[must_use]
    pub const fn is_dimension(&self) -> bool {
        self.constraint.dimension().is_some()
    }
}

/// Projects the selected visible sketch's solved constraint metadata into a
/// stable, collision-avoiding screen-space layout.
#[must_use]
pub fn layout_sketch_annotations(
    scene: &EvaluatedScene,
    selected: Option<FeatureId>,
    viewport: Rect,
    camera: OrbitCamera,
) -> Vec<ScreenSketchAnnotation> {
    if viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return Vec::new();
    }
    let Some(sketch) =
        selected.and_then(|id| scene.sketches.iter().find(|sketch| sketch.feature_id == id))
    else {
        return Vec::new();
    };
    let projection =
        Mat4::from_cols_array_2d(&camera.view_projection(viewport.width() / viewport.height()));
    let mut occupied = Vec::new();
    let mut result = Vec::new();

    for annotation in &sketch.constraint_annotations {
        match &annotation.geometry {
            cadx_core::kernel::SketchAnnotationGeometry2D::Glyph { anchors } => {
                let label = constraint_glyph(&annotation.constraint);
                for anchor in anchors {
                    let Some(anchor) = project_local(sketch, *anchor, viewport, projection) else {
                        continue;
                    };
                    let ideal = anchor + Vec2::new(10.0, -10.0);
                    let (label_rect, leader) =
                        place_label(ideal, label_size(label), viewport, &mut occupied);
                    result.push(ScreenSketchAnnotation {
                        feature_id: sketch.feature_id,
                        constraint_index: annotation.constraint_index,
                        constraint: annotation.constraint.clone(),
                        label_rect,
                        label: label.into(),
                        leader,
                        geometry: ScreenGeometry::Glyph { anchor },
                    });
                }
            }
            cadx_core::kernel::SketchAnnotationGeometry2D::LinearDimension {
                first,
                second,
                axis,
            } => {
                let Some(first_screen) = project_local(sketch, *first, viewport, projection) else {
                    continue;
                };
                let Some(second_screen) = project_local(sketch, *second, viewport, projection)
                else {
                    continue;
                };
                let axis = screen_axis(
                    sketch,
                    *first,
                    *axis,
                    first_screen,
                    second_screen,
                    viewport,
                    projection,
                );
                let normal = Vec2::new(-axis.y, axis.x);
                let baseline = first_screen
                    .to_vec2()
                    .dot(normal)
                    .max(second_screen.to_vec2().dot(normal))
                    + DIMENSION_OFFSET;
                let first_dimension_vector =
                    axis * first_screen.to_vec2().dot(axis) + normal * baseline;
                let second_dimension_vector =
                    axis * second_screen.to_vec2().dot(axis) + normal * baseline;
                let first_dimension = Pos2::new(first_dimension_vector.x, first_dimension_vector.y);
                let second_dimension =
                    Pos2::new(second_dimension_vector.x, second_dimension_vector.y);
                let ideal = first_dimension + (second_dimension - first_dimension) * 0.5;
                let label = dimension_label(&annotation.constraint);
                let (label_rect, leader) =
                    place_label(ideal, label_size(&label), viewport, &mut occupied);
                result.push(ScreenSketchAnnotation {
                    feature_id: sketch.feature_id,
                    constraint_index: annotation.constraint_index,
                    constraint: annotation.constraint.clone(),
                    label_rect,
                    label,
                    leader,
                    geometry: ScreenGeometry::Linear {
                        witnesses: [first_screen, second_screen],
                        dimension: [first_dimension, second_dimension],
                    },
                });
            }
            cadx_core::kernel::SketchAnnotationGeometry2D::AngularDimension {
                center,
                first_ray,
                second_ray,
            } => {
                let Some(center) = project_local(sketch, *center, viewport, projection) else {
                    continue;
                };
                let Some(first_ray) = project_local(sketch, *first_ray, viewport, projection)
                else {
                    continue;
                };
                let Some(second_ray) = project_local(sketch, *second_ray, viewport, projection)
                else {
                    continue;
                };
                let first_direction = normalized_or(first_ray - center, Vec2::X);
                let second_direction = normalized_or(second_ray - center, Vec2::Y);
                let radius = 28.0;
                let rays = [
                    center + first_direction * radius,
                    center + second_direction * radius,
                ];
                let arc = angular_arc(center, first_direction, second_direction, radius);
                let ideal = arc
                    .get(arc.len() / 2)
                    .copied()
                    .unwrap_or(center + Vec2::new(radius, -radius));
                let label = dimension_label(&annotation.constraint);
                let (label_rect, leader) =
                    place_label(ideal, label_size(&label), viewport, &mut occupied);
                result.push(ScreenSketchAnnotation {
                    feature_id: sketch.feature_id,
                    constraint_index: annotation.constraint_index,
                    constraint: annotation.constraint.clone(),
                    label_rect,
                    label,
                    leader,
                    geometry: ScreenGeometry::Angular { center, rays, arc },
                });
            }
            cadx_core::kernel::SketchAnnotationGeometry2D::RadialDimension { center, rim } => {
                let Some(center) = project_local(sketch, *center, viewport, projection) else {
                    continue;
                };
                let Some(rim) = project_local(sketch, *rim, viewport, projection) else {
                    continue;
                };
                let direction = normalized_or(rim - center, Vec2::X);
                let ideal = rim + direction * 20.0;
                let label = dimension_label(&annotation.constraint);
                let (label_rect, leader) =
                    place_label(ideal, label_size(&label), viewport, &mut occupied);
                result.push(ScreenSketchAnnotation {
                    feature_id: sketch.feature_id,
                    constraint_index: annotation.constraint_index,
                    constraint: annotation.constraint.clone(),
                    label_rect,
                    label,
                    leader,
                    geometry: ScreenGeometry::Radial { center, rim },
                });
            }
        }
    }
    result
}

/// Paints constraint glyphs and drafting-style dimension geometry over the
/// WGPU viewport. Diagnostics use the same zero-based indices as the scene.
pub fn paint_sketch_annotations(
    ui: &Ui,
    annotations: &[ScreenSketchAnnotation],
    redundant_constraints: &[u32],
    conflict_constraints: &[u32],
    editing_constraint: Option<u32>,
) {
    let hover_position = ui.input(|input| input.pointer.hover_pos());
    let hovered_dimension =
        hover_position.and_then(|pointer| pick_sketch_dimension(annotations, pointer));
    if hovered_dimension.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    for annotation in annotations {
        let hovered = hovered_dimension.is_some_and(|candidate| {
            candidate.feature_id == annotation.feature_id
                && candidate.constraint_index == annotation.constraint_index
        });
        let color = annotation_color(
            annotation,
            hovered,
            redundant_constraints,
            conflict_constraints,
            editing_constraint,
        );
        let stroke = Stroke::new(if hovered { 1.8 } else { 1.25 }, color);
        match &annotation.geometry {
            ScreenGeometry::Glyph { anchor } => {
                painter.circle_filled(*anchor, 2.2, color);
            }
            ScreenGeometry::Linear {
                witnesses,
                dimension,
            } => {
                let direction = normalized_or(dimension[1] - dimension[0], Vec2::X);
                let normal = Vec2::new(-direction.y, direction.x);
                painter.line_segment(
                    [witnesses[0], dimension[0] + normal * 4.0],
                    Stroke::new(0.9, color.gamma_multiply(0.75)),
                );
                painter.line_segment(
                    [witnesses[1], dimension[1] + normal * 4.0],
                    Stroke::new(0.9, color.gamma_multiply(0.75)),
                );
                painter.line_segment(*dimension, stroke);
                paint_arrowhead(painter, dimension[0], direction, color);
                paint_arrowhead(painter, dimension[1], -direction, color);
            }
            ScreenGeometry::Angular { center, rays, arc } => {
                painter.line_segment([*center, rays[0]], stroke);
                painter.line_segment([*center, rays[1]], stroke);
                painter.add(egui::Shape::line(arc.clone(), stroke));
            }
            ScreenGeometry::Radial { center, rim } => {
                painter.line_segment([*center, *rim], stroke);
                painter.line_segment([*rim, annotation.label_rect.center()], stroke);
                paint_arrowhead(painter, *rim, normalized_or(*center - *rim, Vec2::X), color);
                painter.circle_filled(*center, 2.2, color);
            }
        }
        if let Some(leader) = annotation.leader {
            painter.line_segment(leader, Stroke::new(0.8, color.gamma_multiply(0.72)));
        }
        painter.rect_filled(annotation.label_rect, 2.0, LABEL_BACKGROUND);
        painter.text(
            annotation.label_rect.center(),
            egui::Align2::CENTER_CENTER,
            &annotation.label,
            FontId::proportional(11.0),
            color,
        );
    }
}

#[must_use]
pub fn pick_sketch_dimension(
    annotations: &[ScreenSketchAnnotation],
    pointer: Pos2,
) -> Option<&ScreenSketchAnnotation> {
    annotations
        .iter()
        .filter(|annotation| annotation.is_dimension())
        .filter(|annotation| annotation.label_rect.expand(4.0).contains(pointer))
        .min_by(|first, second| {
            first
                .label_rect
                .center()
                .distance(pointer)
                .total_cmp(&second.label_rect.center().distance(pointer))
        })
}

fn project_local(
    sketch: &EvaluatedSketch,
    point: [f64; 2],
    viewport: Rect,
    projection: Mat4,
) -> Option<Pos2> {
    let world = std::array::from_fn(|axis| {
        sketch.x_direction[axis].mul_add(
            point[0],
            sketch.y_direction[axis].mul_add(point[1], sketch.origin[axis]),
        )
    });
    project_topology_point(world, viewport, projection).map(|(point, _)| point)
}

fn screen_axis(
    sketch: &EvaluatedSketch,
    local_origin: [f64; 2],
    local_axis: Option<[f64; 2]>,
    first: Pos2,
    second: Pos2,
    viewport: Rect,
    projection: Mat4,
) -> Vec2 {
    if let Some(axis) = local_axis {
        let endpoint = [local_origin[0] + axis[0], local_origin[1] + axis[1]];
        if let Some(endpoint) = project_local(sketch, endpoint, viewport, projection) {
            return normalized_or(endpoint - first, Vec2::X);
        }
    }
    if (second - first).length_sq() > f32::EPSILON {
        return (second - first).normalized();
    }
    let endpoint = [local_origin[0] + 1.0, local_origin[1]];
    project_local(sketch, endpoint, viewport, projection)
        .map_or(Vec2::X, |endpoint| normalized_or(endpoint - first, Vec2::X))
}

fn angular_arc(center: Pos2, first: Vec2, second: Vec2, radius: f32) -> Vec<Pos2> {
    let start = first.y.atan2(first.x);
    let mut sweep = second.y.atan2(second.x) - start;
    while sweep > std::f32::consts::PI {
        sweep -= std::f32::consts::TAU;
    }
    while sweep < -std::f32::consts::PI {
        sweep += std::f32::consts::TAU;
    }
    (0_u8..=16)
        .map(|step| {
            let angle = sweep.mul_add(f32::from(step) / 16.0, start);
            center + Vec2::angled(angle) * radius
        })
        .collect()
}

fn normalized_or(value: Vec2, fallback: Vec2) -> Vec2 {
    if value.length_sq() > f32::EPSILON {
        value.normalized()
    } else {
        fallback
    }
}

fn label_size(label: &str) -> Vec2 {
    let character_count = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
    Vec2::new(
        f32::from(character_count) * 6.8 + LABEL_PADDING,
        LABEL_HEIGHT,
    )
}

fn place_label(
    ideal: Pos2,
    size: Vec2,
    viewport: Rect,
    occupied: &mut Vec<Rect>,
) -> (Rect, Option<[Pos2; 2]>) {
    const OFFSETS: [Vec2; 13] = [
        Vec2::ZERO,
        Vec2::new(0.0, -COLLISION_STEP),
        Vec2::new(0.0, COLLISION_STEP),
        Vec2::new(COLLISION_STEP, 0.0),
        Vec2::new(-COLLISION_STEP, 0.0),
        Vec2::new(COLLISION_STEP, -COLLISION_STEP),
        Vec2::new(-COLLISION_STEP, -COLLISION_STEP),
        Vec2::new(COLLISION_STEP, COLLISION_STEP),
        Vec2::new(-COLLISION_STEP, COLLISION_STEP),
        Vec2::new(0.0, -2.0 * COLLISION_STEP),
        Vec2::new(0.0, 2.0 * COLLISION_STEP),
        Vec2::new(2.0 * COLLISION_STEP, 0.0),
        Vec2::new(-2.0 * COLLISION_STEP, 0.0),
    ];
    let bounds = viewport.shrink(4.0);
    let mut selected = Rect::from_center_size(ideal, size);
    for offset in OFFSETS {
        let candidate = clamp_rect(Rect::from_center_size(ideal + offset, size), bounds);
        if occupied
            .iter()
            .all(|existing| !existing.expand(2.0).intersects(candidate))
        {
            selected = candidate;
            break;
        }
    }
    occupied.push(selected);
    let displaced = selected.center().distance(ideal) > 2.0;
    (selected, displaced.then_some([ideal, selected.center()]))
}

fn clamp_rect(rect: Rect, bounds: Rect) -> Rect {
    let mut offset = Vec2::ZERO;
    if rect.left() < bounds.left() {
        offset.x += bounds.left() - rect.left();
    } else if rect.right() > bounds.right() {
        offset.x -= rect.right() - bounds.right();
    }
    if rect.top() < bounds.top() {
        offset.y += bounds.top() - rect.top();
    } else if rect.bottom() > bounds.bottom() {
        offset.y -= rect.bottom() - bounds.bottom();
    }
    rect.translate(offset)
}

fn compact_number(value: f64, precision: usize) -> String {
    let mut formatted = format!("{value:.precision$}");
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }
    if formatted == "-0" {
        "0".into()
    } else {
        formatted
    }
}

fn dimension_label(constraint: &Constraint) -> String {
    let Some(dimension) = constraint.dimension() else {
        return constraint_glyph(constraint).into();
    };
    let precision = usize::from(dimension.kind == SketchDimensionKind::Angle) + 2;
    let value = compact_number(dimension.value, precision);
    match dimension.kind {
        SketchDimensionKind::HorizontalDistance => format!("X {value} mm"),
        SketchDimensionKind::VerticalDistance => format!("Y {value} mm"),
        SketchDimensionKind::PointLineDistance => format!("D {value} mm"),
        SketchDimensionKind::Radius => format!("R {value} mm"),
        SketchDimensionKind::Angle => format!("{value}°"),
        SketchDimensionKind::Distance | SketchDimensionKind::Length => format!("{value} mm"),
    }
}

fn constraint_glyph(constraint: &Constraint) -> &'static str {
    match constraint {
        Constraint::Coincident { .. } | Constraint::Concentric { .. } => "◎",
        Constraint::Horizontal { .. } => "H",
        Constraint::Vertical { .. } => "V",
        Constraint::Fixed { .. } => "F",
        Constraint::LineThroughCenter { .. } => "⊙",
        Constraint::PointOnCurve { .. } => "○",
        Constraint::Midpoint { .. } => "△",
        Constraint::Symmetric { .. } => "↔",
        Constraint::EqualLength { .. } | Constraint::EqualRadius { .. } => "=",
        Constraint::Parallel { .. } => "∥",
        Constraint::Perpendicular { .. } => "⊥",
        Constraint::FixedCenter { .. } => "⊕",
        Constraint::Tangent { .. } => "T",
        Constraint::CurvatureContinuous { .. } => "G²",
        Constraint::Distance { .. }
        | Constraint::HorizontalDistance { .. }
        | Constraint::VerticalDistance { .. }
        | Constraint::PointLineDistance { .. }
        | Constraint::Length { .. }
        | Constraint::Angle { .. }
        | Constraint::Radius { .. } => "",
    }
}

fn annotation_color(
    annotation: &ScreenSketchAnnotation,
    hovered: bool,
    redundant_constraints: &[u32],
    conflict_constraints: &[u32],
    editing_constraint: Option<u32>,
) -> Color32 {
    if conflict_constraints.contains(&annotation.constraint_index) {
        CONFLICT_COLOR
    } else if redundant_constraints.contains(&annotation.constraint_index) {
        REDUNDANT_COLOR
    } else if hovered || editing_constraint == Some(annotation.constraint_index) {
        HOVER_COLOR
    } else if annotation.is_dimension() {
        NORMAL_COLOR
    } else {
        GLYPH_COLOR
    }
}

fn paint_arrowhead(painter: &egui::Painter, tip: Pos2, direction: Vec2, color: Color32) {
    let direction = normalized_or(direction, Vec2::X);
    let normal = Vec2::new(-direction.y, direction.x);
    let base = tip + direction * 6.0;
    let stroke = Stroke::new(1.25, color);
    painter.line_segment([tip, base + normal * 3.2], stroke);
    painter.line_segment([tip, base - normal * 3.2], stroke);
}

#[cfg(test)]
mod tests {
    use cadx_core::{
        domain::{Constraint, SketchLoop2D},
        kernel::EvaluatedSketch,
    };

    use super::*;

    fn annotated_scene(constraints: &[Constraint]) -> EvaluatedScene {
        let profile = SketchLoop2D::from_polygon(vec![
            [-10.0, -6.0],
            [10.0, -6.0],
            [10.0, 6.0],
            [-10.0, 6.0],
        ]);
        let constraint_annotations =
            cadx_core::kernel::constraint_annotations(&profile, &[], constraints).unwrap();
        EvaluatedScene {
            sketches: vec![EvaluatedSketch {
                feature_id: 42,
                name: "annotated".into(),
                color: [0.2, 0.7, 0.5, 1.0],
                constraint_annotations,
                profile: profile
                    .vertices()
                    .into_iter()
                    .map(|point| [point[0], point[1], 0.0])
                    .collect(),
                holes: Vec::new(),
                construction: Vec::new(),
                origin: [0.0, 0.0, 0.0],
                x_direction: [1.0, 0.0, 0.0],
                y_direction: [0.0, 1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            }],
            ..EvaluatedScene::default()
        }
    }

    #[test]
    fn dimension_layout_is_screen_stable_and_hit_testable() {
        let scene = annotated_scene(&[Constraint::Length {
            segment: 0,
            length: 20.0,
        }]);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0));
        let mut camera = OrbitCamera::default();
        camera.frame_scene(&scene);
        let first = layout_sketch_annotations(&scene, Some(42), viewport, camera);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].constraint_index, 0);
        assert!(first[0].label_rect.width() >= 40.0);
        assert_eq!(
            pick_sketch_dimension(&first, first[0].label_rect.center())
                .map(|annotation| annotation.constraint_index),
            Some(0)
        );

        camera.distance *= 2.0;
        let zoomed = layout_sketch_annotations(&scene, Some(42), viewport, camera);
        let ScreenGeometry::Linear {
            witnesses,
            dimension,
        } = &zoomed[0].geometry
        else {
            panic!("length must render as a linear dimension");
        };
        assert!((dimension[0].distance(witnesses[0]) - DIMENSION_OFFSET).abs() < 0.1);
    }

    #[test]
    fn collocated_glyphs_receive_distinct_label_rectangles() {
        let scene = annotated_scene(&[
            Constraint::Horizontal { segment: 0 },
            Constraint::Fixed {
                point: 0,
                x: -10.0,
                y: -6.0,
            },
            Constraint::Coincident {
                first: 0,
                second: 0,
            },
        ]);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0));
        let mut camera = OrbitCamera::default();
        camera.frame_scene(&scene);
        let annotations = layout_sketch_annotations(&scene, Some(42), viewport, camera);
        assert_eq!(annotations.len(), 3);
        for (index, annotation) in annotations.iter().enumerate() {
            assert!(
                annotations[index + 1..]
                    .iter()
                    .all(|other| !annotation.label_rect.intersects(other.label_rect))
            );
        }
    }

    #[test]
    fn curvature_continuity_has_a_non_dimensional_g2_glyph() {
        let constraint = Constraint::CurvatureContinuous {
            first: 0,
            second: 1,
        };
        assert_eq!(constraint_glyph(&constraint), "G²");
        assert!(constraint.dimension().is_none());
    }
}
