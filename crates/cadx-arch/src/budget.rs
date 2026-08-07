//! The per-file readability budget.
//!
//! A module that outgrows this budget stops being reviewable in one sitting and
//! starts hiding coupling. The rule is intentionally blunt: split the file into
//! cohesive submodules rather than raising the number.
//!
//! [`PENDING_DECOMPOSITION`] is a ratchet, not an escape hatch. Every entry must
//! still be over budget, so decomposing a file forces its entry to be deleted;
//! the list can only shrink.

use std::{fs, path::Path};

use crate::workspace::Workspace;

/// The maximum number of lines a single `.rs` file may contain.
pub const FILE_LINE_BUDGET: usize = 1_000;

/// Files that predate the budget and are still being decomposed.
///
/// Workspace-relative paths. A path listed here is exempt from
/// [`FILE_LINE_BUDGET`] but must still exceed it — see
/// [`super::tests`] for the staleness check that enforces the ratchet.
pub const PENDING_DECOMPOSITION: &[&str] = &[
    "crates/cadx-core/src/domain.rs",
    "crates/cadx-core/src/persistence.rs",
    "crates/cadx-desktop/src/lib.rs",
    "crates/cadx-kernel-truck/src/boolean/contact.rs",
    "crates/cadx-kernel-truck/src/lib.rs",
    "crates/cadx-kernel-truck/src/topology.rs",
    "crates/cadx-render/src/lib.rs",
];

/// A source file over its allowed size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBudgetViolation {
    /// Workspace-relative path.
    pub path: String,
    /// The file's actual line count.
    pub lines: usize,
}

/// Every non-exempt source file that exceeds the budget, largest first.
#[must_use]
pub fn audit_file_budget(workspace: &Workspace) -> Vec<FileBudgetViolation> {
    let mut violations = measure(workspace);
    violations.retain(|violation| !is_exempt(&violation.path));
    violations
}

/// Exempt paths that no longer exceed the budget and must be delisted.
#[must_use]
pub fn stale_exemptions(workspace: &Workspace) -> Vec<&'static str> {
    let over_budget = measure(workspace);
    PENDING_DECOMPOSITION
        .iter()
        .copied()
        .filter(|path| {
            !over_budget
                .iter()
                .any(|violation| violation.path.as_str() == *path)
        })
        .collect()
}

/// Whether a workspace-relative path is currently exempt.
#[must_use]
pub fn is_exempt(relative: &str) -> bool {
    PENDING_DECOMPOSITION.contains(&relative)
}

fn measure(workspace: &Workspace) -> Vec<FileBudgetViolation> {
    let mut violations = Vec::new();
    for manifest in workspace.manifests() {
        for file in manifest.source_files() {
            let Some(path) = relative_path(workspace.root(), &file) else {
                continue;
            };
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let lines = text.lines().count();
            if lines > FILE_LINE_BUDGET {
                violations.push(FileBudgetViolation { path, lines });
            }
        }
    }
    violations.sort_by_key(|violation| std::cmp::Reverse(violation.lines));
    violations
}

fn relative_path(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemptions_are_unique_relative_and_sorted() {
        let mut previous = "";
        for path in PENDING_DECOMPOSITION {
            assert!(!path.starts_with('/'), "{path} must be workspace-relative");
            assert!(
                previous < *path,
                "PENDING_DECOMPOSITION must be sorted and duplicate-free near {path}"
            );
            previous = path;
        }
    }

    #[test]
    fn exemption_membership_is_exact() {
        assert!(is_exempt("crates/cadx-core/src/domain.rs"));
        assert!(!is_exempt("crates/cadx-core/src/lib.rs"));
    }
}
