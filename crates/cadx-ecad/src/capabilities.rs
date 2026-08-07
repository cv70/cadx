//! Declared ECAD/PCB feature flags.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct EcadCapabilities {
    pub schematic_capture: bool,
    pub netlist_import: bool,
    pub multilayer_layout: bool,
    pub electrical_drc: bool,
    pub impedance_rules: bool,
    pub automatic_routing: bool,
    pub component_3d_link: bool,
    pub enclosure_interference: bool,
    pub gerber_export: bool,
    pub step_export: bool,
}

impl Default for EcadCapabilities {
    fn default() -> Self {
        Self {
            schematic_capture: true,
            netlist_import: true,
            multilayer_layout: true,
            electrical_drc: true,
            impedance_rules: true,
            automatic_routing: true,
            component_3d_link: true,
            enclosure_interference: true,
            gerber_export: true,
            step_export: true,
        }
    }
}
