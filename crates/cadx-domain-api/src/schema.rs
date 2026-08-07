//! Declarative inspector/tool field schemas and their value validation.

use crate::{DomainIssue, DomainIssueSeverity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainFieldKind {
    Text,
    Integer,
    Decimal,
    LengthMm,
    AngleDeg,
    Boolean,
    Select,
    EntityReference,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainSelectOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DomainFieldSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: DomainFieldKind,
    pub default_value: Option<&'static str>,
    pub unit: Option<&'static str>,
    pub options: &'static [DomainSelectOption],
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DomainPanelSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub fields: &'static [DomainFieldSchema],
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct DomainInspectorSchema {
    pub panels: &'static [DomainPanelSchema],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DomainFieldValue {
    Text(String),
    Integer(i64),
    Decimal(f64),
    Boolean(bool),
    EntityReference(u64),
    Color(String),
}

impl DomainFieldValue {
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) | Self::Color(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_decimal(&self) -> Option<f64> {
        match self {
            Self::Decimal(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_boolean(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_entity_reference(&self) -> Option<u64> {
        match self {
            Self::EntityReference(value) => Some(*value),
            _ => None,
        }
    }
}

pub type DomainParameters = BTreeMap<String, DomainFieldValue>;

impl DomainPanelSchema {
    #[must_use]
    pub fn field(&self, id: &str) -> Option<&DomainFieldSchema> {
        self.fields.iter().find(|field| field.id == id)
    }

    /// Merges schema defaults into supplied values and validates all values.
    ///
    /// # Errors
    ///
    /// Returns one or more field-level issues when a required value is absent,
    /// a value has the wrong type, or a select value is not declared.
    pub fn resolve_parameters(
        &self,
        supplied: &DomainParameters,
    ) -> Result<DomainParameters, Vec<DomainIssue>> {
        let mut resolved = supplied.clone();
        let mut issues = Vec::new();

        for field in self.fields {
            if !resolved.contains_key(field.id) {
                if let Some(default) = field.default_value {
                    match parse_default(field.kind, default) {
                        Ok(value) => {
                            resolved.insert(field.id.into(), value);
                        }
                        Err(message) => issues.push(parameter_issue(field.id, message)),
                    }
                } else if field.required {
                    issues.push(parameter_issue(field.id, "required value is missing"));
                }
            }

            if let Some(value) = resolved.get(field.id)
                && let Err(message) = validate_field_value(field, value)
            {
                issues.push(parameter_issue(field.id, message));
            }
        }

        for id in supplied.keys() {
            if self.field(id).is_none() {
                issues.push(parameter_issue(id, "field is not declared by this panel"));
            }
        }

        if issues.is_empty() {
            Ok(resolved)
        } else {
            Err(issues)
        }
    }
}

fn parse_default(kind: DomainFieldKind, value: &str) -> Result<DomainFieldValue, &'static str> {
    match kind {
        DomainFieldKind::Text | DomainFieldKind::Select => Ok(DomainFieldValue::Text(value.into())),
        DomainFieldKind::Integer => value
            .parse()
            .map(DomainFieldValue::Integer)
            .map_err(|_| "schema default is not an integer"),
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(DomainFieldValue::Decimal)
            .ok_or("schema default is not a finite number"),
        DomainFieldKind::Boolean => value
            .parse()
            .map(DomainFieldValue::Boolean)
            .map_err(|_| "schema default is not a boolean"),
        DomainFieldKind::EntityReference => value
            .parse()
            .map(DomainFieldValue::EntityReference)
            .map_err(|_| "schema default is not an entity reference"),
        DomainFieldKind::Color => Ok(DomainFieldValue::Color(value.into())),
    }
}

fn validate_field_value(
    field: &DomainFieldSchema,
    value: &DomainFieldValue,
) -> Result<(), &'static str> {
    let valid_type = match field.kind {
        DomainFieldKind::Text | DomainFieldKind::Select => value.as_text().is_some(),
        DomainFieldKind::Integer => value.as_integer().is_some(),
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => {
            value.as_decimal().is_some_and(f64::is_finite)
        }
        DomainFieldKind::Boolean => value.as_boolean().is_some(),
        DomainFieldKind::EntityReference => value.as_entity_reference().is_some(),
        DomainFieldKind::Color => matches!(value, DomainFieldValue::Color(_)),
    };
    if !valid_type {
        return Err("value type does not match the schema");
    }
    if field.kind == DomainFieldKind::Select
        && !field
            .options
            .iter()
            .any(|option| value.as_text() == Some(option.value))
    {
        return Err("value is not one of the declared options");
    }
    Ok(())
}

fn parameter_issue(field: &str, message: impl Into<String>) -> DomainIssue {
    DomainIssue {
        code: format!("INVALID_PARAMETER.{field}"),
        severity: DomainIssueSeverity::Error,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_resolves_defaults_and_rejects_unknown_select_values() {
        static OPTIONS: [DomainSelectOption; 2] = [
            DomainSelectOption {
                value: "first",
                label: "First",
            },
            DomainSelectOption {
                value: "second",
                label: "Second",
            },
        ];
        static FIELDS: [DomainFieldSchema; 2] = [
            DomainFieldSchema {
                id: "size",
                label: "Size",
                kind: DomainFieldKind::LengthMm,
                default_value: Some("12.5"),
                unit: Some("mm"),
                options: &[],
                required: true,
            },
            DomainFieldSchema {
                id: "mode",
                label: "Mode",
                kind: DomainFieldKind::Select,
                default_value: Some("first"),
                unit: None,
                options: &OPTIONS,
                required: true,
            },
        ];
        let panel = DomainPanelSchema {
            id: "test",
            label: "Test",
            fields: &FIELDS,
        };
        let defaults = panel.resolve_parameters(&DomainParameters::new()).unwrap();
        assert_eq!(defaults["size"].as_decimal(), Some(12.5));
        assert_eq!(defaults["mode"].as_text(), Some("first"));

        let invalid = [("mode".into(), DomainFieldValue::Text("third".into()))]
            .into_iter()
            .collect();
        assert!(panel.resolve_parameters(&invalid).is_err());
    }
}
