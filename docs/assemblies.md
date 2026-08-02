# Assembly Domain and STEP Occurrences

CADX persists product structure separately from parametric feature history.
`cadx-core::assembly` defines reusable component definitions, placed component
occurrences, stable local ids, source STEP entity identities, and right-handed
rigid transforms. Persisted mates add deterministic motion without introducing
a second product graph. An occurrence may own zero or more concrete solid features;
zero-body occurrences preserve assembly nodes whose product definition has no
direct B-Rep.

## Invariants

Every assembly is validated before a command commits or a document loads:

- assembly, definition, and occurrence ids are non-zero and unique in their
  documented scope;
- names and collection sizes are bounded;
- every occurrence references an existing definition and parent;
- only assembly definitions may contain child occurrences;
- occurrence graphs have at least one root and contain no cycles or unreachable
  cyclic definitions;
- direct suppression is inherited by every descendant, and no active feature
  may depend on a body in the resulting suppressed subtree;
- placements contain finite translations and right-handed orthonormal rotation
  matrices;
- mate ids are non-zero and unique, every mate drives exactly one non-root
  occurrence from its actual hierarchy parent, and no occurrence has multiple
  drivers;
- mate anchor frames are rigid, motion axes are finite unit vectors, scalar
  state and limits are finite and ordered, and state lies inside its limits;
- every mate-driven occurrence's materialized local placement matches the
  deterministic forward-kinematics solution;
- every materialized feature exists, is a solid, and belongs to exactly one
  occurrence across the document; and
- the feature's evaluated translation and Euler rotation reconstruct the same
  world transform as the occurrence hierarchy.

Direct feature move, rotate, and delete commands reject assembly-owned features.
`SetOccurrenceTransform` is the supported editing boundary for an un-driven
occurrence: it replaces one local rigid placement, recomputes the complete
hierarchy, and updates every materialized body in that occurrence and its
descendants in one kernel-validated transaction. A mate-driven occurrence
rejects direct placement edits. Deleting the assembly deliberately releases
its features.

Assembly persistence begins with `.cadx` schema version 24. Version 23 and
earlier documents deserialize with an empty assembly collection and rebuild the
assembly id allocator during validation. Schema version 25 adds direct
occurrence suppression; version 24 documents load occurrences as unsuppressed.
Schema version 26 adds mates; version 25 documents load with an empty mate
collection.

## Assembly Mates and Forward Kinematics

`AssemblyMate` persists one full parent anchor frame, one full child anchor
frame, a kind, and scalar state. Frames map mate coordinates into their
respective occurrence-local coordinates. Fixed, revolute, and slider mates
solve the driven child's local placement as:

```text
parent_frame * motion(kind, state) * inverse(child_frame)
```

Revolute axes are normalized in the parent anchor frame and state is measured
in degrees. Slider axes use the same frame and state is measured in
millimeters. Both kinds accept optional inclusive limits in their state unit;
fixed state is always zero. Full frames preserve orientation as well as anchor
coincidence and avoid the under-constrained point-only convention.

Mates attach only to existing hierarchy edges, so occurrence hierarchy remains
the sole assembly graph. Roots retain their authored local placement. A
deterministic parent-first traversal composes every solved child pose through
arbitrarily nested subassemblies. Suppression does not participate in solving:
it preserves mate state and pose while engineering consumers exclude the
suppressed subtree.

`CreateAssemblyMate` solves and stores the driven local placement.
`SetAssemblyMateState` validates limits, solves the complete hierarchy, and
updates every occurrence-owned descendant feature atomically.
`DeleteAssemblyMate` retains the last solved placement, returning the child to
direct placement control. All three commands pass through the ordinary staged
document, kernel evaluation, and undo/redo boundary. The Desktop inspector can
create fixed or principal-axis revolute/slider mates at the current occurrence
placement, edit state, inspect anchor frames, and delete the mate. AI can
propose the same reviewed commands with arbitrary validated full frames and
unit axes.

