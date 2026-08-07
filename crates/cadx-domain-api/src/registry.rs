//! Runtime-filterable bus that owns registered domain packs.

use crate::{
    DomainAiTool, DomainContext, DomainExecution, DomainExecutionError, DomainId,
    DomainInspectorSchema, DomainManifest, DomainPack, DomainRoute, DomainShader, DomainSolver,
    DomainToolRequest,
};
use std::{collections::BTreeSet, sync::Arc};

/// Compile-time registered, runtime-filterable domain pack bus.
///
/// The bus stores only the small [`DomainPack`] SPI. It never knows about a
/// geometry kernel, document entity, renderer, or domain implementation type.
#[derive(Default)]
pub struct DomainRegistry {
    packs: Vec<Arc<dyn DomainPack>>,
    enabled: BTreeSet<DomainId>,
}

impl DomainRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, pack: Arc<dyn DomainPack>) {
        let id = pack.manifest().id;
        if self
            .packs
            .iter()
            .all(|candidate| candidate.manifest().id != id)
        {
            self.packs.push(pack);
            self.enabled.insert(id);
        }
    }

    pub fn set_enabled(&mut self, id: DomainId, enabled: bool) {
        if self.packs.iter().any(|pack| pack.manifest().id == id) {
            if enabled {
                self.enabled.insert(id);
            } else {
                self.enabled.remove(&id);
            }
        }
    }

    #[must_use]
    pub fn is_enabled(&self, id: DomainId) -> bool {
        self.enabled.contains(&id)
    }

    #[must_use]
    pub fn enabled_packs(&self) -> Vec<Arc<dyn DomainPack>> {
        self.packs
            .iter()
            .filter(|pack| self.enabled.contains(&pack.manifest().id))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn manifests(&self) -> Vec<DomainManifest> {
        self.packs
            .iter()
            .filter(|pack| self.enabled.contains(&pack.manifest().id))
            .map(|pack| pack.manifest())
            .collect()
    }

    #[must_use]
    pub fn registered_manifests(&self) -> Vec<DomainManifest> {
        self.packs.iter().map(|pack| pack.manifest()).collect()
    }

    #[must_use]
    pub fn pack(&self, id: DomainId) -> Option<Arc<dyn DomainPack>> {
        self.packs
            .iter()
            .find(|pack| pack.manifest().id == id)
            .cloned()
    }

    #[must_use]
    pub fn inspector_schema(&self, id: DomainId) -> Option<DomainInspectorSchema> {
        self.pack(id).map(|pack| pack.inspector_schema())
    }

    #[must_use]
    pub fn solvers(&self, id: DomainId) -> Vec<DomainSolver> {
        self.pack(id)
            .map_or_else(Vec::new, |pack| pack.solvers().to_vec())
    }

    #[must_use]
    pub fn shaders(&self, id: DomainId) -> Vec<DomainShader> {
        self.pack(id)
            .map_or_else(Vec::new, |pack| pack.shaders().to_vec())
    }

    #[must_use]
    pub fn ai_tools(&self, id: DomainId) -> Vec<DomainAiTool> {
        self.pack(id)
            .map_or_else(Vec::new, |pack| pack.ai_tools().to_vec())
    }

    /// Executes a tool through the enabled pack boundary.
    ///
    /// # Errors
    ///
    /// Returns a registration, enablement, parameter, or pack execution error.
    pub fn execute(
        &self,
        id: DomainId,
        request: &DomainToolRequest,
    ) -> Result<DomainExecution, DomainExecutionError> {
        let pack = self
            .pack(id)
            .ok_or(DomainExecutionError::PackNotRegistered(id))?;
        if !self.is_enabled(id) {
            return Err(DomainExecutionError::PackDisabled(id));
        }
        pack.execute_tool(request)
    }

    #[must_use]
    pub fn route(&self, input: &str, context: &DomainContext) -> Option<(DomainId, DomainRoute)> {
        self.enabled_packs()
            .into_iter()
            .map(|pack| {
                let manifest = pack.manifest();
                (
                    manifest.id,
                    manifest.priority,
                    pack.route_natural_language(input, context),
                )
            })
            .max_by(|(_, first_priority, first), (_, second_priority, second)| {
                first
                    .confidence
                    .total_cmp(&second.confidence)
                    .then_with(|| first_priority.cmp(second_priority))
            })
            .map(|(id, _, route)| (id, route))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DomainAction, DomainIssue, DomainSolverStage, DomainTool, ExportFormat};

    struct TestPack(DomainId);

    impl DomainPack for TestPack {
        fn manifest(&self) -> DomainManifest {
            DomainManifest {
                id: self.0,
                name: "test",
                version: "0.1",
                description: "test",
                priority: 0,
            }
        }

        fn tools(&self) -> &'static [DomainTool] {
            &[]
        }

        fn route_natural_language(&self, _input: &str, _context: &DomainContext) -> DomainRoute {
            DomainRoute {
                action: DomainAction::GenerateBom,
                confidence: 0.5,
                rationale: "test".into(),
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
    fn packs_can_be_disabled_without_unregistering_them() {
        let mut registry = DomainRegistry::new();
        registry.register(Arc::new(TestPack(DomainId::Mcad)));
        registry.register(Arc::new(TestPack(DomainId::Mcad)));
        assert_eq!(registry.manifests().len(), 1);
        registry.set_enabled(DomainId::Mcad, false);
        assert!(registry.manifests().is_empty());
    }

    #[test]
    fn registry_exposes_pack_pipeline_descriptors() {
        static SOLVERS: [DomainSolver; 1] = [DomainSolver {
            id: "solver",
            label: "Solver",
            stage: DomainSolverStage::Modeling,
            description: "test",
            inputs: &["input"],
            outputs: &["output"],
        }];

        struct DescribedPack;

        impl DomainPack for DescribedPack {
            fn manifest(&self) -> DomainManifest {
                DomainManifest {
                    id: DomainId::Aec,
                    name: "described",
                    version: "0.1",
                    description: "test",
                    priority: 0,
                }
            }

            fn tools(&self) -> &'static [DomainTool] {
                &[]
            }

            fn solvers(&self) -> &'static [DomainSolver] {
                &SOLVERS
            }

            fn route_natural_language(
                &self,
                _input: &str,
                _context: &DomainContext,
            ) -> DomainRoute {
                DomainRoute {
                    action: DomainAction::OpenPanel {
                        panel: "test".into(),
                    },
                    confidence: 1.0,
                    rationale: "test".into(),
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

        let mut registry = DomainRegistry::new();
        registry.register(Arc::new(DescribedPack));
        assert_eq!(registry.solvers(DomainId::Aec)[0].id, "solver");
    }

    #[test]
    fn registry_blocks_execution_when_pack_is_disabled() {
        let mut registry = DomainRegistry::new();
        registry.register(Arc::new(TestPack(DomainId::Mcad)));
        registry.set_enabled(DomainId::Mcad, false);
        let error = registry
            .execute(
                DomainId::Mcad,
                &DomainToolRequest::new("missing", DomainContext::default()),
            )
            .unwrap_err();
        assert_eq!(error, DomainExecutionError::PackDisabled(DomainId::Mcad));
    }
}
