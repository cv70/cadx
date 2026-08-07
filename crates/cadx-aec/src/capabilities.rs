//! Declared AEC/BIM feature flags.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AecCapabilities {
    pub wall_and_slab_solids: bool,
    pub opening_solids: bool,
    pub bim_attributes: bool,
    pub level_management: bool,
    pub ifc_exchange: bool,
    pub clash_review: bool,
    pub quantity_takeoff: bool,
}

impl Default for AecCapabilities {
    fn default() -> Self {
        Self {
            wall_and_slab_solids: true,
            opening_solids: true,
            bim_attributes: true,
            level_management: true,
            ifc_exchange: true,
            clash_review: true,
            quantity_takeoff: true,
        }
    }
}
