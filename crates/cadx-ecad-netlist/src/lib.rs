//! Validated, renderer-neutral schematic and netlist contracts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PinRef {
    pub reference: String,
    pub pin: String,
}

impl PinRef {
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}.{}", self.reference, self.pin)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchematicComponent {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub pins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElectricalNet {
    pub name: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub pins: Vec<PinRef>,
    #[serde(default)]
    pub impedance_ohms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Netlist {
    #[serde(default)]
    pub components: Vec<SchematicComponent>,
    #[serde(default)]
    pub nets: Vec<ElectricalNet>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NetlistError {
    #[error("component reference {0} occurs more than once")]
    DuplicateComponent(String),
    #[error("component {0} has invalid identity or duplicate pins")]
    InvalidComponent(String),
    #[error("net name {0} occurs more than once")]
    DuplicateNet(String),
    #[error("net {net} references unknown pin {pin}")]
    UnknownPin { net: String, pin: String },
    #[error("pin {pin} is assigned to both {first_net} and {second_net}")]
    PinOnMultipleNets {
        pin: String,
        first_net: String,
        second_net: String,
    },
    #[error("net {0} has invalid impedance")]
    InvalidImpedance(String),
}

impl Netlist {
    /// Validates component identity, pin ownership, net names, and impedance.
    ///
    /// # Errors
    ///
    /// Returns the first deterministic connectivity error.
    pub fn validate(&self) -> Result<(), NetlistError> {
        let mut known_pins = BTreeSet::new();
        let mut references = BTreeSet::new();
        for component in &self.components {
            if !references.insert(component.reference.clone()) {
                return Err(NetlistError::DuplicateComponent(
                    component.reference.clone(),
                ));
            }
            let mut pins = BTreeSet::new();
            if component.reference.trim().is_empty()
                || component.value.trim().is_empty()
                || component.footprint.trim().is_empty()
                || component.pins.is_empty()
                || component
                    .pins
                    .iter()
                    .any(|pin| pin.trim().is_empty() || !pins.insert(pin.clone()))
            {
                return Err(NetlistError::InvalidComponent(component.reference.clone()));
            }
            known_pins.extend(component.pins.iter().map(|pin| PinRef {
                reference: component.reference.clone(),
                pin: pin.clone(),
            }));
        }

        let mut net_names = BTreeSet::new();
        let mut pin_owners = BTreeMap::<PinRef, String>::new();
        for net in &self.nets {
            if net.name.trim().is_empty() || !net_names.insert(net.name.clone()) {
                return Err(NetlistError::DuplicateNet(net.name.clone()));
            }
            if net
                .impedance_ohms
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            {
                return Err(NetlistError::InvalidImpedance(net.name.clone()));
            }
            for pin in &net.pins {
                if !known_pins.contains(pin) {
                    return Err(NetlistError::UnknownPin {
                        net: net.name.clone(),
                        pin: pin.key(),
                    });
                }
                if let Some(first_net) = pin_owners.insert(pin.clone(), net.name.clone()) {
                    return Err(NetlistError::PinOnMultipleNets {
                        pin: pin.key(),
                        first_net,
                        second_net: net.name.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn unrouted_pin_count(&self, routed_nets: &BTreeSet<String>) -> usize {
        self.nets
            .iter()
            .filter(|net| !routed_nets.contains(&net.name))
            .map(|net| net.pins.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(reference: &str) -> SchematicComponent {
        SchematicComponent {
            reference: reference.into(),
            value: "R".into(),
            footprint: "0603".into(),
            pins: vec!["1".into(), "2".into()],
        }
    }

    #[test]
    fn valid_connectivity_passes() {
        let netlist = Netlist {
            components: vec![component("R1"), component("R2")],
            nets: vec![ElectricalNet {
                name: "SIGNAL".into(),
                class: "DEFAULT".into(),
                pins: vec![
                    PinRef {
                        reference: "R1".into(),
                        pin: "1".into(),
                    },
                    PinRef {
                        reference: "R2".into(),
                        pin: "1".into(),
                    },
                ],
                impedance_ohms: None,
            }],
        };
        netlist.validate().unwrap();
    }

    #[test]
    fn unknown_pin_is_rejected() {
        let netlist = Netlist {
            components: vec![component("R1")],
            nets: vec![ElectricalNet {
                name: "SIGNAL".into(),
                class: String::new(),
                pins: vec![PinRef {
                    reference: "R1".into(),
                    pin: "9".into(),
                }],
                impedance_ohms: None,
            }],
        };
        assert!(matches!(
            netlist.validate(),
            Err(NetlistError::UnknownPin { .. })
        ));
    }
}
