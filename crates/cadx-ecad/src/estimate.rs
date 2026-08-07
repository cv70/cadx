//! Routing completeness estimate derived from a PCB layout.

use crate::layout;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutingEstimate {
    pub declared_net_count: usize,
    pub routeable_layer_count: usize,
    pub unrouted_pin_count: usize,
    pub estimated_segment_count: usize,
}

#[must_use]
pub fn routing_estimate(board: &layout::PcbBoard) -> RoutingEstimate {
    let routeable_layer_count = board
        .layers
        .iter()
        .filter(|layer| matches!(layer.kind, layout::LayerKind::Copper))
        .count();
    let routed_pins = board.traces.len().saturating_mul(2);
    let declared_pins = board.nets.iter().map(|net| net.pins.len()).sum::<usize>();
    RoutingEstimate {
        declared_net_count: board.nets.len(),
        routeable_layer_count,
        unrouted_pin_count: declared_pins.saturating_sub(routed_pins),
        estimated_segment_count: board.traces.len().saturating_add(declared_pins / 2),
    }
}

#[cfg(test)]
mod tests {
    use super::routing_estimate;
    use crate::{default_net_classes, footprint_library};
    use cadx_ecad_layout::PcbBoard;

    #[test]
    fn footprint_library_and_routing_estimate_are_deterministic() {
        assert_eq!(footprint_library()[0].package, "QFN-32");
        assert_eq!(default_net_classes()[2].name, "USB_HS");

        let estimate = routing_estimate(&PcbBoard::demo());
        assert_eq!(estimate.declared_net_count, 1);
        assert_eq!(estimate.routeable_layer_count, 4);
        assert_eq!(estimate.unrouted_pin_count, 1);
    }
}
