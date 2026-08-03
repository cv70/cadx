//! Standards-aware engineering drawing primitives.
//!
//! This crate intentionally has no dependency on CADX geometry or UI crates.
//! A future drawing renderer can consume the same validated sheet contract.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DrawingStandard {
    #[default]
    Gb,
    Iso,
    Asme,
}

impl DrawingStandard {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Gb => "GB/T",
            Self::Iso => "ISO",
            Self::Asme => "ASME",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionMethod {
    #[default]
    FirstAngle,
    ThirdAngle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    LinearDimension,
    Diameter,
    Radius,
    GeometricTolerance,
    SurfaceRoughness,
    Weld,
    Gear,
    Chain,
    Fit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub text: String,
    pub position_mm: [f64; 2],
    #[serde(default)]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingSheet {
    pub standard: DrawingStandard,
    pub projection: ProjectionMethod,
    pub width_mm: f64,
    pub height_mm: f64,
    pub scale: f64,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
}

impl Default for DrawingSheet {
    fn default() -> Self {
        Self {
            standard: DrawingStandard::Gb,
            projection: ProjectionMethod::FirstAngle,
            width_mm: 297.0,
            height_mm: 210.0,
            scale: 1.0,
            annotations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardsSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandardsIssue {
    pub code: String,
    pub severity: StandardsSeverity,
    pub message: String,
    #[serde(default)]
    pub annotation_index: Option<usize>,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DrawingError {
    #[error("sheet dimensions must be finite and positive")]
    InvalidSheet,
    #[error("drawing scale must be finite and positive")]
    InvalidScale,
    #[error("annotation {0} contains a non-finite position")]
    InvalidAnnotation(usize),
    #[error("annotation text must not be empty")]
    EmptyAnnotation,
}

impl DrawingSheet {
    /// Validates the sheet before it is handed to a renderer or exporter.
    /// # Errors
    ///
    /// Returns [`DrawingError`] when the sheet or annotation geometry is invalid.
    pub fn validate(&self) -> Result<(), DrawingError> {
        if !self.width_mm.is_finite()
            || !self.height_mm.is_finite()
            || self.width_mm <= 0.0
            || self.height_mm <= 0.0
        {
            return Err(DrawingError::InvalidSheet);
        }
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return Err(DrawingError::InvalidScale);
        }
        for (index, annotation) in self.annotations.iter().enumerate() {
            if annotation.text.trim().is_empty() {
                return Err(DrawingError::EmptyAnnotation);
            }
            if !annotation.position_mm.iter().all(|value| value.is_finite()) {
                return Err(DrawingError::InvalidAnnotation(index));
            }
        }
        Ok(())
    }

    /// Performs deterministic checks that are useful before a detailed CAD
    /// drawing validator is available.
    #[must_use]
    pub fn inspect(&self) -> Vec<StandardsIssue> {
        let mut issues = Vec::new();
        if self.validate().is_err() {
            issues.push(StandardsIssue {
                code: "SHEET_INVALID".into(),
                severity: StandardsSeverity::Error,
                message: "Sheet geometry or scale is invalid".into(),
                annotation_index: None,
            });
            return issues;
        }
        for (index, annotation) in self.annotations.iter().enumerate() {
            let out_of_bounds = annotation.position_mm[0] < 0.0
                || annotation.position_mm[0] > self.width_mm
                || annotation.position_mm[1] < 0.0
                || annotation.position_mm[1] > self.height_mm;
            if out_of_bounds {
                issues.push(StandardsIssue {
                    code: "ANNOTATION_OUT_OF_SHEET".into(),
                    severity: StandardsSeverity::Warning,
                    message: "Annotation is outside the drawing sheet".into(),
                    annotation_index: Some(index),
                });
            }
            if matches!(annotation.kind, AnnotationKind::GeometricTolerance)
                && !annotation.text.to_ascii_uppercase().contains("DIA")
                && !annotation.text.to_ascii_uppercase().contains("DATUM")
            {
                issues.push(StandardsIssue {
                    code: "GEOMETRIC_TOLERANCE_DATUM".into(),
                    severity: StandardsSeverity::Warning,
                    message: "Geometric tolerance should identify a datum or diameter".into(),
                    annotation_index: Some(index),
                });
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gb_sheet_reports_annotations_outside_border() {
        let sheet = DrawingSheet {
            annotations: vec![Annotation {
                kind: AnnotationKind::LinearDimension,
                text: "24".into(),
                position_mm: [400.0, 10.0],
                reference: None,
            }],
            ..DrawingSheet::default()
        };
        let issues = sheet.inspect();
        assert_eq!(issues[0].code, "ANNOTATION_OUT_OF_SHEET");
    }
}