## STEP/AP242 Import

`cadx-io` follows the standard product-structure chain:

1. `PRODUCT_DEFINITION` resolves through its formation to a named `PRODUCT`.
2. `PRODUCT_DEFINITION_SHAPE` and `SHAPE_DEFINITION_REPRESENTATION` associate a
   definition with its shape representations and B-Rep bodies.
3. `NEXT_ASSEMBLY_USAGE_OCCURRENCE` provides parent-child definition usage.
4. `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` binds that exact usage to a
   representation relationship.
5. `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` resolves an
   `ITEM_DEFINED_TRANSFORMATION` and its two `AXIS2_PLACEMENT_3D` frames.

The effective local placement is `parent_placement * inverse(child_frame)`.
Translation is converted from the DATA section's declared length unit to
millimeters before persistence. Direction vectors are normalized and
orthogonalized according to `AXIS2_PLACEMENT_3D`; degenerate, reflected,
non-finite, missing, mismatched, or ambiguous relationships fail closed.
Explicit identity `SHAPE_REPRESENTATION_RELATIONSHIP` records are supported.

Definition graphs are expanded from every root. Repeated uses of one product
definition remain distinct occurrences with distinct transforms, and nested
uses compose into world placements. Bodies mapped into product structure are
not also imported as standalone features. Bodies outside the structure retain
the existing flat import behavior.

`cadx-app::plan_step_import` materializes each body occurrence as an ordinary
imported solid feature and appends the complete assembly model to the same
kernel-validated transaction. This keeps current topology picking, booleans,
analysis, and exact export behavior intact while establishing explicit product
identity. The desktop model panel displays assembly and occurrence hierarchy;
the selected feature inspector reports its component definition, occurrence,
source entity, local placement, and mate state. The AI planning tool exposes
the same occurrence and mate commands rather than bypassing ownership with
feature transforms.

## STEP/AP242 Export

When a document contains at least one effectively active assembly occurrence,
Truck exports the current configuration as AP242 product structure. Every CADX
assembly receives a synthetic container product so each authored root
occurrence remains an explicit usage with its complete local rigid transform.
Nested occurrences become canonical `NEXT_ASSEMBLY_USAGE_OCCURRENCE`,
`CONTEXT_DEPENDENT_SHAPE_REPRESENTATION`, and complex
`REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` records. The exported
placements capture the current solved mate state; STEP does not carry the CADX
mate definition, limits, or driver semantics.

For each active component definition and ordered body slot, the representative
world-space Truck solid is inverse-transformed into component-local coordinates
and serialized once. Every repeated occurrence then references that product
definition with its own local placement. Multi-boundary topology retains the
same deterministic disjoint-solid and void-shell partitioning as standalone
export. Visible features outside assembly ownership are emitted as independent
STEP products and remain outside all assembly usages. If no assembly is active,
the existing flat AP214 B-Rep export is retained.

Definition reuse fails closed. All active occurrences of one CADX definition
must have the same ordered body count, geometry primitive, visibility, color,
and active child structure; matching children must retain definition identity,
name, order, and local transform. An incompatible definition returns a STEP
exchange error instead of flattening instances or attaching occurrence-specific
state to one AP242 product. Effectively suppressed occurrences and descendants
are excluded before this comparison and never enter geometry or product
structure. Generated AP242 is parsed and its exact shells are reconstructed by
Truck before export succeeds; conformance regressions also round-trip it through
the kernel-neutral STEP adapter.

## Occurrence Suppression

`SetOccurrenceSuppressed` changes only the direct state of one occurrence. An
effectively suppressed occurrence is one whose own flag or any ancestor flag is
set. Descendant flags are preserved independently, as are body visibility
values, so restoring a parent reveals exactly the subtree state that existed
before suppression.

