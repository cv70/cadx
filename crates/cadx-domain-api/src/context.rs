//! Read-only host snapshot handed to domain packs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DomainContext {
    pub document_name: String,
    pub selected_feature_ids: Vec<u64>,
    pub visible_solid_count: usize,
    pub active_feature_count: usize,
    pub selected_feature_name: Option<String>,
    #[serde(default)]
    pub spatial_entities: Vec<DomainSpatialEntity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainSpatialEntity {
    pub feature_id: u64,
    pub name: String,
    pub minimum_mm: [f64; 3],
    pub maximum_mm: [f64; 3],
}
