//! Domain tool registry used by function-calling and the egui command palette.

use cadx_domain_api::{
    DomainAiTool, DomainFieldKind, DomainFieldValue, DomainId, DomainPack, DomainPanelSchema,
    DomainParameters, DomainTool,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct DomainAiToolBinding {
    pub domain: DomainId,
    pub ai_tool: DomainAiTool,
    pub executable_tool: DomainTool,
    pub panel: Option<DomainPanelSchema>,
}

impl DomainAiToolBinding {
    #[must_use]
    pub fn parameter_schema(&self) -> Value {
        let Some(panel) = self.panel else {
            return json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            });
        };

        let mut properties = Map::new();
        let mut required = Vec::new();
        for field in panel.fields {
            let mut schema = Map::new();
            match field.kind {
                DomainFieldKind::Text | DomainFieldKind::Color => {
                    schema.insert("type".into(), json!("string"));
                }
                DomainFieldKind::Select => {
                    schema.insert("type".into(), json!("string"));
                    schema.insert(
                        "enum".into(),
                        Value::Array(
                            field
                                .options
                                .iter()
                                .map(|option| Value::String(option.value.into()))
                                .collect(),
                        ),
                    );
                }
                DomainFieldKind::Integer => {
                    schema.insert("type".into(), json!("integer"));
                }
                DomainFieldKind::Decimal
                | DomainFieldKind::LengthMm
                | DomainFieldKind::AngleDeg => {
                    schema.insert("type".into(), json!("number"));
                }
                DomainFieldKind::Boolean => {
                    schema.insert("type".into(), json!("boolean"));
                }
                DomainFieldKind::EntityReference => {
                    schema.insert("type".into(), json!("integer"));
                    schema.insert("minimum".into(), json!(0));
                }
            }
            let description = field.unit.map_or_else(
                || field.label.to_owned(),
                |unit| format!("{} ({unit})", field.label),
            );
            schema.insert("description".into(), Value::String(description));
            if let Some(default) = json_default(field.kind, field.default_value) {
                schema.insert("default".into(), default);
            }
            if field.required {
                required.push(Value::String(field.id.into()));
            }
            properties.insert(field.id.into(), Value::Object(schema));
        }

        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    }

    /// Converts the model's plain JSON object into the domain pack's typed
    /// parameter representation, then applies the pack panel's validation.
    ///
    /// # Errors
    ///
    /// Returns a bounded validation message when arguments are not an object,
    /// contain undeclared fields, use incompatible JSON types, or fail the
    /// domain panel's required/default/select validation.
    pub fn decode_parameters(&self, arguments: &Value) -> Result<DomainParameters, String> {
        let object = arguments
            .as_object()
            .ok_or_else(|| format!("{} arguments must be a JSON object", self.ai_tool.id))?;
        let Some(panel) = self.panel else {
            return object
                .is_empty()
                .then(DomainParameters::new)
                .ok_or_else(|| format!("{} does not accept parameters", self.ai_tool.id));
        };

        let mut supplied = DomainParameters::new();
        for (id, value) in object {
            let field = panel
                .field(id)
                .ok_or_else(|| format!("field {id} is not declared by {}", self.ai_tool.id))?;
            let value = decode_field_value(field.kind, value)
                .ok_or_else(|| format!("field {id} has the wrong JSON type"))?;
            supplied.insert(id.clone(), value);
        }
        panel.resolve_parameters(&supplied).map_err(|issues| {
            issues
                .into_iter()
                .map(|issue| format!("{}: {}", issue.code, issue.message))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
}

fn json_default(kind: DomainFieldKind, default: Option<&str>) -> Option<Value> {
    let default = default?;
    match kind {
        DomainFieldKind::Text | DomainFieldKind::Select | DomainFieldKind::Color => {
            Some(Value::String(default.into()))
        }
        DomainFieldKind::Integer => default.parse::<i64>().ok().map(Value::from),
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => default
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Value::from),
        DomainFieldKind::Boolean => default.parse::<bool>().ok().map(Value::from),
        DomainFieldKind::EntityReference => default.parse::<u64>().ok().map(Value::from),
    }
}

fn decode_field_value(kind: DomainFieldKind, value: &Value) -> Option<DomainFieldValue> {
    match kind {
        DomainFieldKind::Text | DomainFieldKind::Select => value
            .as_str()
            .map(|value| DomainFieldValue::Text(value.into())),
        DomainFieldKind::Integer => value.as_i64().map(DomainFieldValue::Integer),
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(DomainFieldValue::Decimal),
        DomainFieldKind::Boolean => value.as_bool().map(DomainFieldValue::Boolean),
        DomainFieldKind::EntityReference => value.as_u64().map(DomainFieldValue::EntityReference),
        DomainFieldKind::Color => value
            .as_str()
            .map(|value| DomainFieldValue::Color(value.into())),
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolRegistryError {
    #[error("domain {domain:?} declares duplicate tool id {tool_id}")]
    DuplicateTool { domain: DomainId, tool_id: String },
    #[error("domain {domain:?} declares duplicate AI tool id {tool_id}")]
    DuplicateAiTool { domain: DomainId, tool_id: String },
    #[error(
        "AI tool {ai_tool_id} in domain {domain:?} maps to unknown executable tool {executable_tool_id}"
    )]
    UnknownExecutableTool {
        domain: DomainId,
        ai_tool_id: String,
        executable_tool_id: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<(DomainId, String), DomainTool>,
    ai_tools: BTreeMap<(DomainId, String), DomainAiToolBinding>,
}

impl ToolRegistry {
    /// Registers a complete pack atomically. Invalid or duplicate mappings leave
    /// the registry unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`ToolRegistryError`] for duplicate domain or AI tool ids and
    /// for AI tools that do not name an executable tool from the same pack.
    pub fn register_pack(&mut self, pack: &dyn DomainPack) -> Result<(), ToolRegistryError> {
        let domain = pack.manifest().id;
        let mut staged_tools = Vec::new();
        let mut tool_ids = BTreeSet::new();
        for tool in pack.tools() {
            let key = (domain, tool.id.to_owned());
            if !tool_ids.insert(tool.id) || self.tools.contains_key(&key) {
                return Err(ToolRegistryError::DuplicateTool {
                    domain,
                    tool_id: tool.id.into(),
                });
            }
            staged_tools.push((key, tool.clone()));
        }

        let tools_by_id = staged_tools
            .iter()
            .map(|(_, tool)| (tool.id, tool))
            .collect::<BTreeMap<_, _>>();
        let mut staged_ai_tools = Vec::new();
        let mut ai_tool_ids = BTreeSet::new();
        for ai_tool in pack.ai_tools() {
            let key = (domain, ai_tool.id.to_owned());
            if !ai_tool_ids.insert(ai_tool.id) || self.ai_tools.contains_key(&key) {
                return Err(ToolRegistryError::DuplicateAiTool {
                    domain,
                    tool_id: ai_tool.id.into(),
                });
            }
            let executable_tool = tools_by_id.get(ai_tool.executable_tool_id).ok_or_else(|| {
                ToolRegistryError::UnknownExecutableTool {
                    domain,
                    ai_tool_id: ai_tool.id.into(),
                    executable_tool_id: ai_tool.executable_tool_id.into(),
                }
            })?;
            staged_ai_tools.push((
                key,
                DomainAiToolBinding {
                    domain,
                    ai_tool: *ai_tool,
                    executable_tool: (*executable_tool).clone(),
                    panel: pack.tool_panel(ai_tool.executable_tool_id),
                },
            ));
        }

        self.tools.extend(staged_tools);
        self.ai_tools.extend(staged_ai_tools);
        Ok(())
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
    pub fn ai_tools_for(&self, domain: DomainId) -> Vec<&DomainAiToolBinding> {
        self.ai_tools
            .iter()
            .filter(|((candidate, _), _)| *candidate == domain)
            .map(|(_, tool)| tool)
            .collect()
    }

    #[must_use]
    pub fn find_ai_tool(&self, domain: DomainId, tool_id: &str) -> Option<&DomainAiToolBinding> {
        self.ai_tools.get(&(domain, tool_id.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_domain_api::{
        DomainAction, DomainContext, DomainFieldSchema, DomainIssue, DomainManifest, DomainRoute,
        ExportFormat,
    };

    const FIELDS: [DomainFieldSchema; 1] = [DomainFieldSchema {
        id: "quantity",
        label: "Quantity",
        kind: DomainFieldKind::Integer,
        default_value: Some("1"),
        unit: None,
        options: &[],
        required: true,
    }];
    const PANEL: DomainPanelSchema = DomainPanelSchema {
        id: "bom",
        label: "BOM",
        fields: &FIELDS,
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
        fn tool_panel(&self, tool_id: &str) -> Option<DomainPanelSchema> {
            (tool_id == "bom").then_some(PANEL)
        }
        fn ai_tools(&self) -> &'static [DomainAiTool] {
            static AI_TOOLS: [DomainAiTool; 1] = [DomainAiTool {
                id: "generate_bom",
                label: "Generate BOM",
                description: "Create a grouped bill of materials",
                schema_id: "cadx.domain.mechanical.generate_bom",
                executable_tool_id: "bom",
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
    fn registry_binds_ai_metadata_to_an_executable_tool() {
        let mut registry = ToolRegistry::default();
        registry.register_pack(&Pack).unwrap();
        assert_eq!(registry.tools_for(DomainId::Mcad).len(), 1);
        let binding = registry
            .find_ai_tool(DomainId::Mcad, "generate_bom")
            .unwrap();
        assert_eq!(binding.executable_tool.id, "bom");
        assert_eq!(binding.parameter_schema()["additionalProperties"], false);
        assert_eq!(binding.parameter_schema()["required"], json!(["quantity"]));
        assert_eq!(
            binding.decode_parameters(&json!({"quantity": 3})).unwrap()["quantity"],
            DomainFieldValue::Integer(3)
        );
    }

    #[test]
    fn registry_rejects_dangling_ai_mapping_without_partial_registration() {
        struct BrokenPack;
        impl DomainPack for BrokenPack {
            fn manifest(&self) -> DomainManifest {
                Pack.manifest()
            }
            fn tools(&self) -> &'static [DomainTool] {
                Pack.tools()
            }
            fn ai_tools(&self) -> &'static [DomainAiTool] {
                static AI_TOOLS: [DomainAiTool; 1] = [DomainAiTool {
                    id: "broken",
                    label: "Broken",
                    description: "Broken mapping",
                    schema_id: "broken.v1",
                    executable_tool_id: "missing",
                }];
                &AI_TOOLS
            }
            fn route_natural_language(&self, input: &str, context: &DomainContext) -> DomainRoute {
                Pack.route_natural_language(input, context)
            }
            fn validate_export(
                &self,
                format: ExportFormat,
                context: &DomainContext,
            ) -> Vec<DomainIssue> {
                Pack.validate_export(format, context)
            }
        }

        let mut registry = ToolRegistry::default();
        assert!(matches!(
            registry.register_pack(&BrokenPack),
            Err(ToolRegistryError::UnknownExecutableTool { .. })
        ));
        assert!(registry.tools_for(DomainId::Mcad).is_empty());
    }

    #[test]
    fn decoder_rejects_tagged_domain_values_and_unknown_fields() {
        let mut registry = ToolRegistry::default();
        registry.register_pack(&Pack).unwrap();
        let binding = registry
            .find_ai_tool(DomainId::Mcad, "generate_bom")
            .unwrap();
        assert!(
            binding
                .decode_parameters(&json!({"quantity": {"kind": "integer", "value": 3}}))
                .is_err()
        );
        assert!(binding.decode_parameters(&json!({"extra": 1})).is_err());
    }
}
