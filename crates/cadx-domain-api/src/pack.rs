//! The [`DomainPack`] service-provider interface implemented by every pack.

use crate::{
    DomainAction, DomainAiTool, DomainContext, DomainExecution, DomainExecutionError,
    DomainInspectorSchema, DomainIssue, DomainManifest, DomainPanelSchema, DomainRoute,
    DomainShader, DomainSolver, DomainTool, DomainToolRequest, ExportFormat,
};

/// A domain pack is a pure business plugin. Implementations may parse natural
/// language and perform domain checks, but they never receive kernel objects.
pub trait DomainPack: Send + Sync {
    fn manifest(&self) -> DomainManifest;
    fn tools(&self) -> &'static [DomainTool];
    fn inspector_schema(&self) -> DomainInspectorSchema {
        DomainInspectorSchema::default()
    }
    fn tool_panel(&self, _tool_id: &str) -> Option<DomainPanelSchema> {
        None
    }
    fn solvers(&self) -> &'static [DomainSolver] {
        &[]
    }
    fn shaders(&self) -> &'static [DomainShader] {
        &[]
    }
    fn ai_tools(&self) -> &'static [DomainAiTool] {
        &[]
    }
    /// Executes a registered domain tool without accessing host or kernel
    /// state. Packs return business actions; the host owns their transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DomainExecutionError::UnknownTool`] when the tool is not part
    /// of this pack. Implementations may report parameter or domain failures.
    fn execute_tool(
        &self,
        request: &DomainToolRequest,
    ) -> Result<DomainExecution, DomainExecutionError> {
        let manifest = self.manifest();
        if !self.tools().iter().any(|tool| tool.id == request.tool_id) {
            return Err(DomainExecutionError::UnknownTool {
                domain: manifest.id,
                tool_id: request.tool_id.clone(),
            });
        }
        Ok(DomainExecution::with_action(
            format!("Open {}", request.tool_id),
            DomainAction::OpenPanel {
                panel: request.tool_id.clone(),
            },
        ))
    }
    fn route_natural_language(&self, input: &str, context: &DomainContext) -> DomainRoute;
    fn validate_export(&self, format: ExportFormat, context: &DomainContext) -> Vec<DomainIssue>;
}
