//! Kernel-neutral manufacturability checks for mechanical parts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartCheckInput {
    pub id: String,
    pub name: String,
    pub bbox_mm: [f64; 3],
    #[serde(default)]
    pub minimum_wall_mm: Option<f64>,
    #[serde(default)]
    pub smallest_hole_mm: Option<f64>,
    #[serde(default)]
    pub material: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DfmSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DfmIssue {
    pub code: String,
    pub severity: DfmSeverity,
    pub part_id: String,
    pub message: String,
    #[serde(default)]
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DfmReport {
    pub issues: Vec<DfmIssue>,
    pub checked_parts: usize,
}

/// Runs conservative, material-agnostic checks. Domain-specific process
/// plugins can layer milling, turning, sheet-metal, or additive rules on top.
#[must_use]
pub fn inspect(parts: &[PartCheckInput]) -> DfmReport {
    let mut report = DfmReport {
        checked_parts: parts.len(),
        ..DfmReport::default()
    };
    for part in parts {
        let [x, y, z] = part.bbox_mm;
        if ![x, y, z].iter().all(|value| value.is_finite())
            || [x, y, z].iter().any(|value| *value <= 0.0)
        {
            report.issues.push(DfmIssue {
                code: "INVALID_ENVELOPE".into(),
                severity: DfmSeverity::Error,
                part_id: part.id.clone(),
                message: "Part envelope is not finite and positive".into(),
                recommendation: Some(
                    "Rebuild the source feature before manufacturing review".into(),
                ),
            });
        }
        if part.minimum_wall_mm.is_some_and(|wall| wall < 1.0) {
            report.issues.push(DfmIssue {
                code: "THIN_WALL".into(),
                severity: DfmSeverity::Warning,
                part_id: part.id.clone(),
                message: "Minimum wall is below the 1.0 mm review threshold".into(),
                recommendation: Some(
                    "Increase wall thickness or confirm the process capability".into(),
                ),
            });
        }
        if part.smallest_hole_mm.is_some_and(|hole| hole < 1.5) {
            report.issues.push(DfmIssue {
                code: "SMALL_HOLE".into(),
                severity: DfmSeverity::Warning,
                part_id: part.id.clone(),
                message: "Smallest hole may require a special tool".into(),
                recommendation: Some("Use a standard drill or add a process note".into()),
            });
        }
        if part.material.is_none() {
            report.issues.push(DfmIssue {
                code: "MATERIAL_UNASSIGNED".into(),
                severity: DfmSeverity::Info,
                part_id: part.id.clone(),
                message: "Material is not assigned".into(),
                recommendation: Some("Assign a material before mass and process sign-off".into()),
            });
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_wall_and_material_are_reported() {
        let report = inspect(&[PartCheckInput {
            id: "1".into(),
            name: "Bracket".into(),
            bbox_mm: [40.0, 20.0, 3.0],
            minimum_wall_mm: Some(0.8),
            smallest_hole_mm: None,
            material: None,
        }]);
        assert!(report.issues.iter().any(|issue| issue.code == "THIN_WALL"));
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "MATERIAL_UNASSIGNED")
        );
    }
}
