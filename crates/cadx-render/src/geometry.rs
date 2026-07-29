use cadx_core::{EntityId, Point2, Units};

use crate::bounds::Bounds2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    pub entity_id: EntityId,
    pub distance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapKind {
    Vertex,
    Midpoint,
    Center,
    Quadrant,
    Insertion,
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapSettings {
    pub geometry_enabled: bool,
    pub grid_enabled: bool,
    pub grid_step: f64,
}

impl SnapSettings {
    pub const fn new(geometry_enabled: bool, grid_enabled: bool, grid_step: f64) -> Self {
        Self {
            geometry_enabled,
            grid_enabled,
            grid_step,
        }
    }
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self::new(true, true, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapHit {
    pub point: Point2,
    pub kind: SnapKind,
    pub entity_id: Option<EntityId>,
    pub distance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignedDimensionGeometry {
    pub start: Point2,
    pub end: Point2,
    pub dimension_start: Point2,
    pub dimension_end: Point2,
    pub dimension_midpoint: Point2,
    pub unit_direction: Point2,
    pub normal_direction: Point2,
    pub measurement: f64,
}

pub fn aligned_dimension_geometry(
    start: Point2,
    end: Point2,
    offset: f64,
) -> Option<AlignedDimensionGeometry> {
    if !finite_point(start) || !finite_point(end) || !offset.is_finite() || offset == 0.0 {
        return None;
    }
    let delta = Point2::new(end.x - start.x, end.y - start.y);
    let measurement = delta.x.hypot(delta.y);
    if !measurement.is_finite() || measurement <= f64::EPSILON {
        return None;
    }
    let unit_direction = Point2::new(delta.x / measurement, delta.y / measurement);
    let normal_direction = Point2::new(-unit_direction.y, unit_direction.x);
    let offset_vector = Point2::new(normal_direction.x * offset, normal_direction.y * offset);
    let dimension_start = Point2::new(start.x + offset_vector.x, start.y + offset_vector.y);
    let dimension_end = Point2::new(end.x + offset_vector.x, end.y + offset_vector.y);
    Some(AlignedDimensionGeometry {
        start,
        end,
        dimension_start,
        dimension_end,
        dimension_midpoint: midpoint(dimension_start, dimension_end),
        unit_direction,
        normal_direction,
        measurement,
    })
}

pub fn aligned_dimension_offset(start: Point2, end: Point2, line_point: Point2) -> Option<f64> {
    if !finite_point(line_point) {
        return None;
    }
    let delta = Point2::new(end.x - start.x, end.y - start.y);
    let measurement = delta.x.hypot(delta.y);
    if !measurement.is_finite() || measurement <= f64::EPSILON {
        return None;
    }
    let normal = Point2::new(-delta.y / measurement, delta.x / measurement);
    let offset = (line_point.x - start.x).mul_add(normal.x, (line_point.y - start.y) * normal.y);
    offset.is_finite().then_some(offset)
}

pub fn format_dimension_text(
    measurement: f64,
    units: Units,
    text_override: Option<&str>,
) -> String {
    let value = match units {
        Units::Millimeters => format!("{measurement:.2}"),
        Units::Meters | Units::Inches => format!("{measurement:.3}"),
    };
    text_override.map_or_else(|| value.clone(), |template| template.replace("<>", &value))
}

pub(crate) fn bounds_from_points(points: impl IntoIterator<Item = Point2>) -> Bounds2 {
    let mut points = points.into_iter();
    let first = points
        .next()
        .expect("bounds requested for non-empty points");
    let mut bounds = Bounds2::from_point(first);
    for point in points {
        bounds.include_point(point);
    }
    bounds
}

pub(crate) fn finite_point(point: Point2) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

pub(crate) fn distance(left: Point2, right: Point2) -> f64 {
    (left.x - right.x).hypot(left.y - right.y)
}

pub(crate) fn midpoint(left: Point2, right: Point2) -> Point2 {
    Point2::new((left.x + right.x) * 0.5, (left.y + right.y) * 0.5)
}

pub(crate) fn arc_point(center: Point2, radius: f64, angle: f64) -> Point2 {
    Point2::new(
        angle.cos().mul_add(radius, center.x),
        angle.sin().mul_add(radius, center.y),
    )
}

pub(crate) fn angle_on_arc(angle: f64, start_angle: f64, sweep_angle: f64) -> bool {
    (angle - start_angle).rem_euclid(std::f64::consts::TAU) <= sweep_angle + 1.0e-12
}

pub(crate) fn distance_to_arc(
    point: Point2,
    center: Point2,
    radius: f64,
    start_angle: f64,
    sweep_angle: f64,
) -> f64 {
    let radial_distance = distance(center, point);
    let point_angle = (point.y - center.y).atan2(point.x - center.x);
    if angle_on_arc(point_angle, start_angle, sweep_angle) {
        (radial_distance - radius).abs()
    } else {
        distance(point, arc_point(center, radius, start_angle)).min(distance(
            point,
            arc_point(center, radius, start_angle + sweep_angle),
        ))
    }
}

pub(crate) fn arc_snap_points(
    center: Point2,
    radius: f64,
    start_angle: f64,
    sweep_angle: f64,
) -> Vec<(SnapKind, Point2)> {
    let mut points = vec![
        (SnapKind::Vertex, arc_point(center, radius, start_angle)),
        (
            SnapKind::Vertex,
            arc_point(center, radius, start_angle + sweep_angle),
        ),
        (
            SnapKind::Midpoint,
            arc_point(center, radius, start_angle + sweep_angle * 0.5),
        ),
        (SnapKind::Center, center),
    ];
    for angle in [
        0.0,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
        std::f64::consts::PI + std::f64::consts::FRAC_PI_2,
    ] {
        if angle_on_arc(angle, start_angle, sweep_angle) {
            points.push((SnapKind::Quadrant, arc_point(center, radius, angle)));
        }
    }
    points
}

pub(crate) fn polyline_snap_points(points: &[Point2], closed: bool) -> Vec<(SnapKind, Point2)> {
    let mut candidates = points
        .iter()
        .copied()
        .map(|point| (SnapKind::Vertex, point))
        .collect::<Vec<_>>();
    for segment in points.windows(2) {
        candidates.push((SnapKind::Midpoint, midpoint(segment[0], segment[1])));
    }
    if closed && points.len() > 2 {
        candidates.push((
            SnapKind::Midpoint,
            midpoint(points[points.len() - 1], points[0]),
        ));
    }
    candidates
}

pub(crate) fn compare_snap_hits(left: &SnapHit, right: &SnapHit) -> std::cmp::Ordering {
    left.distance
        .total_cmp(&right.distance)
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.entity_id.cmp(&right.entity_id))
        .then_with(|| left.point.x.total_cmp(&right.point.x))
        .then_with(|| left.point.y.total_cmp(&right.point.y))
}

pub(crate) fn distance_to_segment(point: Point2, start: Point2, end: Point2) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx.mul_add(dx, dy * dy);
    if length_squared <= f64::EPSILON {
        return distance(point, start);
    }
    let projection = ((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared;
    let projection = projection.clamp(0.0, 1.0);
    distance(
        point,
        Point2::new(start.x + projection * dx, start.y + projection * dy),
    )
}

pub(crate) fn distance_to_polyline(point: Point2, points: &[Point2], closed: bool) -> Option<f64> {
    let mut distances = points
        .windows(2)
        .map(|segment| distance_to_segment(point, segment[0], segment[1]));
    let first = distances.next()?;
    let mut closest = first;
    for distance in distances {
        closest = closest.min(distance);
    }
    if closed && points.len() > 2 {
        closest = closest.min(distance_to_segment(
            point,
            points[points.len() - 1],
            points[0],
        ));
    }
    Some(closest)
}

pub(crate) fn distance_to_closed_polyline(point: Point2, points: &[Point2]) -> f64 {
    distance_to_polyline(point, points, true).expect("closed polyline has four points")
}

pub(crate) fn point_in_polygon(point: Point2, points: &[Point2]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = points[points.len() - 1];
    for current in points {
        let intersects = (current.y > point.y) != (previous.y > point.y)
            && point.x
                < (previous.x - current.x) * (point.y - current.y) / (previous.y - current.y)
                    + current.x;
        if intersects {
            inside = !inside;
        }
        previous = *current;
    }
    inside
}
