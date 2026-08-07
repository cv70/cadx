//! Layer classification and the inward-only dependency rule.

use crate::workspace::Workspace;

/// One architectural layer of the CADX workspace.
///
/// Layers are not a single linear stack: `Spi` and the domain packs above it
/// form an independent column that must stay free of the geometry column, and
/// only the composition root may see both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    /// Exact kernel-neutral 2D geometry and constraint solving.
    Geometry,
    /// The parametric document, commands, invariants, and kernel ports.
    Domain,
    /// The geometry-neutral domain-pack plugin protocol.
    Spi,
    /// A concrete industry pack built only on [`Layer::Spi`].
    Pack,
    /// Read-only measurement and physical analysis over evaluated scenes.
    Analysis,
    /// Typed user configuration and preference storage.
    Configuration,
    /// Use cases, transactions, history, and document sessions.
    Application,
    /// Concrete adapters: CAD kernels, exchange formats, AI providers.
    Infrastructure,
    /// Rendering and localization resources consumed by a UI shell.
    Presentation,
    /// The single binary that constructs every concrete adapter.
    Composition,
    /// This crate: the executable architecture contract itself.
    Contract,
}

impl Layer {
    /// Whether a crate in this layer may depend on a crate in `other`.
    #[must_use]
    pub const fn may_depend_on(self, other: Self) -> bool {
        match self {
            Self::Geometry | Self::Spi | Self::Configuration | Self::Contract => false,
            Self::Domain => matches!(other, Self::Geometry),
            Self::Pack => matches!(other, Self::Spi | Self::Pack),
            Self::Analysis => matches!(other, Self::Domain | Self::Geometry),
            Self::Application => matches!(other, Self::Domain | Self::Geometry),
            Self::Infrastructure => matches!(
                other,
                Self::Domain | Self::Geometry | Self::Analysis | Self::Configuration | Self::Spi
            ),
            Self::Presentation => matches!(other, Self::Domain | Self::Geometry),
            Self::Composition => true,
        }
    }

    /// Third-party crate-name prefixes this layer must not depend on.
    ///
    /// Prefixes are matched against the dependency's first `-`-separated
    /// segment, so `truck` covers `truck-modeling` and `truck-topology`.
    #[must_use]
    pub const fn forbidden_technologies(self) -> &'static [&'static str] {
        match self {
            // Pure layers: no UI, GPU, concrete kernel, provider, or filesystem.
            Self::Geometry | Self::Domain | Self::Spi | Self::Pack | Self::Analysis => &[
                "egui", "eframe", "rfd", "iconflow", "wgpu", "bytemuck", "glam", "truck",
                "ruststep", "lib3mf", "genai", "tokio", "home", "tempfile",
            ],
            // Use cases and configuration may touch the filesystem boundary
            // through an adapter, but never a UI, GPU, kernel, or provider SDK.
            Self::Application | Self::Configuration => &[
                "egui", "eframe", "rfd", "iconflow", "wgpu", "bytemuck", "glam", "truck",
                "ruststep", "lib3mf", "genai", "tokio",
            ],
            // Adapters own their backend but must not draw their own UI.
            Self::Infrastructure => &["egui", "eframe", "rfd", "iconflow"],
            // Rendering owns the GPU but never a kernel or provider.
            Self::Presentation => &["truck", "ruststep", "lib3mf", "genai", "tokio"],
            Self::Composition | Self::Contract => &[],
        }
    }
}

/// Every workspace member's layer. Adding a crate requires adding it here.
pub const CRATE_LAYERS: &[(&str, Layer)] = &[
    ("cadx-sketch", Layer::Geometry),
    ("cadx-core", Layer::Domain),
    ("cadx-domain-api", Layer::Spi),
    ("cadx-mcad", Layer::Pack),
    ("cadx-mcad-model", Layer::Pack),
    ("cadx-mcad-standards", Layer::Pack),
    ("cadx-mcad-dfm", Layer::Pack),
    ("cadx-mcad-bom", Layer::Pack),
    ("cadx-aec", Layer::Pack),
    ("cadx-aec-bim", Layer::Pack),
    ("cadx-aec-ifc", Layer::Pack),
    ("cadx-aec-analysis", Layer::Pack),
    ("cadx-ecad", Layer::Pack),
    ("cadx-ecad-netlist", Layer::Pack),
    ("cadx-ecad-layout", Layer::Pack),
    ("cadx-ecad-router", Layer::Pack),
    ("cadx-ecad-drc", Layer::Pack),
    ("cadx-ecad-export", Layer::Pack),
    ("cadx-analysis", Layer::Analysis),
    ("cadx-config", Layer::Configuration),
    ("cadx-app", Layer::Application),
    ("cadx-kernel-truck", Layer::Infrastructure),
    ("cadx-io", Layer::Infrastructure),
    ("cadx-ai", Layer::Infrastructure),
    ("cadx-render", Layer::Presentation),
    ("cadx-i18n", Layer::Presentation),
    ("cadx-desktop", Layer::Composition),
    ("cadx-arch", Layer::Contract),
];

/// The layer a crate belongs to, or `None` when it is unclassified.
#[must_use]
pub fn layer_of(name: &str) -> Option<Layer> {
    CRATE_LAYERS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, layer)| *layer)
}

