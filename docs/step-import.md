# STEP Import Contract

CADX imports STEP physical files as portable, rebuildable B-Rep features. The
original exchange text remains embedded in the `.cadx` document; kernel-native
objects and source filesystem paths never enter persistence.

## Body Identity

`cadx-io` parses the complete physical file and discovers solid ownership in
every DATA section. `MANIFOLD_SOLID_BREP` and `FACETED_BREP` contribute one
outer `CLOSED_SHELL`. `BREP_WITH_VOIDS` contributes one outer shell plus its ordered
`ORIENTED_CLOSED_SHELL` inner boundaries; those inner shells are not emitted as
independent bodies. Closed shells that are not owned by either solid form, or
that occur in a shell-based surface model, remain standalone import bodies.

An imported body persists the zero-based DATA-section index, outer shell entity
id, and every void's underlying shell id and effective orientation. Truck can
therefore reconstruct the declared solid instead of assuming the first DATA
section or manufacturing one solid per shell. Missing, duplicate, multiply
owned, non-closed, or malformed boundary references fail before document
commands are submitted. The outer shell count and bounded void list are also
validated by `cadx-core` when loading or applying commands.

When a solid or surface-model entity supplies a non-empty name, CADX uses it as
the feature name. Otherwise the desktop assigns a deterministic filename and
body-index fallback. Duplicate names are allowed because feature ids remain the
document identity.

## Length Units

CADX model space is millimeters. Import recognizes an assigned STEP length unit
from `GLOBAL_UNIT_ASSIGNED_CONTEXT` and persists:

- the source unit name;
- the finite positive millimeters-per-source-unit factor; and
- whether the unit was declared or assumed.

SI metre units with standard prefixes from atto through exa are supported.
`CONVERSION_BASED_UNIT` is supported when its length measure resolves through a
finite positive conversion chain to an SI length unit, including common inch
definitions. Reference cycles, malformed conversions, and conflicting assigned
length units fail closed before any document command is submitted.

Legacy and unitless files are interpreted as millimeters and marked as assumed
in the inspector. This preserves documents written before schema version 23
without presenting the assumption as source metadata.

Truck reconstructs every source boundary in its native numeric coordinates,
applies each oriented void-shell flag, creates one checked multi-boundary
`Solid`, then uniformly scales the exact B-Rep to millimeters before topology
naming, tessellation, measurement, booleans, material analysis, or re-export. A
unit is therefore not a display preference: all downstream geometry consumes
the same converted solid.

## Presentation Colors

The STEP adapter follows `STYLED_ITEM` presentation assignments through
standard surface-style, fill-area-style, RGB/predefined-color, and transparency
entities. A style targeting `MANIFOLD_SOLID_BREP`, `FACETED_BREP`, or
`BREP_WITH_VOIDS` becomes the default for all of that solid's boundaries.
Shell, oriented-shell, and face styles override that default only when the
resulting complete body remains one uniform color.

CADX currently stores one RGBA color per feature. Boundary- or face-level styles
are promoted only when every styled boundary resolves to the same effective
color and unstyled descendants can inherit an explicit ancestor style. Partial
coverage, conflicting assignments, malformed style graphs, and genuinely mixed
face colors are reported as unsupported instead of being flattened into a
misleading body color. Geometry remains importable and the desktop reports how
many bodies use a CADX default color for this reason.

STEP export writes one entity-level presentation assignment per visible solid,
including alpha through `SURFACE_STYLE_TRANSPARENT`. Before serialization,
multi-boundary results are classified by signed shell orientation and
tolerance-bounded tessellated containment. Disjoint outer shells become separate
`MANIFOLD_SOLID_BREP` entities; opposite-oriented contained shells remain one
`BREP_WITH_VOIDS`. A cavity that cannot be assigned unambiguously fails export
instead of being mislabeled. Exported colors are linked to the generated B-Rep
entity ids after parsing the generated DATA section; no Truck object id or mesh
index crosses the exchange boundary. The complete file is then
topology-validated as before.

When active CADX assembly occurrences exist, the same exact bodies and styles
are wrapped in AP242 product definitions and canonical occurrence relationships.
One representative body per compatible definition/body slot is converted from
its evaluated world placement back to component-local coordinates; repeated
uses carry full rigid local transforms instead of duplicate B-Reps. Synthetic
assembly-container products preserve authored root placements, standalone
visible features remain independent products, and effectively suppressed
subtrees are omitted. Export fails when repeated occurrences disagree in
geometry, visibility, color, or active child structure because one AP242
definition cannot represent those occurrence-specific differences safely.

## Assembly Occurrences

When a DATA section contains product structure, `cadx-io` follows
`PRODUCT_DEFINITION` and `NEXT_ASSEMBLY_USAGE_OCCURRENCE` links into reusable
component definitions and placed occurrences. The canonical
`CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` link selects the representation
relationship for each exact usage. A fallback is accepted only when the
parent-child representation pair identifies one relationship unambiguously;
missing, malformed, ambiguous, or mismatched links fail closed.

`ITEM_DEFINED_TRANSFORMATION` placements are reconstructed from their two
`AXIS2_PLACEMENT_3D` frames as `parent_placement * inverse(child_frame)` and
converted to millimeters. Repeated and nested uses materialize distinct body
features with composed world transforms while retaining shared component
definition identity. Bodies owned by occurrences are excluded from the flat
standalone import set, so one source B-Rep is not imported twice.

The persisted assembly records stable definition and occurrence ids, source
STEP entity identities, hierarchy, direct suppression state, and local rigid
transforms. Assembly-owned features cannot be moved, rotated, or deleted
independently. Local occurrence edits recompute all materialized descendant
bodies atomically; suppression excludes the effective descendant subtree from
evaluation and exchange without changing feature visibility. Deleting the
assembly releases its bodies. See [`assemblies.md`](assemblies.md) for the
complete domain contract and current boundaries.

## Atomicity and Failure Semantics

`cadx-app::plan_step_import` creates one `ImportStep` command for every
standalone body and every materialized body occurrence, predicts their stable
feature ids, applies occurrence rotations, and appends the complete assembly
model to the same transaction. `DocumentSession` commits none of it if a DATA
section is missing, a boundary cannot be resolved or converted, an assembly
relationship is invalid, or any body fails kernel validation. Imported solids
retain ordinary topology, analysis, boolean, and exact-export behavior.

Current scope does not preserve genuinely different colors per face, import or
export PMI/manufacturing metadata, retain one mutable transformed B-Rep/topology
object across occurrences, or encode CADX mate constraints as STEP semantics.
AP242 export captures the currently solved occurrence configuration.
Unsupported exchange semantics are not flattened into misleading feature
metadata.
