//! Kernel-neutral PCB layout contracts.
//!
//! The board model is deliberately independent from a renderer, router, or
//! electrical solver. It is the interchange boundary for those future packs.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    Copper,
    SolderMask,
    Silkscreen,
    Dielectric,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbLayer {
    pub name: String,
    pub kind: LayerKind,
    pub index: u16,
    pub thickness_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbComponent {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub position_mm: [f64; 2],
    pub size_mm: [f64; 2],
    pub height_mm: f64,
    #[serde(default)]
    pub rotation_deg: f64,
    #[serde(default)]
    pub side: ComponentSide,
    #[serde(default)]
    pub model_3d: Option<String>,
    #[serde(default)]
    pub linked_feature_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ComponentSide {
    #[default]
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbNet {
    pub name: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub pins: Vec<String>,
    #[serde(default)]
    pub impedance_ohms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbTrace {
    pub net: String,
    pub layer: String,
    pub width_mm: f64,
    pub points_mm: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbPad {
    pub reference: String,
    pub number: String,
    pub net: String,
    pub position_mm: [f64; 2],
    pub size_mm: [f64; 2],
    #[serde(default)]
    pub drill_mm: Option<f64>,
    #[serde(default)]
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbVia {
    pub net: String,
    pub position_mm: [f64; 2],
    pub diameter_mm: f64,
    pub drill_mm: f64,
    pub start_layer: String,
    pub end_layer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbKeepout {
    pub name: String,
    pub center_mm: [f64; 2],
    pub size_mm: [f64; 2],
    #[serde(default)]
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbDesignRules {
    pub min_trace_width_mm: f64,
    pub min_clearance_mm: f64,
    pub min_hole_mm: f64,
    pub edge_clearance_mm: f64,
    #[serde(default)]
    pub impedance_tolerance_percent: Option<f64>,
}

impl Default for PcbDesignRules {
    fn default() -> Self {
        Self {
            min_trace_width_mm: 0.15,
            min_clearance_mm: 0.15,
            min_hole_mm: 0.2,
            edge_clearance_mm: 0.25,
            impedance_tolerance_percent: Some(10.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcbBoard {
    pub name: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub thickness_mm: f64,
    pub layers: Vec<PcbLayer>,
    #[serde(default)]
    pub components: Vec<PcbComponent>,
    #[serde(default)]
    pub nets: Vec<PcbNet>,
    #[serde(default)]
    pub traces: Vec<PcbTrace>,
    #[serde(default)]
    pub pads: Vec<PcbPad>,
    #[serde(default)]
    pub vias: Vec<PcbVia>,
    #[serde(default)]
    pub keepouts: Vec<PcbKeepout>,
    #[serde(default)]
    pub rules: PcbDesignRules,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LayoutError {
    #[error("board dimensions and thickness must be finite and positive")]
    InvalidBoard,
    #[error("board must contain at least two layers")]
    MissingLayers,
    #[error("layer names must be unique and thicknesses positive")]
    InvalidLayers,
    #[error("component {0} has invalid geometry")]
    InvalidComponent(String),
    #[error("net names must be unique")]
    DuplicateNet,
    #[error("trace {0} has invalid geometry")]
    InvalidTrace(usize),
    #[error("pad {0} has invalid geometry or references")]
    InvalidPad(usize),
    #[error("via {0} has invalid geometry or references")]
    InvalidVia(usize),
    #[error("design rules must be finite and positive")]
    InvalidRules,
}

impl PcbBoard {
    /// Creates an empty rectangular board with an alternating copper and
    /// dielectric stackup.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::InvalidLayers`] unless the copper layer count is
    /// even and at least two, or when the requested thickness cannot contain
    /// the copper foils.
    pub fn rectangular(
        name: impl Into<String>,
        width_mm: f64,
        height_mm: f64,
        thickness_mm: f64,
        copper_layer_count: u16,
    ) -> Result<Self, LayoutError> {
        let copper_thickness = 0.035;
        if copper_layer_count < 2
            || !copper_layer_count.is_multiple_of(2)
            || !width_mm.is_finite()
            || !height_mm.is_finite()
            || !thickness_mm.is_finite()
            || width_mm <= 0.0
            || height_mm <= 0.0
            || thickness_mm <= copper_thickness * f64::from(copper_layer_count)
        {
            return Err(LayoutError::InvalidLayers);
        }
        let dielectric_thickness = (thickness_mm
            - copper_thickness * f64::from(copper_layer_count))
            / f64::from(copper_layer_count - 1);
        let mut layers = Vec::with_capacity(usize::from(copper_layer_count) * 2 - 1);
        for copper_index in 0..copper_layer_count {
            let name = match copper_index {
                0 => "F.Cu".into(),
                index if index + 1 == copper_layer_count => "B.Cu".into(),
                index => format!("In{index}.Cu"),
            };
            layers.push(PcbLayer {
                name,
                kind: LayerKind::Copper,
                index: u16::try_from(layers.len()).unwrap_or(u16::MAX),
                thickness_mm: copper_thickness,
            });
            if copper_index + 1 < copper_layer_count {
                layers.push(PcbLayer {
                    name: format!("Core{}.Dielectric", copper_index + 1),
                    kind: LayerKind::Dielectric,
                    index: u16::try_from(layers.len()).unwrap_or(u16::MAX),
                    thickness_mm: dielectric_thickness,
                });
            }
        }
        Ok(Self {
            name: name.into(),
            width_mm,
            height_mm,
            thickness_mm,
            layers,
            components: Vec::new(),
            nets: Vec::new(),
            traces: Vec::new(),
            pads: Vec::new(),
            vias: Vec::new(),
            keepouts: Vec::new(),
            rules: PcbDesignRules::default(),
        })
    }

    #[must_use]
    pub fn demo() -> Self {
        let mut board =
            Self::rectangular("Controller board", 80.0, 50.0, 1.6, 4).unwrap_or_else(|_| Self {
                name: "Controller board".into(),
                width_mm: 80.0,
                height_mm: 50.0,
                thickness_mm: 1.6,
                layers: vec![
                    PcbLayer {
                        name: "F.Cu".into(),
                        kind: LayerKind::Copper,
                        index: 0,
                        thickness_mm: 0.035,
                    },
                    PcbLayer {
                        name: "B.Cu".into(),
                        kind: LayerKind::Copper,
                        index: 1,
                        thickness_mm: 0.035,
                    },
                ],
                components: Vec::new(),
                nets: Vec::new(),
                traces: Vec::new(),
                pads: Vec::new(),
                vias: Vec::new(),
                keepouts: Vec::new(),
                rules: PcbDesignRules::default(),
            });
        board.components = vec![PcbComponent {
            reference: "U1".into(),
            value: "MCU".into(),
            footprint: "QFN-32".into(),
            position_mm: [40.0, 25.0],
            size_mm: [8.0, 8.0],
            height_mm: 1.0,
            rotation_deg: 0.0,
            side: ComponentSide::Top,
            model_3d: Some("QFN-32.step".into()),
            linked_feature_id: None,
        }];
        board.nets = vec![PcbNet {
            name: "GND".into(),
            class: "POWER".into(),
            pins: vec!["U1.1".into()],
            impedance_ohms: None,
        }];
        board
    }

    /// # Errors
    ///
    /// Returns [`LayoutError`] when board, layer, component, net, or trace
    /// invariants are violated.
    pub fn validate(&self) -> Result<(), LayoutError> {
        if self.name.trim().is_empty()
            || !self.width_mm.is_finite()
            || !self.height_mm.is_finite()
            || !self.thickness_mm.is_finite()
            || self.width_mm <= 0.0
            || self.height_mm <= 0.0
            || self.thickness_mm <= 0.0
        {
            return Err(LayoutError::InvalidBoard);
        }
        if self.layers.len() < 2 {
            return Err(LayoutError::MissingLayers);
        }
        if !self.rules.min_trace_width_mm.is_finite()
            || !self.rules.min_clearance_mm.is_finite()
            || !self.rules.min_hole_mm.is_finite()
            || !self.rules.edge_clearance_mm.is_finite()
            || self.rules.min_trace_width_mm <= 0.0
            || self.rules.min_clearance_mm <= 0.0
            || self.rules.min_hole_mm <= 0.0
            || self.rules.edge_clearance_mm <= 0.0
        {
            return Err(LayoutError::InvalidRules);
        }
        let mut names = std::collections::BTreeSet::new();
        if self.layers.iter().any(|layer| {
            !layer.thickness_mm.is_finite()
                || layer.thickness_mm <= 0.0
                || layer.name.trim().is_empty()
                || !names.insert(layer.name.clone())
        }) {
            return Err(LayoutError::InvalidLayers);
        }
        let mut references = std::collections::BTreeSet::new();
        for component in &self.components {
            let valid = !component.reference.trim().is_empty()
                && references.insert(component.reference.clone())
                && component.position_mm.iter().all(|value| value.is_finite())
                && component
                    .size_mm
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0)
                && component.height_mm.is_finite()
                && component.height_mm > 0.0
                && component.rotation_deg.is_finite()
                && component.position_mm[0] - component.size_mm[0] * 0.5 >= 0.0
                && component.position_mm[1] - component.size_mm[1] * 0.5 >= 0.0
                && component.position_mm[0] + component.size_mm[0] * 0.5 <= self.width_mm
                && component.position_mm[1] + component.size_mm[1] * 0.5 <= self.height_mm;
            if !valid {
                return Err(LayoutError::InvalidComponent(component.reference.clone()));
            }
        }
        let mut nets = std::collections::BTreeSet::new();
        if self
            .nets
            .iter()
            .any(|net| net.name.trim().is_empty() || !nets.insert(net.name.clone()))
        {
            return Err(LayoutError::DuplicateNet);
        }
        for (index, trace) in self.traces.iter().enumerate() {
            if trace.width_mm <= 0.0
                || !trace.width_mm.is_finite()
                || trace.points_mm.len() < 2
                || trace.points_mm.iter().any(|point| {
                    !point.iter().all(|value| value.is_finite())
                        || point[0] < 0.0
                        || point[0] > self.width_mm
                        || point[1] < 0.0
                        || point[1] > self.height_mm
                })
            {
                return Err(LayoutError::InvalidTrace(index));
            }
        }
        for (index, pad) in self.pads.iter().enumerate() {
            let valid = !pad.reference.trim().is_empty()
                && !pad.number.trim().is_empty()
                && !pad.net.trim().is_empty()
                && pad.position_mm.iter().all(|value| value.is_finite())
                && pad
                    .size_mm
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0)
                && pad
                    .drill_mm
                    .is_none_or(|value| value.is_finite() && value > 0.0)
                && point_inside(self, pad.position_mm)
                && self.nets.iter().any(|net| net.name == pad.net)
                && pad
                    .layers
                    .iter()
                    .all(|name| self.layers.iter().any(|layer| layer.name == *name));
            if !valid {
                return Err(LayoutError::InvalidPad(index));
            }
        }
        for (index, via) in self.vias.iter().enumerate() {
            let valid = !via.net.trim().is_empty()
                && via.position_mm.iter().all(|value| value.is_finite())
                && point_inside(self, via.position_mm)
                && via.diameter_mm.is_finite()
                && via.drill_mm.is_finite()
                && via.diameter_mm > via.drill_mm
                && via.drill_mm > 0.0
                && self.nets.iter().any(|net| net.name == via.net)
                && self
                    .layers
                    .iter()
                    .any(|layer| layer.name == via.start_layer)
                && self.layers.iter().any(|layer| layer.name == via.end_layer)
                && via.start_layer != via.end_layer;
            if !valid {
                return Err(LayoutError::InvalidVia(index));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn bom(&self) -> Vec<(String, String, String, u32)> {
        let mut grouped = std::collections::BTreeMap::<(String, String), Vec<String>>::new();
        for component in &self.components {
            grouped
                .entry((component.value.clone(), component.footprint.clone()))
                .or_default()
                .push(component.reference.clone());
        }
        grouped
            .into_iter()
            .map(|((value, footprint), mut references)| {
                references.sort();
                let quantity = u32::try_from(references.len()).unwrap_or(u32::MAX);
                (references.join(", "), value, footprint, quantity)
            })
            .collect()
    }
}

fn point_inside(board: &PcbBoard, point: [f64; 2]) -> bool {
    point[0] >= 0.0 && point[0] <= board.width_mm && point[1] >= 0.0 && point[1] <= board.height_mm
}

impl Default for PcbBoard {
    fn default() -> Self {
        Self::demo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_rejects_component_outside_outline() {
        let mut board = PcbBoard::demo();
        board.components[0].position_mm = [1.0, 1.0];
        assert!(matches!(
            board.validate(),
            Err(LayoutError::InvalidComponent(_))
        ));
    }
}
