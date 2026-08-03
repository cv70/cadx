//! Validated, kernel-neutral building information model contracts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BimElementClass {
    Project,
    Site,
    Building,
    Storey,
    Wall,
    Slab,
    Opening,
    Space,
    Door,
    Window,
    Column,
    Beam,
    Roof,
    Stair,
    DistributionElement,
    Proxy,
}

impl BimElementClass {
    #[must_use]
    pub const fn ifc_name(self) -> &'static str {
        match self {
            Self::Project => "IFCPROJECT",
            Self::Site => "IFCSITE",
            Self::Building => "IFCBUILDING",
            Self::Storey => "IFCBUILDINGSTOREY",
            Self::Wall => "IFCWALL",
            Self::Slab => "IFCSLAB",
            Self::Opening => "IFCOPENINGELEMENT",
            Self::Space => "IFCSPACE",
            Self::Door => "IFCDOOR",
            Self::Window => "IFCWINDOW",
            Self::Column => "IFCCOLUMN",
            Self::Beam => "IFCBEAM",
            Self::Roof => "IFCROOF",
            Self::Stair => "IFCSTAIR",
            Self::DistributionElement => "IFCDISTRIBUTIONELEMENT",
            Self::Proxy => "IFCBUILDINGELEMENTPROXY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum BimValue {
    Text(String),
    Boolean(bool),
    Integer(i64),
    Number(f64),
}

impl BimValue {
    #[must_use]
    pub fn display_value(&self) -> String {
        match self {
            Self::Text(value) => value.clone(),
            Self::Boolean(value) => value.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BimAttribute {
    pub name: String,
    pub value: BimValue,
    #[serde(default)]
    pub property_set: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds3 {
    pub minimum_mm: [f64; 3],
    pub maximum_mm: [f64; 3],
}

impl Bounds3 {
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.minimum_mm
            .iter()
            .chain(self.maximum_mm.iter())
            .all(|value| value.is_finite())
            && (0..3).all(|axis| self.minimum_mm[axis] <= self.maximum_mm[axis])
    }

    #[must_use]
    pub fn size_mm(self) -> [f64; 3] {
        std::array::from_fn(|axis| self.maximum_mm[axis] - self.minimum_mm[axis])
    }

    #[must_use]
    pub fn volume_mm3(self) -> f64 {
        let [x, y, z] = self.size_mm();
        x * y * z
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BimElement {
    pub id: String,
    pub name: String,
    pub class: BimElementClass,
    pub storey_id: String,
    #[serde(default)]
    pub attributes: Vec<BimAttribute>,
    #[serde(default)]
    pub bounds: Option<Bounds3>,
    #[serde(default)]
    pub linked_feature_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingStorey {
    pub id: String,
    pub name: String,
    pub elevation_mm: f64,
    pub height_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BimModel {
    pub project_id: String,
    pub project_name: String,
    #[serde(default)]
    pub storeys: Vec<BuildingStorey>,
    #[serde(default)]
    pub elements: Vec<BimElement>,
}

impl Default for BimModel {
    fn default() -> Self {
        Self {
            project_id: "cadx-project".into(),
            project_name: "CADX project".into(),
            storeys: vec![BuildingStorey {
                id: "level-1".into(),
                name: "Level 1".into(),
                elevation_mm: 0.0,
                height_mm: 2_800.0,
            }],
            elements: Vec::new(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BimError {
    #[error("project identity must not be empty")]
    EmptyProject,
    #[error("storey id {0} occurs more than once")]
    DuplicateStorey(String),
    #[error("storey {0} has invalid elevation or height")]
    InvalidStorey(String),
    #[error("element id {0} occurs more than once")]
    DuplicateElement(String),
    #[error("element {element} references missing storey {storey}")]
    MissingStorey { element: String, storey: String },
    #[error("element {0} has invalid identity, bounds, or attributes")]
    InvalidElement(String),
}

impl BimModel {
    /// Validates project, storey, element, bounds, and property invariants.
    ///
    /// # Errors
    ///
    /// Returns the first deterministic BIM model validation error.
    pub fn validate(&self) -> Result<(), BimError> {
        if self.project_id.trim().is_empty() || self.project_name.trim().is_empty() {
            return Err(BimError::EmptyProject);
        }
        let mut storeys = BTreeSet::new();
        for storey in &self.storeys {
            if !storeys.insert(storey.id.clone()) {
                return Err(BimError::DuplicateStorey(storey.id.clone()));
            }
            if storey.id.trim().is_empty()
                || storey.name.trim().is_empty()
                || !storey.elevation_mm.is_finite()
                || !storey.height_mm.is_finite()
                || storey.height_mm <= 0.0
            {
                return Err(BimError::InvalidStorey(storey.id.clone()));
            }
        }
        let mut elements = BTreeSet::new();
        for element in &self.elements {
            if !elements.insert(element.id.clone()) {
                return Err(BimError::DuplicateElement(element.id.clone()));
            }
            if !storeys.contains(&element.storey_id) {
                return Err(BimError::MissingStorey {
                    element: element.id.clone(),
                    storey: element.storey_id.clone(),
                });
            }
            let attributes_valid = element.attributes.iter().all(|attribute| {
                !attribute.name.trim().is_empty()
                    && !matches!(&attribute.value, BimValue::Number(value) if !value.is_finite())
            });
            if element.id.trim().is_empty()
                || element.name.trim().is_empty()
                || element.bounds.is_some_and(|bounds| !bounds.is_valid())
                || !attributes_valid
            {
                return Err(BimError::InvalidElement(element.id.clone()));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn schedule(&self, class: Option<BimElementClass>) -> Vec<ScheduleRow> {
        let mut rows = self
            .elements
            .iter()
            .filter(|element| class.is_none_or(|class| element.class == class))
            .map(|element| ScheduleRow {
                id: element.id.clone(),
                name: element.name.clone(),
                ifc_class: element.class.ifc_name().into(),
                storey: self
                    .storeys
                    .iter()
                    .find(|storey| storey.id == element.storey_id)
                    .map_or_else(|| element.storey_id.clone(), |storey| storey.name.clone()),
                properties: element
                    .attributes
                    .iter()
                    .map(|attribute| (attribute.name.clone(), attribute.value.display_value()))
                    .collect(),
            })
            .collect::<Vec<_>>();
        rows.sort_by(|first, second| first.id.cmp(&second.id));
        rows
    }

    #[must_use]
    pub fn quantity_takeoff(&self) -> QuantityTakeoff {
        let mut takeoff = QuantityTakeoff {
            element_count: self.elements.len(),
            ..QuantityTakeoff::default()
        };
        for element in &self.elements {
            let Some(bounds) = element.bounds else {
                continue;
            };
            let size = bounds.size_mm();
            takeoff.gross_volume_mm3 += bounds.volume_mm3();
            match element.class {
                BimElementClass::Wall => takeoff.wall_length_mm += size[0].max(size[1]),
                BimElementClass::Slab => takeoff.slab_area_mm2 += size[0] * size[1],
                _ => {}
            }
        }
        takeoff
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRow {
    pub id: String,
    pub name: String,
    pub ifc_class: String,
    pub storey: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuantityTakeoff {
    pub element_count: usize,
    pub wall_length_mm: f64,
    pub slab_area_mm2: f64,
    pub gross_volume_mm3: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WallSpec {
    pub length_mm: f64,
    pub thickness_mm: f64,
    pub height_mm: f64,
    pub base_elevation_mm: f64,
}

impl WallSpec {
    #[must_use]
    pub fn is_valid(self) -> bool {
        [self.length_mm, self.thickness_mm, self.height_mm]
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
            && self.base_elevation_mm.is_finite()
    }

    #[must_use]
    pub const fn box_size_mm(self) -> [f64; 3] {
        [self.length_mm, self.thickness_mm, self.height_mm]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SlabSpec {
    pub width_mm: f64,
    pub depth_mm: f64,
    pub thickness_mm: f64,
    pub elevation_mm: f64,
}

impl SlabSpec {
    #[must_use]
    pub fn is_valid(self) -> bool {
        [self.width_mm, self.depth_mm, self.thickness_mm]
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
            && self.elevation_mm.is_finite()
    }

    #[must_use]
    pub const fn box_size_mm(self) -> [f64; 3] {
        [self.width_mm, self.depth_mm, self.thickness_mm]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_quantities_are_deterministic() {
        let mut model = BimModel::default();
        model.elements.push(BimElement {
            id: "wall-1".into(),
            name: "External wall".into(),
            class: BimElementClass::Wall,
            storey_id: "level-1".into(),
            attributes: Vec::new(),
            bounds: Some(Bounds3 {
                minimum_mm: [0.0; 3],
                maximum_mm: [3_000.0, 200.0, 2_800.0],
            }),
            linked_feature_id: None,
        });
        model.validate().unwrap();
        assert_eq!(model.schedule(None)[0].ifc_class, "IFCWALL");
        assert!((model.quantity_takeoff().wall_length_mm - 3_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_storey_is_rejected() {
        let mut model = BimModel::default();
        model.elements.push(BimElement {
            id: "wall-1".into(),
            name: "Wall".into(),
            class: BimElementClass::Wall,
            storey_id: "missing".into(),
            attributes: Vec::new(),
            bounds: None,
            linked_feature_id: None,
        });
        assert!(matches!(
            model.validate(),
            Err(BimError::MissingStorey { .. })
        ));
    }
}
