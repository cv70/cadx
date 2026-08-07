//! Solver, shader, and AI tool descriptors advertised by domain packs.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainSolverStage {
    Modeling,
    Constraint,
    Analysis,
    Routing,
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainSolver {
    pub id: &'static str,
    pub label: &'static str,
    pub stage: DomainSolverStage,
    pub description: &'static str,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainShaderStage {
    Render,
    Overlay,
    Compute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainShader {
    pub id: &'static str,
    pub label: &'static str,
    pub stage: DomainShaderStage,
    pub entry_point: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainAiTool {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub schema_id: &'static str,
    /// Stable [`DomainTool::id`] executed after the model selects this tool.
    ///
    /// [`DomainTool::id`]: crate::DomainTool::id
    pub executable_tool_id: &'static str,
}
