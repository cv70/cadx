//! Shared exact scalar, planar vector, and closest-point helpers.

use std::f64::consts::{PI, TAU};

use crate::{
    GEOMETRY_EPSILON,
    curves::{CurveDerivatives2D, CurveError},
};

pub(crate) fn arc_sweep(start: [f64; 2], end: [f64; 2], center: [f64; 2], ccw: bool) -> f64 {
    let start_angle = (start[1] - center[1]).atan2(start[0] - center[0]);
    let end_angle = (end[1] - center[1]).atan2(end[0] - center[0]);
    if ccw {
        (end_angle - start_angle).rem_euclid(TAU)
    } else {
        -(start_angle - end_angle).rem_euclid(TAU)
    }
}

pub(crate) fn point_segment_distance_squared(
    point: [f64; 2],
    start: [f64; 2],
    end: [f64; 2],
) -> f64 {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let length_squared = dot(delta, delta);
    let factor = if length_squared > f64::EPSILON {
        (dot([point[0] - start[0], point[1] - start[1]], delta) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    squared_distance(
        point,
        [
            delta[0].mul_add(factor, start[0]),
            delta[1].mul_add(factor, start[1]),
        ],
    )
}

pub(crate) fn sampled_curve_length(points: &[[f64; 2]]) -> f64 {
    points
        .windows(2)
        .map(|pair| distance(pair[0], pair[1]))
        .sum()
}

pub(crate) fn deduplicate_points(points: &mut Vec<[f64; 2]>) {
    let mut unique = Vec::with_capacity(points.len());
    for point in points.drain(..) {
        if unique
            .iter()
            .all(|other| distance(point, *other) > GEOMETRY_EPSILON)
        {
            unique.push(point);
        }
    }
    *points = unique;
}

pub(crate) fn squared_distance(first: [f64; 2], second: [f64; 2]) -> f64 {
    (first[0] - second[0]).mul_add(
        first[0] - second[0],
        (first[1] - second[1]) * (first[1] - second[1]),
    )
}

pub(crate) fn distance(first: [f64; 2], second: [f64; 2]) -> f64 {
    squared_distance(first, second).sqrt()
}

pub(crate) fn dot(first: [f64; 2], second: [f64; 2]) -> f64 {
    first[0].mul_add(second[0], first[1] * second[1])
}

pub(crate) fn cross(first: [f64; 2], second: [f64; 2]) -> f64 {
    first[0].mul_add(second[1], -first[1] * second[0])
}

pub(crate) fn closest_point_on_line_segment(
    point: [f64; 2],
    start: [f64; 2],
    end: [f64; 2],
) -> [f64; 2] {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let length_squared = dot(delta, delta);
    let factor = if length_squared > f64::EPSILON {
        (dot([point[0] - start[0], point[1] - start[1]], delta) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [
        delta[0].mul_add(factor, start[0]),
        delta[1].mul_add(factor, start[1]),
    ]
}

pub(crate) fn closest_point_on_parametric_curve(
    point: [f64; 2],
    derivatives: impl Fn(f64) -> Result<CurveDerivatives2D, CurveError>,
) -> Option<[f64; 2]> {
    const SEED_COUNT: u32 = 32;
    const MAX_ITERATIONS: usize = 24;
    let mut best = None;
    let mut best_distance = f64::INFINITY;
    for seed in 0..=SEED_COUNT {
        let mut parameter = f64::from(seed) / f64::from(SEED_COUNT);
        for _ in 0..MAX_ITERATIONS {
            let value = derivatives(parameter).ok()?;
            let offset = [value.point[0] - point[0], value.point[1] - point[1]];
            let gradient = dot(offset, value.first);
            let hessian = dot(value.first, value.first) + dot(offset, value.second);
            if !gradient.is_finite() || !hessian.is_finite() || hessian.abs() <= f64::EPSILON {
                break;
            }
            let next = (parameter - gradient / hessian).clamp(0.0, 1.0);
            if (next - parameter).abs() <= 1.0e-13 {
                parameter = next;
                break;
            }
            parameter = next;
        }
        let candidate = derivatives(parameter).ok()?.point;
        let candidate_distance = squared_distance(point, candidate);
        if candidate_distance < best_distance {
            best = Some(candidate);
            best_distance = candidate_distance;
        }
    }
    best
}

pub(crate) fn normalize_vector(vector: [f64; 2]) -> [f64; 2] {
    let length = vector[0].hypot(vector[1]).max(GEOMETRY_EPSILON);
    [vector[0] / length, vector[1] / length]
}

pub(crate) fn normalize_angle(angle: f64) -> f64 {
    (angle + PI).rem_euclid(TAU) - PI
}
