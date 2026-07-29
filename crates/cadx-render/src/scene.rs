use cadx_core::{CadDocument, EntityId, EntityKind, LayerId, Point2, Units};

use crate::bounds::Bounds2;
use crate::geometry::{
    PickHit, SnapHit, SnapKind, SnapSettings, bounds_from_points, compare_snap_hits, distance,
    distance_to_arc, distance_to_closed_polyline, distance_to_polyline, distance_to_segment,
    finite_point, format_dimension_text, midpoint, point_in_polygon, polyline_snap_points,
};
use crate::geometry::{aligned_dimension_geometry, angle_on_arc, arc_point, arc_snap_points};

#[derive(Clone, Debug, PartialEq)]
pub struct RenderScene {
    pub items: Vec<RenderItem>,
    pub bounds: Option<Bounds2>,
}

impl RenderScene {
    pub fn from_document(document: &CadDocument) -> Self {
        let mut items = Vec::new();
        let mut bounds: Option<Bounds2> = None;
        for entity in document.entities.values() {
            let Some(layer) = document.layers.get(&entity.layer) else {
                continue;
            };
            if !entity.visible || !layer.visible {
                continue;
            }
            let primitive = RenderPrimitive::from_entity_kind(&entity.kind, document.units);
            if let Some(item_bounds) = primitive.bounds() {
                match &mut bounds {
                    Some(scene_bounds) => scene_bounds.include_bounds(item_bounds),
                    None => bounds = Some(item_bounds),
                }
            }
            items.push(RenderItem {
                entity_id: entity.id,
                layer_id: layer.id,
                color: layer.color,
                locked: layer.locked,
                primitive,
            });
        }
        Self { items, bounds }
    }

    pub fn pick(&self, point: Point2, tolerance: f64) -> Option<PickHit> {
        if !finite_point(point) || !tolerance.is_finite() || tolerance < 0.0 {
            return None;
        }
        self.items
            .iter()
            .filter(|item| !item.locked)
            .filter_map(|item| {
                item.primitive
                    .distance_to(point)
                    .filter(|distance| *distance <= tolerance)
                    .map(|distance| PickHit {
                        entity_id: item.entity_id,
                        distance,
                    })
            })
            .min_by(|left, right| {
                left.distance
                    .total_cmp(&right.distance)
                    .then_with(|| left.entity_id.cmp(&right.entity_id))
            })
    }