Suppression removes the complete effective subtree from kernel evaluation,
viewport and topology picking, physical analysis, tessellated exchange, and
exact STEP export. It is not a rendering shortcut. Core rejects a command or
persisted document when an unsuppressed feature directly depends on a
suppressed body; suppressing both the dependent and its source within one
subtree remains valid. The desktop hierarchy and inspector issue the same
undoable command, and AI can only propose that command through the reviewed
plan boundary.

## Definition-Scoped B-Rep Reuse

`CadDocument::assembly_feature_instances` provides a kernel-neutral lookup from
each assembly-owned feature to its assembly, reusable component definition,
occurrence, and ordered body slot. Truck uses that identity during scene
evaluation and STEP export to avoid reparsing and reconstructing identical
embedded STEP geometry for every repeated occurrence.

Reuse is deliberately conservative. A component definition is eligible only
when it has at least two occurrences, every occurrence has the same ordered body
count, and every matching body slot contains an identical `ImportedStep`
primitive. Feature placement, name, color, visibility, and material do not alter
the reusable component-local geometry. Any body-count or primitive mismatch
disables reuse for the complete definition rather than risking an incorrect
binding.

The cache contains the scaled component-local Truck `Solid` and lives for one
evaluation or export attempt. Each occurrence receives a fresh topological
clone, its own world transform, and feature-specific face, edge, and vertex
references. Downstream booleans, picking, analysis, and exact export therefore
continue to consume ordinary per-feature world-space B-Reps.

## Renderer Mesh Instancing

Kernel evaluation also emits a renderer-neutral `EvaluatedMeshDefinition` for
each compatible repeated definition body and an `EvaluatedMeshInstance` for
every visible occurrence. Definitions contain one component-local triangle
mesh keyed by assembly, component definition, and ordered body slot. Instances
carry the owning feature id and its world-space rigid transform. This follows
the `vcad` definition/instance split without exposing WGPU types through the
kernel port.

Renderer eligibility includes identical imported STEP bodies and identical
dependency-free native boxes, cylinders, spheres, cones, toruses, and direct
extrusions. Dependency-bearing or mismatched bodies continue through the
ordinary per-part path. `cadx-render` uploads every eligible definition once,
stores per-occurrence model matrices and colors in an instance buffer, and
issues one indexed instanced draw per definition body. Selected or measured
faces use small world-space overlay batches, so feature-specific topology
highlighting remains exact while the base mesh stays shared.

`EvaluatedPart` and its world-space mesh remain authoritative for picking,
measurement, mass properties, STL/3MF export, and other engineering consumers.
The render definitions are an additional optimization contract rather than a
replacement that would force transforms into those layers.

## Product Interference

Truck reuses the same one-pass B-Rep materialization for product interference.
Effective suppression removes occurrence bodies; feature visibility does not,
because it is presentation state. Terminal Boolean and edge-modifier results
replace their consumed feature-history inputs in the candidate set. Bodies in
one multi-body occurrence are treated as one product and are not self-paired,
while distinct repeated occurrences remain independent candidates.

Reports use deterministic feature ordering, exact B-Rep bounds, AABB broad
phase, bounded contact and containment classification, and Truck B-Rep
intersection. Pair outcomes retain method, volume precision, and typed failure
evidence without exposing Truck objects. Definition-scoped STEP reconstruction
still occurs once during the combined evaluation and analysis pass. See
[`interference-analysis.md`](interference-analysis.md).

## Current Boundary

Source B-Rep reconstruction, AP242 definition geometry, and GPU render meshes
can be shared, but transformed B-Rep cloning, feature topology, and engineering
world meshes still occur per occurrence during ordinary evaluation. Sharing
tessellation across topology consumers requires a local-space topology contract
rather than renderer shortcuts. Broader multi-degree-of-freedom joints,
closed-loop constraint solving, physics, instance-level overrides, BOM/PMI, and
assembly-aware topological references require their own domain contracts. The
persisted definition/occurrence/mate model is the stable base for those
features.
