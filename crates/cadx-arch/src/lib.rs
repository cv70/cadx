//! Executable architecture contract for the CADX workspace.
//!
//! The layering rules in `docs/architecture.md` are prose. This crate turns the
//! parts a machine can check into tests: every workspace member is classified
//! into exactly one layer, dependencies may only point inward, inward layers may
//! not name outward technologies, and no source file may exceed its readability
//! budget.
//!
//! The crate is `publish = false` and has no dependencies. It reads the
//! workspace from disk rather than linking against it, so adding a rule here
//! can never change shipped behavior.

mod budget;
mod layers;
mod manifest;
mod workspace;

pub use budget::{
    FILE_LINE_BUDGET, FileBudgetViolation, PENDING_DECOMPOSITION, audit_file_budget, is_exempt,
    stale_exemptions,
};
pub use layers::{
    CRATE_LAYERS, Layer, LayerViolation, PENDING_INVERSION, TechnologyViolation,
    audit_dependencies, audit_technologies, is_pending, layer_of, stale_inversions,
};
pub use manifest::{CrateManifest, parse_dependencies};
pub use workspace::{Workspace, WorkspaceError};

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace::discover().expect("workspace root is reachable from this crate")
    }

    #[test]
    fn every_member_is_classified_into_exactly_one_layer() {
        let workspace = workspace();
        let mut unclassified = Vec::new();
        for member in workspace.members() {
            if layer_of(member).is_none() {
                unclassified.push(member.clone());
            }
        }
        assert!(
            unclassified.is_empty(),
            "workspace members are missing from CRATE_LAYERS: {unclassified:?}. \
             Add the crate to cadx-arch::layers and document it in docs/module-map.md."
        );

        let mut stale = Vec::new();
        for (name, _) in CRATE_LAYERS {
            if !workspace.members().iter().any(|member| member == name) {
                stale.push(*name);
            }
        }
        assert!(
            stale.is_empty(),
            "CRATE_LAYERS names crates that are not workspace members: {stale:?}"
        );
    }

    #[test]
    fn dependencies_only_point_inward() {
        let workspace = workspace();
        let violations = layers::audit_dependencies(&workspace);
        assert!(
            violations.is_empty(),
            "outward or forbidden crate dependencies: {violations:#?}"
        );
    }

    #[test]
    fn pending_inversions_still_exist() {
        let workspace = workspace();
        let stale = stale_inversions(&workspace);
        assert!(
            stale.is_empty(),
            "PENDING_INVERSION lists edges that no longer exist: {stale:?}. \
             Delete them — the list is a ratchet and may only shrink."
        );
    }

    #[test]
    fn inward_layers_do_not_name_outward_technologies() {
        let workspace = workspace();
        let violations = layers::audit_technologies(&workspace);
        assert!(
            violations.is_empty(),
            "a layer depends on a technology it must stay independent of: {violations:#?}"
        );
    }

    #[test]
    fn no_source_file_exceeds_its_readability_budget() {
        let workspace = workspace();
        let violations = audit_file_budget(&workspace);
        assert!(
            violations.is_empty(),
            "source files exceed the {FILE_LINE_BUDGET}-line budget: {violations:#?}. \
             Split the file into cohesive submodules instead of raising the budget."
        );
    }

    #[test]
    fn pending_decompositions_are_still_over_budget() {
        let workspace = workspace();
        let stale = stale_exemptions(&workspace);
        assert!(
            stale.is_empty(),
            "PENDING_DECOMPOSITION lists files that now fit in {FILE_LINE_BUDGET} lines: \
             {stale:?}. Delete them — the list is a ratchet and may only shrink."
        );
    }
}
