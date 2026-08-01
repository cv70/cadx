use cadx_core::{
    diagnostics::{BooleanDiagnostic, EdgeModifierDiagnostic, SketchConstraintDiagnostic},
    domain::DocumentError,
    kernel::KernelError,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("document command failed: {0}")]
    Document(#[from] DocumentError),
    #[error("CAD kernel rejected the document: {0}")]
    Kernel(#[from] KernelError),
    #[error("document revision id space is exhausted")]
    RevisionExhausted,
}

impl SessionError {
    #[must_use]
    pub const fn boolean_diagnostic(&self) -> Option<&BooleanDiagnostic> {
        match self {
            Self::Kernel(KernelError::Boolean(diagnostic)) => Some(diagnostic),
            Self::Document(_) | Self::Kernel(_) | Self::RevisionExhausted => None,
        }
    }

    #[must_use]
    pub const fn edge_modifier_diagnostic(&self) -> Option<&EdgeModifierDiagnostic> {
        match self {
            Self::Kernel(KernelError::EdgeModifier(diagnostic)) => Some(diagnostic),
            Self::Document(_) | Self::Kernel(_) | Self::RevisionExhausted => None,
        }
    }

    #[must_use]
    pub const fn sketch_constraint_diagnostic(&self) -> Option<&SketchConstraintDiagnostic> {
        match self {
            Self::Document(DocumentError::SketchConstraint(diagnostic)) => Some(diagnostic),
            Self::Document(_) | Self::Kernel(_) | Self::RevisionExhausted => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use cadx_core::diagnostics::{SketchConstraintDiagnostic, SketchConstraintFailureReason};

    use super::*;

    #[test]
    fn exposes_structured_sketch_constraint_diagnostics() {
        let diagnostic = SketchConstraintDiagnostic {
            reason: SketchConstraintFailureReason::Conflict,
            constraint_indices: vec![0, 2],
            iterations: 32,
            residual: 0.25,
            detail: "incompatible dimensions".into(),
        };
        let error = SessionError::Document(DocumentError::SketchConstraint(diagnostic.clone()));
        assert_eq!(error.sketch_constraint_diagnostic(), Some(&diagnostic));
        assert!(error.boolean_diagnostic().is_none());
        assert!(error.edge_modifier_diagnostic().is_none());
    }
}
