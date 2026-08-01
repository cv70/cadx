# Persistent Topological Naming

CADX topology references are owned by `cadx-core` and are independent of any
CAD kernel's process-local object identifiers. Faces, edges, and vertices form
a dependency chain of persistent identities.

## Identity Model

Primitive features assign names from their construction semantics:

- boxes use the six local axis sides;
- cylinders and cones use start cap, end cap, and lateral roles;
- extrusions use cap roles, `ProfileSide { segment }` for the outer source
  loop, and `HoleSide { hole, segment }` for explicit inner loops;
- ruled lofts use `StartCap`, `EndCap`, and
  `LoftSide { transition, segment }` from ordered section construction;
- imported STEP shells use deterministic patch numbers in source face order;
- kernel patches created for a single semantic face receive a deterministic
  patch number.

Boolean result faces use `FaceName::Derived`. The name records all upstream
faces whose supporting surface generated the result and a deterministic
fragment number when one source face is split. Derived names may therefore be
traced through multiple feature-graph levels.

`FaceRef` is serializable. It contains no Truck `FaceID`, pointer, traversal
index, tessellation index, or geometric hash. A consumer resolves the reference
against a newly evaluated scene with `EvaluatedScene::face`.

Edges are not assigned an unrelated second naming scheme. An `EdgeRef` stores
its owning feature, the canonical sorted pair of adjacent `FaceRef` values,
and a fragment number when the same face pair shares more than one edge. This
follows adjacent-face identity while making every evaluated edge
individually addressable.

A `VertexRef` stores its owning feature and the canonical set of incident
`EdgeRef` values. A fragment distinguishes the unusual case where multiple
vertices have the same incidence set. References contain topology lineage,
not endpoint coordinates or Truck ids.

## Evaluation Contract

Every `EvaluatedPart` contains:

- one unique `FaceRef` per B-Rep face;
- the supporting surface classification, area, centroid, and mean normal;
- a non-empty, contiguous range of mesh triangle ordinals for each face.
- one unique `EdgeRef` for every exact B-Rep edge, with curve kind, endpoints,
  sampled polyline, length, and numerical precision provenance;
- one unique `VertexRef` for every exact B-Rep vertex and its position.

The ranges partition the part mesh without overlap. This gives picking,
measurement, feature attachment, and manufacturing metadata a common mapping
between render geometry and exact topology.

Truck keeps the same names while rebuilding hidden upstream features. Primitive
names are assigned before feature transforms, so moving or rotating a body does
not change its references. Boolean lineage is recovered from the exact
supporting surfaces copied by Truck's shape operations; fragments are ordered
geometrically rather than by Truck object allocation order. Edge fragments
are ordered by curve geometry and vertex fragments by position. Rigid
transforms and dimension edits retain adjacency-derived names; regression tests
cover primitives and boolean rebuilds.

`resolve_face`, `resolve_edge`, and `resolve_vertex` return
`TopologyResolution::Resolved`, `Ambiguous`, or `Lost`. Convenience accessors
return a value only for a unique resolution. This makes duplicate names a hard
diagnostic rather than silently returning the first kernel entity.

The desktop View toolbar provides face, edge, and vertex selection modes. Edge
and vertex picking uses projected B-Rep samples with a surface-depth check.
Measurement sets retain the same persistent references across rebuilds and are
removed when unique resolution fails. Selected topology and validated
measurement results are sent to AI only as read-only structured context.

## Failure Semantics

Persistent naming does not mean that every reference can survive an arbitrary
topology change. If an edit removes a generating profile segment, deletes a
cap, separates adjacent faces, or changes a boolean so that a fragment
disappears, resolution must return `Lost`. CADX must never choose the first of
multiple exact candidates.

`Primitive::DatumPlane` and `SketchPlane::PlanarFace` persist a `FaceRef`, while
`Primitive::DatumPoint` persists a `VertexRef`. All participate in the
dependency graph, block deletion of their source feature, survive save/load
and undo/redo, and are resolved by the kernel after every rebuild. Resolution
is fail-closed: if a name disappears or resolves ambiguously, the complete
staged transaction is rejected rather than attaching reference geometry or a
sketch to geometrically similar topology.

For sketch-driven extrusions, revolves, and ruled lofts, face roles are derived
from the solved exact sketch segment order. A segment id identifies one stored
Line, Arc, rational quadratic, or cubic Bezier, never a tessellation chord.
Constraint edits are solved before Truck names
the faces; an edit that cannot converge or produces an invalid region is
rejected by the document transaction, while a valid edit may still invalidate a
face whose generating segment no longer exists.

For a holed extrusion, `hole` is the stable index in the sketch's explicit
`holes` list and `segment` is the exact curve index within that hole.
Kernel-only wire reversal does not change either index. If Truck splits one
semantic wall, `HoleSidePatch { hole, segment, patch }` identifies each ordered
patch. Reversing loop winding therefore preserves the complete set of semantic
face references.

Extrusion naming is work-plane aware. Start and end caps are classified by
projection onto the resolved sketch normal. Side faces first match the expected
exact segment sweep area (`segment length * height`), then use distance to the
source Line or Arc to disambiguate equal-area candidates. Duplicate patch
ordering uses the same local frame. No part of this classification assumes
world XY or world Z, so changing a datum offset or applying a rigid source
transform retains the semantic cap and segment names.

`DatumPlane` consumes a persistent planar face and applies a scalar offset along
its oriented normal. A direct `PlanarFace` sketch consumes the same complete
reference but places its origin at the resolved face-centroid projection and
creates no implicit datum. `DatumPoint` consumes a canonical persistent vertex
and applies a three-axis model-space offset to its resolved position. `Chamfer`
and `Fillet` consume a canonical set of persistent edges from one solid: each
resolves every complete adjacent-face pair and fragment after every rebuild,
accepts only unique linear convex edges between planar faces, and names every
generated bevel or cylindrical blend from that edge's two source faces.
Shared-vertex chamfers use an explicit convex-polyhedral corner miter; fillets
remain vertex-disjoint. See [`edge-chamfer.md`](edge-chamfer.md) and
[`edge-fillet.md`](edge-fillet.md). Resolved datum and sketch geometry is
carried beside solid parts in `EvaluatedScene`, so it can be rendered and
picked without entering B-Rep export or physical analysis. See
[`reference-geometry.md`](reference-geometry.md).

Fragment ordering has the same deliberate limitation as face fragments:
perfectly symmetric siblings can exchange deterministic ordinal positions
after a topology-changing edit. CADX currently fails closed for missing or
duplicate full references but does not use a fuzzy coordinate fallback. A
future continuity hint may report such a reorder as ambiguous, but it must not
silently rebind a modeling feature by nearest geometry.
