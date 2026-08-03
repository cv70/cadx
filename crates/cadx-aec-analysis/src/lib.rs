//! Deterministic broad-phase AEC clash and coordination checks.

use cadx_aec_bim::{BimModel, Bounds3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClashIssue {
    pub first_element_id: String,
    pub second_element_id: String,
    pub overlap_mm: [f64; 3],
    pub overlap_volume_mm3: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClashReport {
    pub checked_pairs: usize,
    pub clashes: Vec<ClashIssue>,
}

#[must_use]
pub fn detect_clashes(model: &BimModel, tolerance_mm: f64) -> ClashReport {
    let tolerance = if tolerance_mm.is_finite() {
        tolerance_mm.max(0.0)
    } else {
        0.0
    };
    let bounded = model
        .elements
        .iter()
        .filter_map(|element| element.bounds.map(|bounds| (element, bounds)))
        .collect::<Vec<_>>();
    let mut report = ClashReport::default();
    for (index, (first, first_bounds)) in bounded.iter().enumerate() {
        for (second, second_bounds) in bounded.iter().skip(index + 1) {
            report.checked_pairs += 1;
            if let Some(overlap) = overlap(*first_bounds, *second_bounds, tolerance) {
                report.clashes.push(ClashIssue {
                    first_element_id: first.id.clone(),
                    second_element_id: second.id.clone(),
                    overlap_mm: overlap,
                    overlap_volume_mm3: overlap.iter().product(),
                });
            }
        }
    }
    report
}

fn overlap(first: Bounds3, second: Bounds3, tolerance: f64) -> Option<[f64; 3]> {
    let overlap = std::array::from_fn(|axis| {
        first.maximum_mm[axis].min(second.maximum_mm[axis])
            - first.minimum_mm[axis].max(second.minimum_mm[axis])
    });
    overlap
        .iter()
        .all(|value| *value > tolerance)
        .then_some(overlap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_aec_bim::{BimElement, BimElementClass};

    #[test]
    fn overlapping_elements_are_reported_once() {
        let mut model = BimModel::default();
        for (id, minimum, maximum) in [("wall", [0.0; 3], [10.0; 3]), ("duct", [5.0; 3], [12.0; 3])]
        {
            model.elements.push(BimElement {
                id: id.into(),
                name: id.into(),
                class: BimElementClass::Proxy,
                storey_id: "level-1".into(),
                attributes: Vec::new(),
                bounds: Some(Bounds3 {
                    minimum_mm: minimum,
                    maximum_mm: maximum,
                }),
                linked_feature_id: None,
            });
        }
        let report = detect_clashes(&model, 0.1);
        assert_eq!(report.checked_pairs, 1);
        assert!((report.clashes[0].overlap_volume_mm3 - 125.0).abs() < f64::EPSILON);
    }
}