/// Outward dependencies that exist today and are scheduled for inversion.
///
/// A ratchet, not an escape hatch: [`audit_dependencies`] still reports the
/// edge, and [`stale_inversions`] fails once the edge is gone, forcing the entry
/// to be deleted. Entries may only be removed, never added, without an
/// architecture decision recorded in `docs/module-map.md`.
pub const PENDING_INVERSION: &[(&str, &str)] = &[
    // `plan_step_import` consumes cadx-io's parsed STEP DTOs directly. The
    // inversion moves those kernel-neutral DTOs down into cadx-core so the use
    // case depends on the contract instead of the adapter.
    ("cadx-app", "cadx-io"),
];

/// A dependency that points outward or crosses an independent column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerViolation {
    /// The depending crate.
    pub from: String,
    /// Its layer.
    pub from_layer: Layer,
    /// The dependency crate.
    pub to: String,
    /// The dependency's layer.
    pub to_layer: Layer,
}

/// A third-party dependency a layer must stay independent of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TechnologyViolation {
    /// The depending crate.
    pub crate_name: String,
    /// Its layer.
    pub layer: Layer,
    /// The forbidden third-party dependency.
    pub dependency: String,
}

/// Every inward-only rule violation among runtime dependencies.
///
/// Edges listed in [`PENDING_INVERSION`] are excluded.
#[must_use]
pub fn audit_dependencies(workspace: &Workspace) -> Vec<LayerViolation> {
    let mut violations = outward_edges(workspace);
    violations.retain(|violation| !is_pending(&violation.from, &violation.to));
    violations
}

/// Listed inversions that are already done and must be delisted.
#[must_use]
pub fn stale_inversions(workspace: &Workspace) -> Vec<(&'static str, &'static str)> {
    let edges = outward_edges(workspace);
    PENDING_INVERSION
        .iter()
        .copied()
        .filter(|(from, to)| {
            !edges
                .iter()
                .any(|edge| edge.from == *from && edge.to == *to)
        })
        .collect()
}

/// Whether an outward edge is a recorded pending inversion.
#[must_use]
pub fn is_pending(from: &str, to: &str) -> bool {
    PENDING_INVERSION
        .iter()
        .any(|(listed_from, listed_to)| *listed_from == from && *listed_to == to)
}

fn outward_edges(workspace: &Workspace) -> Vec<LayerViolation> {
    let mut violations = Vec::new();
    for manifest in workspace.manifests() {
        let Some(from_layer) = layer_of(&manifest.name) else {
            continue;
        };
        for dependency in &manifest.dependencies {
            let Some(to_layer) = layer_of(dependency) else {
                continue;
            };
            if from_layer.may_depend_on(to_layer) {
                continue;
            }
            violations.push(LayerViolation {
                from: manifest.name.clone(),
                from_layer,
                to: dependency.clone(),
                to_layer,
            });
        }
    }
    violations
}

/// Every forbidden third-party dependency among runtime dependencies.
#[must_use]
pub fn audit_technologies(workspace: &Workspace) -> Vec<TechnologyViolation> {
    let mut violations = Vec::new();
    for manifest in workspace.manifests() {
        let Some(layer) = layer_of(&manifest.name) else {
            continue;
        };
        for dependency in &manifest.dependencies {
            if layer_of(dependency).is_some() {
                continue;
            }
            if layer
                .forbidden_technologies()
                .iter()
                .any(|prefix| dependency.split('-').next() == Some(prefix))
            {
                violations.push(TechnologyViolation {
                    crate_name: manifest.name.clone(),
                    layer,
                    dependency: dependency.clone(),
                });
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_is_a_leaf_and_composition_sees_everything() {
        assert!(!Layer::Geometry.may_depend_on(Layer::Domain));
        assert!(!Layer::Geometry.may_depend_on(Layer::Geometry));
        for (_, layer) in CRATE_LAYERS {
            assert!(Layer::Composition.may_depend_on(*layer));
        }
    }

    #[test]
    fn packs_and_the_geometry_column_stay_independent() {
        assert!(!Layer::Pack.may_depend_on(Layer::Domain));
        assert!(!Layer::Domain.may_depend_on(Layer::Spi));
        assert!(!Layer::Application.may_depend_on(Layer::Infrastructure));
        assert!(Layer::Pack.may_depend_on(Layer::Spi));
    }

    #[test]
    fn inward_layers_forbid_ui_gpu_kernel_provider_and_filesystem() {
        let forbidden = Layer::Domain.forbidden_technologies();
        for expected in ["egui", "wgpu", "truck", "genai", "home"] {
            assert!(
                forbidden.contains(&expected),
                "{expected} must be forbidden"
            );
        }
        assert!(
            !Layer::Infrastructure
                .forbidden_technologies()
                .contains(&"truck")
        );
        assert!(
            Layer::Infrastructure
                .forbidden_technologies()
                .contains(&"egui")
        );
    }

    #[test]
    fn crate_layers_has_no_duplicate_entries() {
        let mut seen = Vec::new();
        for (name, _) in CRATE_LAYERS {
            assert!(!seen.contains(name), "{name} is listed twice");
            seen.push(name);
        }
    }
}
