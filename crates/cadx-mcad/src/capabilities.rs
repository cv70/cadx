//! Declared MCAD feature flags.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct McadCapabilities {
    pub feature_tree: bool,
    pub sketch_and_feature_tools: bool,
    pub engineering_drawing: bool,
    pub tolerance_and_fit_tables: bool,
    pub standard_parts: bool,
    pub assembly_constraints: bool,
    pub interference_check: bool,
    pub dfm_review: bool,
    pub ai_natural_language_parts: bool,
    pub bom: bool,
}

impl Default for McadCapabilities {
    fn default() -> Self {
        Self {
            feature_tree: true,
            sketch_and_feature_tools: true,
            engineering_drawing: true,
            tolerance_and_fit_tables: true,
            standard_parts: true,
            assembly_constraints: true,
            interference_check: true,
            dfm_review: true,
            ai_natural_language_parts: true,
            bom: true,
        }
    }
}