    /// Finds the closest eligible drafting snap point in visible scene geometry or on the grid.
    pub fn snap(&self, point: Point2, tolerance: f64, settings: SnapSettings) -> Option<SnapHit> {
        if !finite_point(point) || !tolerance.is_finite() || tolerance < 0.0 {
            return None;
        }

        let mut hits = Vec::new();
        if settings.geometry_enabled {
            for item in &self.items {
                for (kind, candidate) in item.primitive.snap_points() {
                    if !finite_point(candidate) {
                        continue;
                    }
                    let distance = distance(point, candidate);
                    if distance.is_finite() && distance <= tolerance {
                        hits.push(SnapHit {
                            point: candidate,
                            kind,
                            entity_id: Some(item.entity_id),
                            distance,
                        });
                    }
                }
            }
        }

        if settings.grid_enabled
            && settings.grid_step.is_finite()
            && settings.grid_step > f64::MIN_POSITIVE
        {
            let candidate = Point2::new(
                (point.x / settings.grid_step).round() * settings.grid_step,
                (point.y / settings.grid_step).round() * settings.grid_step,
            );
            if finite_point(candidate) {
                let distance = distance(point, candidate);
                if distance.is_finite() && distance <= tolerance {
                    hits.push(SnapHit {
                        point: candidate,
                        kind: SnapKind::Grid,
                        entity_id: None,
                        distance,
                    });
                }
            }
        }

        hits.into_iter().min_by(compare_snap_hits)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderItem {
    pub entity_id: EntityId,
    pub layer_id: LayerId,
    pub color: [u8; 4],
    pub locked: bool,
    pub primitive: RenderPrimitive,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderPrimitive {
    Line {
        start: Point2,
        end: Point2,
    },
    Circle {
        center: Point2,
        radius: f64,
    },
    Arc {
        center: Point2,
        radius: f64,
        start_angle: f64,
        sweep_angle: f64,
    },
    AlignedDimension {
        start: Point2,
        end: Point2,
        offset: f64,
        label: String,
    },
    Rectangle {
        origin: Point2,
        width: f64,
        height: f64,
    },
    SketchProfile {
        points: Vec<Point2>,
        closed: bool,
    },
    Extrude {
        profile: EntityId,
        distance: f64,
    },
    Wall {
        start: Point2,
        end: Point2,
        thickness: f64,
    },
    Room {
        boundary: Vec<Point2>,
    },
    Text {
        position: Point2,
        content: String,
    },
}

impl RenderPrimitive {
    fn from_entity_kind(kind: &EntityKind, units: Units) -> Self {
        match kind {
            EntityKind::Line { start, end } => Self::Line {
                start: *start,
                end: *end,
            },
            EntityKind::Circle { center, radius } => Self::Circle {
                center: *center,
                radius: *radius,
            },
            EntityKind::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => Self::Arc {
                center: *center,
                radius: *radius,
                start_angle: *start_angle,
                sweep_angle: *sweep_angle,
            },
            EntityKind::AlignedDimension {
                start,
                end,
                offset,
                text_override,
            } => Self::AlignedDimension {
                start: *start,
                end: *end,
                offset: *offset,
                label: format_dimension_text(
                    distance(*start, *end),
                    units,
                    text_override.as_deref(),
                ),
            },
            EntityKind::Rectangle {
                origin,
                width,
                height,
            } => Self::Rectangle {
                origin: *origin,
                width: *width,
                height: *height,
            },
            EntityKind::SketchProfile { points, closed } => Self::SketchProfile {
                points: points.clone(),
                closed: *closed,
            },
            EntityKind::Extrude { profile, distance } => Self::Extrude {
                profile: *profile,
                distance: *distance,
            },
            EntityKind::Wall {
                start,
                end,
                thickness,
            } => Self::Wall {
                start: *start,
                end: *end,
                thickness: *thickness,
            },
            EntityKind::Room { boundary, .. } => Self::Room {
                boundary: boundary.clone(),
            },
            EntityKind::Text { position, content } => Self::Text {
                position: *position,
                content: content.clone(),
            },
        }
    }

    pub fn bounds(&self) -> Option<Bounds2> {
        match self {
            Self::Line { start, end } => Some(bounds_from_points([*start, *end])),
            Self::Circle { center, radius } => Some(Bounds2 {
                min: Point2::new(center.x - radius, center.y - radius),
                max: Point2::new(center.x + radius, center.y + radius),
            }),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                let start = arc_point(*center, *radius, *start_angle);
                let mut bounds = Bounds2::from_point(start);
                bounds.include_point(arc_point(*center, *radius, *start_angle + *sweep_angle));
                for angle in [
                    0.0,
                    std::f64::consts::FRAC_PI_2,
                    std::f64::consts::PI,
                    std::f64::consts::PI + std::f64::consts::FRAC_PI_2,
                ] {
                    if angle_on_arc(angle, *start_angle, *sweep_angle) {
                        bounds.include_point(arc_point(*center, *radius, angle));
                    }
                }
                Some(bounds)
            }
            Self::AlignedDimension {
                start, end, offset, ..
            } => aligned_dimension_geometry(*start, *end, *offset).map(|geometry| {
                bounds_from_points([
                    geometry.start,
                    geometry.end,
                    geometry.dimension_start,
                    geometry.dimension_end,
                ])
            }),
            Self::Rectangle {
                origin,
                width,
                height,
            } => Some(Bounds2 {
                min: *origin,
                max: Point2::new(origin.x + width, origin.y + height),
            }),
            Self::SketchProfile { points, .. } | Self::Room { boundary: points } => {
                (!points.is_empty()).then(|| bounds_from_points(points.iter().copied()))
            }
            Self::Extrude { .. } => None,
            Self::Wall {
                start,
                end,
                thickness,
            } => {
                let radius = thickness * 0.5;
                Some(Bounds2 {
                    min: Point2::new(start.x.min(end.x) - radius, start.y.min(end.y) - radius),
                    max: Point2::new(start.x.max(end.x) + radius, start.y.max(end.y) + radius),
                })
            }
            Self::Text { position, .. } => Some(Bounds2::from_point(*position)),
        }
    }

    pub fn distance_to(&self, point: Point2) -> Option<f64> {
        match self {
            Self::Line { start, end } => Some(distance_to_segment(point, *start, *end)),
            Self::Circle { center, radius } => Some((distance(*center, point) - *radius).abs()),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => Some(distance_to_arc(
                point,
                *center,
                *radius,
                *start_angle,
                *sweep_angle,
            )),
            Self::AlignedDimension {
                start, end, offset, ..
            } => aligned_dimension_geometry(*start, *end, *offset).map(|geometry| {
                distance_to_segment(point, geometry.start, geometry.dimension_start)
                    .min(distance_to_segment(
                        point,
                        geometry.end,
                        geometry.dimension_end,
                    ))
                    .min(distance_to_segment(
                        point,
                        geometry.dimension_start,
                        geometry.dimension_end,
                    ))
            }),
            Self::Rectangle {
                origin,
                width,
                height,
            } => {
                let bounds = Bounds2 {
                    min: *origin,
                    max: Point2::new(origin.x + width, origin.y + height),
                };
                if bounds.contains(point) {
                    Some(0.0)
                } else {
                    Some(distance_to_closed_polyline(
                        point,
                        &[
                            bounds.min,
                            Point2::new(bounds.max.x, bounds.min.y),
                            bounds.max,
                            Point2::new(bounds.min.x, bounds.max.y),
                        ],
                    ))
                }
            }
            Self::SketchProfile { points, closed } => {
                distance_to_polyline(point, points, *closed).filter(|distance| distance.is_finite())
            }
            Self::Extrude { .. } => None,
            Self::Wall {
                start,
                end,
                thickness,
            } => Some((distance_to_segment(point, *start, *end) - thickness * 0.5).max(0.0)),
            Self::Room { boundary } => {
                if point_in_polygon(point, boundary) {
                    Some(0.0)
                } else {
                    distance_to_polyline(point, boundary, true)
                }
            }
            Self::Text { position, .. } => Some(distance(point, *position)),
        }
    }

    fn snap_points(&self) -> Vec<(SnapKind, Point2)> {
        match self {
            Self::Line { start, end } | Self::Wall { start, end, .. } => vec![
                (SnapKind::Vertex, *start),
                (SnapKind::Vertex, *end),
                (SnapKind::Midpoint, midpoint(*start, *end)),
            ],
            Self::Circle { center, radius } => vec![
                (SnapKind::Center, *center),
                (SnapKind::Quadrant, Point2::new(center.x + radius, center.y)),
                (SnapKind::Quadrant, Point2::new(center.x, center.y + radius)),
                (SnapKind::Quadrant, Point2::new(center.x - radius, center.y)),
                (SnapKind::Quadrant, Point2::new(center.x, center.y - radius)),
            ],
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => arc_snap_points(*center, *radius, *start_angle, *sweep_angle),
            Self::AlignedDimension {
                start, end, offset, ..
            } => aligned_dimension_geometry(*start, *end, *offset).map_or_else(
                Vec::new,
                |geometry| {
                    vec![
                        (SnapKind::Vertex, geometry.start),
                        (SnapKind::Vertex, geometry.end),
                        (SnapKind::Vertex, geometry.dimension_start),
                        (SnapKind::Vertex, geometry.dimension_end),
                        (SnapKind::Insertion, geometry.dimension_midpoint),
                    ]
                },
            ),
            Self::Rectangle {
                origin,
                width,
                height,
            } => {
                let lower_left = *origin;
                let lower_right = Point2::new(origin.x + width, origin.y);
                let upper_right = Point2::new(origin.x + width, origin.y + height);
                let upper_left = Point2::new(origin.x, origin.y + height);
                vec![
                    (SnapKind::Vertex, lower_left),
                    (SnapKind::Vertex, lower_right),
                    (SnapKind::Vertex, upper_right),
                    (SnapKind::Vertex, upper_left),
                    (SnapKind::Midpoint, midpoint(lower_left, lower_right)),
                    (SnapKind::Midpoint, midpoint(lower_right, upper_right)),
                    (SnapKind::Midpoint, midpoint(upper_right, upper_left)),
                    (SnapKind::Midpoint, midpoint(upper_left, lower_left)),
                    (SnapKind::Center, midpoint(lower_left, upper_right)),
                ]
            }
            Self::SketchProfile { points, closed } => polyline_snap_points(points, *closed),
            Self::Extrude { .. } => Vec::new(),
            Self::Room { boundary } => polyline_snap_points(boundary, true),
            Self::Text { position, .. } => vec![(SnapKind::Insertion, *position)],
        }
    }
}
