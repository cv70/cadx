//! Shared exact geometry fixtures used by the sketch unit tests.

use crate::geometry::{SketchLoop2D, SketchSegment2D};

pub(crate) fn circle(center: [f64; 2], radius: f64, ccw: bool) -> SketchLoop2D {
    let right = [center[0] + radius, center[1]];
    let left = [center[0] - radius, center[1]];
    let segments = if ccw {
        vec![
            SketchSegment2D::Arc {
                start: right,
                end: left,
                center,
                ccw: true,
            },
            SketchSegment2D::Arc {
                start: left,
                end: right,
                center,
                ccw: true,
            },
        ]
    } else {
        vec![
            SketchSegment2D::Arc {
                start: right,
                end: left,
                center,
                ccw: false,
            },
            SketchSegment2D::Arc {
                start: left,
                end: right,
                center,
                ccw: false,
            },
        ]
    };
    SketchLoop2D { segments }
}
