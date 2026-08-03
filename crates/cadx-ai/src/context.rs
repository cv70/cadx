//! Read-only context collection for the AI native layer.

use cadx_domain_api::DomainId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContextSnapshot {
    pub domain: Option<DomainId>,
    pub document_name: String,
    pub selected_feature_ids: Vec<u64>,
    pub visible_solid_count: usize,
    pub active_feature_count: usize,
    #[serde(default)]
    pub viewport: Map<String, Value>,
    #[serde(default)]
    pub domain_schema: Map<String, Value>,
}

/// Collects host-provided read-only state without retaining kernel handles.
#[derive(Debug, Clone, Default)]
pub struct ContextCollector {
    snapshot: ContextSnapshot,
}

impl ContextCollector {
    #[must_use]
    pub fn new(snapshot: ContextSnapshot) -> Self {
        Self { snapshot }
    }

    #[must_use]
    pub fn snapshot(&self) -> &ContextSnapshot {
        &self.snapshot
    }

    pub fn set_domain_schema(&mut self, schema: Map<String, Value>) {
        self.snapshot.domain_schema = schema;
    }

    #[must_use]
    pub fn as_json(&self) -> Value {
        serde_json::to_value(&self.snapshot).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_serializable_without_kernel_objects() {
        let collector = ContextCollector::new(ContextSnapshot {
            domain: Some(DomainId::Ecad),
            document_name: "board".into(),
            active_feature_count: 3,
            ..ContextSnapshot::default()
        });
        assert_eq!(collector.as_json()["domain"], "ecad");
        assert_eq!(collector.snapshot().active_feature_count, 3);
    }
}
