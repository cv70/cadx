//! Domain tool registry used by function-calling and the egui command palette.

use cadx_domain_api::{DomainAiTool, DomainId, DomainPack, DomainTool};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<(DomainId, String), DomainTool>,
    ai_tools: BTreeMap<(DomainId, String), DomainAiTool>,
}

impl ToolRegistry {
    pub fn register_pack(&mut self, pack: &dyn DomainPack) {
        let domain = pack.manifest().id;
        for tool in pack.tools() {
            self.tools.insert((domain, tool.id.into()), tool.clone());
        }
        for tool in pack.ai_tools() {
            self.ai_tools.insert((domain, tool.id.into()), *tool);
        }
    }

    #[must_use]
    pub fn tools_for(&self, domain: DomainId) -> Vec<&DomainTool> {
        self.tools
            .iter()
            .filter(|((candidate, _), _)| *candidate == domain)
            .map(|(_, tool)| tool)
            .collect()
    }

    #[must_use]
    pub fn find(&self, domain: DomainId, tool_id: &str) -> Option<&DomainTool> {
        self.tools.get(&(domain, tool_id.into()))
    }

    #[must_use]
    pub fn ai_tools_for(&self, domain: DomainId) -> Vec<&DomainAiTool> {
        self.ai_tools
            .iter()
            .filter(|((candidate, _), _)| *candidate == domain)
            .map(|(_, tool)| tool)
            .collect()
    }

    #[must_use]
    pub fn find_ai_tool(&self, domain: DomainId, tool_id: &str) -> Option<&DomainAiTool> {
        self.ai_tools.get(&(domain, tool_id.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_domain_api::{
        DomainAction, DomainContext, DomainIssue, DomainManifest, DomainRoute, ExportFormat,
    };

    struct Pack;

    impl DomainPack for Pack {
        fn manifest(&self) -> DomainManifest {
            DomainManifest {
                id: DomainId::Mcad,
                name: "M",
                version: "0.1",
                description: "",
                priority: 0,
            }
        }
        fn tools(&self) -> &'static [DomainTool] {
            static TOOLS: [DomainTool; 1] = [DomainTool {
                id: "bom",
                label: "BOM",
                icon: "layers",
                category: "AI",
            }];
            &TOOLS
        }
        fn ai_tools(&self) -> &'static [DomainAiTool] {
            static AI_TOOLS: [DomainAiTool; 1] = [DomainAiTool {
                id: "generate_bom",
                label: "Generate BOM",
                description: "Create a grouped bill of materials",
                schema_id: "cadx.domain.mechanical.generate_bom",
            }];
            &AI_TOOLS
        }
        fn route_natural_language(&self, _input: &str, _context: &DomainContext) -> DomainRoute {
            DomainRoute {
                action: DomainAction::GenerateBom,
                confidence: 1.0,
                rationale: String::new(),
            }
        }
        fn validate_export(
            &self,
            _format: ExportFormat,
            _context: &DomainContext,
        ) -> Vec<DomainIssue> {
            Vec::new()
        }
    }

    #[test]
    fn registry_indexes_pack_tools_by_domain() {
        let mut registry = ToolRegistry::default();
        registry.register_pack(&Pack);
        assert_eq!(registry.tools_for(DomainId::Mcad).len(), 1);
        assert!(registry.find(DomainId::Mcad, "bom").is_some());
        assert_eq!(registry.ai_tools_for(DomainId::Mcad).len(), 1);
        assert!(
            registry
                .find_ai_tool(DomainId::Mcad, "generate_bom")
                .is_some()
        );
    }
}
