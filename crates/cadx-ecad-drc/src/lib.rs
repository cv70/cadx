//! Deterministic electrical design-rule checks for PCB layouts.

use cadx_ecad_layout::PcbBoard;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrcSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrcIssue {
    pub code: String,
    pub severity: DrcSeverity,
    pub message: String,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub location_mm: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DrcReport {
    pub issues: Vec<DrcIssue>,
    pub checked_components: usize,
    pub checked_traces: usize,
    pub checked_pads: usize,
    pub checked_vias: usize,
}

#[must_use]
pub fn run(board: &PcbBoard) -> DrcReport {
    let mut report = DrcReport {
        checked_components: board.components.len(),
        checked_traces: board.traces.len(),
        checked_pads: board.pads.len(),
        checked_vias: board.vias.len(),
        ..DrcReport::default()
    };
    if let Err(error) = board.validate() {
        report.issues.push(DrcIssue {
            code: "BOARD_INVALID".into(),
            severity: DrcSeverity::Error,
            message: error.to_string(),
            reference: None,
            location_mm: None,
        });
        return report;
    }
    for component in &board.components {
        let edge_gap_x = (component.position_mm[0] - component.size_mm[0] * 0.5)
            .min(board.width_mm - component.position_mm[0] - component.size_mm[0] * 0.5);
        let edge_gap_y = (component.position_mm[1] - component.size_mm[1] * 0.5)
            .min(board.height_mm - component.position_mm[1] - component.size_mm[1] * 0.5);
        if edge_gap_x.min(edge_gap_y) < board.rules.edge_clearance_mm {
            report.issues.push(DrcIssue {
                code: "EDGE_CLEARANCE".into(),
                severity: DrcSeverity::Warning,
                message: format!("{} is inside the edge clearance rule", component.reference),
                reference: Some(component.reference.clone()),
                location_mm: Some(component.position_mm),
            });
        }
    }
    for (index, trace) in board.traces.iter().enumerate() {
        if trace.width_mm < board.rules.min_trace_width_mm {
            report.issues.push(DrcIssue {
                code: "TRACE_WIDTH".into(),
                severity: DrcSeverity::Error,
                message: format!("Trace {index} is narrower than the rule"),
                reference: Some(trace.net.clone()),
                location_mm: trace.points_mm.first().copied(),
            });
        }
        if !board.layers.iter().any(|layer| layer.name == trace.layer) {
            report.issues.push(DrcIssue {
                code: "UNKNOWN_LAYER".into(),
                severity: DrcSeverity::Error,
                message: format!("Trace {index} references an unknown layer"),
                reference: Some(trace.net.clone()),
                location_mm: trace.points_mm.first().copied(),
            });
        }
        if !board.nets.iter().any(|net| net.name == trace.net) {
            report.issues.push(DrcIssue {
                code: "UNKNOWN_NET".into(),
                severity: DrcSeverity::Warning,
                message: format!("Trace {index} references an undeclared net"),
                reference: Some(trace.net.clone()),
                location_mm: trace.points_mm.first().copied(),
            });
        }
        if trace.points_mm.iter().any(|point| {
            point[0] < board.rules.edge_clearance_mm
                || point[1] < board.rules.edge_clearance_mm
                || board.width_mm - point[0] < board.rules.edge_clearance_mm
                || board.height_mm - point[1] < board.rules.edge_clearance_mm
        }) {
            report.issues.push(DrcIssue {
                code: "TRACE_EDGE_CLEARANCE".into(),
                severity: DrcSeverity::Warning,
                message: format!("Trace {index} enters the board edge clearance"),
                reference: Some(trace.net.clone()),
                location_mm: trace.points_mm.first().copied(),
            });
        }
    }
    for (first_index, first) in board.traces.iter().enumerate() {
        for (second_index, second) in board.traces.iter().enumerate().skip(first_index + 1) {
            if first.layer != second.layer || first.net == second.net {
                continue;
            }
            let required = board.rules.min_clearance_mm + (first.width_mm + second.width_mm) * 0.5;
            if trace_distance(first, second) < required {
                report.issues.push(DrcIssue {
                    code: "TRACE_CLEARANCE".into(),
                    severity: DrcSeverity::Error,
                    message: format!(
                        "Traces {first_index} and {second_index} violate copper clearance"
                    ),
                    reference: Some(format!("{} / {}", first.net, second.net)),
                    location_mm: first.points_mm.first().copied(),
                });
            }
        }
    }
    for (index, pad) in board.pads.iter().enumerate() {
        if pad
            .drill_mm
            .is_some_and(|diameter| diameter < board.rules.min_hole_mm)
        {
            report.issues.push(DrcIssue {
                code: "PAD_DRILL".into(),
                severity: DrcSeverity::Error,
                message: format!("Pad {index} drill is below the minimum hole rule"),
                reference: Some(format!("{}.{}", pad.reference, pad.number)),
                location_mm: Some(pad.position_mm),
            });
        }
    }
    for (index, via) in board.vias.iter().enumerate() {
        if via.drill_mm < board.rules.min_hole_mm {
            report.issues.push(DrcIssue {
                code: "VIA_DRILL".into(),
                severity: DrcSeverity::Error,
                message: format!("Via {index} drill is below the minimum hole rule"),
                reference: Some(via.net.clone()),
                location_mm: Some(via.position_mm),
            });
        }
        if (via.diameter_mm - via.drill_mm) * 0.5 < board.rules.min_clearance_mm * 0.5 {
            report.issues.push(DrcIssue {
                code: "VIA_ANNULAR_RING".into(),
                severity: DrcSeverity::Warning,
                message: format!("Via {index} annular ring is below the review threshold"),
                reference: Some(via.net.clone()),
                location_mm: Some(via.position_mm),
            });
        }
    }
    for (first_index, first) in board.components.iter().enumerate() {
        for second in board.components.iter().skip(first_index + 1) {
            let dx = (first.position_mm[0] - second.position_mm[0]).abs()
                - (first.size_mm[0] + second.size_mm[0]) * 0.5;
            let dy = (first.position_mm[1] - second.position_mm[1]).abs()
                - (first.size_mm[1] + second.size_mm[1]) * 0.5;
            if dx.max(dy) < board.rules.min_clearance_mm {
                report.issues.push(DrcIssue {
                    code: "COMPONENT_CLEARANCE".into(),
                    severity: DrcSeverity::Error,
                    message: format!(
                        "{} and {} violate component clearance",
                        first.reference, second.reference
                    ),
                    reference: Some(format!("{} / {}", first.reference, second.reference)),
                    location_mm: Some([
                        (first.position_mm[0] + second.position_mm[0]) * 0.5,
                        (first.position_mm[1] + second.position_mm[1]) * 0.5,
                    ]),
                });
            }
        }
    }
    report
}

fn trace_distance(first: &cadx_ecad_layout::PcbTrace, second: &cadx_ecad_layout::PcbTrace) -> f64 {
    first
        .points_mm
        .windows(2)
        .flat_map(|first_segment| {
            second.points_mm.windows(2).map(move |second_segment| {
                segment_distance(
                    first_segment[0],
                    first_segment[1],
                    second_segment[0],
                    second_segment[1],
                )
            })
        })
        .fold(f64::INFINITY, f64::min)
}

fn segment_distance(first: [f64; 2], second: [f64; 2], third: [f64; 2], fourth: [f64; 2]) -> f64 {
    if segments_intersect(first, second, third, fourth) {
        return 0.0;
    }
    point_segment_distance(first, third, fourth)
        .min(point_segment_distance(second, third, fourth))
        .min(point_segment_distance(third, first, second))
        .min(point_segment_distance(fourth, first, second))
}

fn point_segment_distance(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> f64 {
    let vector = [end[0] - start[0], end[1] - start[1]];
    let length_squared = vector[0].mul_add(vector[0], vector[1] * vector[1]);
    if length_squared <= f64::EPSILON {
        return point_distance(point, start);
    }
    let projection = (((point[0] - start[0]) * vector[0] + (point[1] - start[1]) * vector[1])
        / length_squared)
        .clamp(0.0, 1.0);
    point_distance(
        point,
        [
            vector[0].mul_add(projection, start[0]),
            vector[1].mul_add(projection, start[1]),
        ],
    )
}

fn point_distance(first: [f64; 2], second: [f64; 2]) -> f64 {
    (first[0] - second[0]).hypot(first[1] - second[1])
}

fn segments_intersect(
    first: [f64; 2],
    second: [f64; 2],
    third: [f64; 2],
    fourth: [f64; 2],
) -> bool {
    let bounds_overlap = first[0].min(second[0]) <= third[0].max(fourth[0])
        && third[0].min(fourth[0]) <= first[0].max(second[0])
        && first[1].min(second[1]) <= third[1].max(fourth[1])
        && third[1].min(fourth[1]) <= first[1].max(second[1]);
    if !bounds_overlap {
        return false;
    }
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]).mul_add(c[1] - a[1], -(b[1] - a[1]) * (c[0] - a[0]))
    };
    let first_side = cross(first, second, third);
    let second_side = cross(first, second, fourth);
    let third_side = cross(third, fourth, first);
    let fourth_side = cross(third, fourth, second);
    first_side * second_side <= 0.0 && third_side * fourth_side <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_trace_is_an_error() {
        let mut board = PcbBoard::demo();
        board.traces.push(cadx_ecad_layout::PcbTrace {
            net: "GND".into(),
            layer: "F.Cu".into(),
            width_mm: 0.05,
            points_mm: vec![[10.0, 10.0], [20.0, 10.0]],
        });
        assert!(
            run(&board)
                .issues
                .iter()
                .any(|issue| issue.code == "TRACE_WIDTH")
        );
    }

    #[test]
    fn trace_clearance_distinguishes_close_and_disjoint_collinear_segments() {
        let mut board = PcbBoard::demo();
        board.components.clear();
        board.nets.push(cadx_ecad_layout::PcbNet {
            name: "VCC".into(),
            class: "POWER".into(),
            pins: Vec::new(),
            impedance_ohms: None,
        });
        board.traces = vec![
            cadx_ecad_layout::PcbTrace {
                net: "GND".into(),
                layer: "F.Cu".into(),
                width_mm: 0.2,
                points_mm: vec![[5.0, 10.0], [15.0, 10.0]],
            },
            cadx_ecad_layout::PcbTrace {
                net: "VCC".into(),
                layer: "F.Cu".into(),
                width_mm: 0.2,
                points_mm: vec![[30.0, 10.0], [40.0, 10.0]],
            },
        ];
        assert!(
            !run(&board)
                .issues
                .iter()
                .any(|issue| issue.code == "TRACE_CLEARANCE")
        );

        board.traces[1].points_mm = vec![[5.0, 10.1], [15.0, 10.1]];
        assert!(
            run(&board)
                .issues
                .iter()
                .any(|issue| issue.code == "TRACE_CLEARANCE")
        );
    }
}
