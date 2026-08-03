//! Reviewable domain intent and diff objects.

use cadx_domain_api::{DomainAction, DomainId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentDiff {
    pub domain: DomainId,
    pub prompt: String,
    pub actions: Vec<DomainAction>,
    pub summary: String,
    pub reversible: bool,
    pub accepted: bool,
}

impl IntentDiff {
    #[must_use]
    pub fn pending(domain: DomainId, prompt: impl Into<String>, action: DomainAction) -> Self {
        Self {
            domain,
            prompt: prompt.into(),
            actions: vec![action],
            summary: "Pending domain action".into(),
            reversible: true,
            accepted: false,
        }
    }

    pub fn accept(&mut self) {
        self.accepted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_diff_is_explicitly_accepted() {
        let mut diff =
            IntentDiff::pending(DomainId::Ecad, "make a board", DomainAction::GenerateBom);
        assert!(!diff.accepted);
        diff.accept();
        assert!(diff.accepted);
    }
}
