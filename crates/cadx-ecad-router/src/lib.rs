//! Deterministic orthogonal routing proposals for ECAD layouts.

use cadx_ecad_layout::{PcbBoard, PcbTrace};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRequest {
    pub net: String,
    pub layer: String,
    pub width_mm: f64,
    pub start_mm: [f64; 2],
    pub end_mm: [f64; 2],
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RouterError {
    #[error("route geometry or width is invalid")]
    InvalidRequest,
    #[error("net {0} is not declared")]
    UnknownNet(String),
    #[error("layer {0} is not a copper layer")]
    UnknownLayer(String),
    #[error("no orthogonal path satisfies component and keepout clearances")]
    NoRoute,
}

/// Routes the shorter of two legal Manhattan paths. Existing traces remain a
/// DRC concern; components and keepouts are treated as hard obstacles.
///
/// # Errors
///
/// Returns a request, net, layer, or no-route error.
pub fn route(board: &PcbBoard, request: &RouteRequest) -> Result<PcbTrace, RouterError> {
    let inside = |point: [f64; 2]| {
        point.iter().all(|value| value.is_finite())
            && point[0] >= 0.0
            && point[0] <= board.width_mm
            && point[1] >= 0.0
            && point[1] <= board.height_mm
    };
    if !inside(request.start_mm)
        || !inside(request.end_mm)
        || !request.width_mm.is_finite()
        || request.width_mm < board.rules.min_trace_width_mm
        || point_distance(request.start_mm, request.end_mm) <= f64::EPSILON
    {
        return Err(RouterError::InvalidRequest);
    }
    if !board.nets.iter().any(|net| net.name == request.net) {
        return Err(RouterError::UnknownNet(request.net.clone()));
    }
    if !board.layers.iter().any(|layer| {
        layer.name == request.layer && matches!(layer.kind, cadx_ecad_layout::LayerKind::Copper)
    }) {
        return Err(RouterError::UnknownLayer(request.layer.clone()));
    }

    let horizontal_first = vec![
        request.start_mm,
        [request.end_mm[0], request.start_mm[1]],
        request.end_mm,
    ];
    let vertical_first = vec![
        request.start_mm,
        [request.start_mm[0], request.end_mm[1]],
        request.end_mm,
    ];
    let clearance = board.rules.min_clearance_mm + request.width_mm * 0.5;
    let mut candidates = [horizontal_first, vertical_first]
        .into_iter()
        .filter(|points| path_is_clear(board, points, clearance, &request.layer))
        .collect::<Vec<_>>();
    candidates.sort_by(|first, second| {
        path_length(first)
            .total_cmp(&path_length(second))
            .then_with(|| first[1][0].total_cmp(&second[1][0]))
    });
    let points_mm = candidates.into_iter().next().ok_or(RouterError::NoRoute)?;
    Ok(PcbTrace {
        net: request.net.clone(),
        layer: request.layer.clone(),
        width_mm: request.width_mm,
        points_mm: simplify(points_mm),
    })
}

#[must_use]
pub fn differential_pair(
    center: &PcbTrace,
    positive_net: &str,
    negative_net: &str,
    gap_mm: f64,
) -> Option<[PcbTrace; 2]> {
    if !gap_mm.is_finite() || gap_mm <= 0.0 || center.points_mm.len() < 2 {
        return None;
    }
    let offset = (gap_mm + center.width_mm) * 0.5;
    let first_segment = [
        center.points_mm[1][0] - center.points_mm[0][0],
        center.points_mm[1][1] - center.points_mm[0][1],
    ];
    let normal = if first_segment[0].abs() >= first_segment[1].abs() {
        [0.0, offset]
    } else {
        [offset, 0.0]
    };
    let shifted = |sign: f64, net: &str| PcbTrace {
        net: net.into(),
        layer: center.layer.clone(),
        width_mm: center.width_mm,
        points_mm: center
            .points_mm
            .iter()
            .map(|point| [point[0] + normal[0] * sign, point[1] + normal[1] * sign])
            .collect(),
    };
    Some([shifted(1.0, positive_net), shifted(-1.0, negative_net)])
}

fn path_is_clear(board: &PcbBoard, points: &[[f64; 2]], clearance: f64, layer: &str) -> bool {
    points.windows(2).all(|segment| {
        board.components.iter().all(|component| {
            let half = [
                component.size_mm[0] * 0.5 + clearance,
                component.size_mm[1] * 0.5 + clearance,
            ];
            !segment_intersects_rect(
                segment[0],
                segment[1],
                [
                    component.position_mm[0] - half[0],
                    component.position_mm[1] - half[1],
                ],
                [
                    component.position_mm[0] + half[0],
                    component.position_mm[1] + half[1],
                ],
            )
        }) && board.keepouts.iter().all(|keepout| {
            if !keepout.layers.is_empty()
                && !keepout
                    .layers
                    .iter()
                    .any(|keepout_layer| keepout_layer == "*" || keepout_layer == layer)
            {
                return true;
            }
            let half = [
                keepout.size_mm[0] * 0.5 + clearance,
                keepout.size_mm[1] * 0.5 + clearance,
            ];
            !segment_intersects_rect(
                segment[0],
                segment[1],
                [
                    keepout.center_mm[0] - half[0],
                    keepout.center_mm[1] - half[1],
                ],
                [
                    keepout.center_mm[0] + half[0],
                    keepout.center_mm[1] + half[1],
                ],
            )
        })
    })
}

fn segment_intersects_rect(
    first: [f64; 2],
    second: [f64; 2],
    minimum: [f64; 2],
    maximum: [f64; 2],
) -> bool {
    if (first[0] - second[0]).abs() <= f64::EPSILON {
        first[0] >= minimum[0]
            && first[0] <= maximum[0]
            && first[1].min(second[1]) <= maximum[1]
            && first[1].max(second[1]) >= minimum[1]
    } else {
        first[1] >= minimum[1]
            && first[1] <= maximum[1]
            && first[0].min(second[0]) <= maximum[0]
            && first[0].max(second[0]) >= minimum[0]
    }
}

fn path_length(points: &[[f64; 2]]) -> f64 {
    points
        .windows(2)
        .map(|segment| {
            (segment[1][0] - segment[0][0]).abs() + (segment[1][1] - segment[0][1]).abs()
        })
        .sum()
}

fn simplify(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.dedup();
    points
}

fn point_distance(first: [f64; 2], second: [f64; 2]) -> f64 {
    (first[0] - second[0]).hypot(first[1] - second[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_creates_a_legal_manhattan_trace() {
        let mut board = PcbBoard::demo();
        board.components.clear();
        let trace = route(
            &board,
            &RouteRequest {
                net: "GND".into(),
                layer: "F.Cu".into(),
                width_mm: 0.2,
                start_mm: [2.0, 2.0],
                end_mm: [30.0, 20.0],
            },
        )
        .unwrap();
        assert_eq!(trace.points_mm.len(), 3);
        assert_eq!(trace.net, "GND");
    }

    #[test]
    fn component_obstacles_can_block_both_paths() {
        let mut board = PcbBoard::demo();
        board.components[0].position_mm = [20.0, 20.0];
        board.components[0].size_mm = [40.0, 40.0];
        let result = route(
            &board,
            &RouteRequest {
                net: "GND".into(),
                layer: "F.Cu".into(),
                width_mm: 0.2,
                start_mm: [1.0, 1.0],
                end_mm: [30.0, 30.0],
            },
        );
        assert_eq!(result, Err(RouterError::NoRoute));
    }
}
