//! Stable BOM contracts independent of feature-tree implementation details.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BomSource {
    pub part_number: String,
    pub description: String,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BomItem {
    pub part_number: String,
    pub description: String,
    pub quantity: u32,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[must_use]
pub fn generate(sources: impl IntoIterator<Item = BomSource>) -> Vec<BomItem> {
    let mut grouped = BTreeMap::<String, BomItem>::new();
    for source in sources {
        let entry = grouped
            .entry(source.part_number.clone())
            .or_insert_with(|| BomItem {
                part_number: source.part_number,
                description: source.description,
                quantity: 0,
                material: source.material,
                revision: source.revision,
            });
        entry.quantity = entry.quantity.saturating_add(1);
    }
    grouped.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_repeated_parts_deterministically() {
        let items = generate([
            BomSource {
                part_number: "B".into(),
                description: "Bolt".into(),
                material: None,
                revision: None,
            },
            BomSource {
                part_number: "A".into(),
                description: "Plate".into(),
                material: None,
                revision: None,
            },
            BomSource {
                part_number: "B".into(),
                description: "Bolt".into(),
                material: None,
                revision: None,
            },
        ]);
        assert_eq!(items[0].part_number, "A");
        assert_eq!(items[1].quantity, 2);
    }
}
