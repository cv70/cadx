# Persistent Multi-Edge Fillet

CADX supports one constant-radius fillet feature over a canonical set of
persistent topological edges from one solid. The feature stores `Vec<EdgeRef>`
and a radius in millimeters. It never persists a Truck object id, mesh index, or
click coordinate, and resolves every reference whenever the graph rebuilds.

## Supported Geometry

The Truck adapter accepts a fillet only when all of these conditions hold:

- every edge reference resolves uniquely against its source feature;
- the set is non-empty, sorted, unique, and all references share one feature id;
- no two selected edges share a vertex;
- the carrying curve is a finite, non-degenerate line;
- exactly two manifold faces meet at the edge;
- both adjacent faces are planar and consistently oriented;
- the edge is convex; and
- the radius is finite, positive, greater than the modeling tolerance, and
  small enough for the surrounding topology.

Curved or concave edges, curved adjacent surfaces, shared-vertex edge sets, lost
or ambiguous references, and radii that consume neighboring topology fail
closed. CADX does not substitute a nearby edge or return the unchanged source
body. Variable-radius and corner-miter fillets are outside this contract.

## Exact B-Rep Construction

The chamfer and fillet paths share one planar-edge frame. It derives the edge
axis, both outward face normals, both face-local inward directions, and the
exterior bisector from the oriented manifold incidences.

For each selected edge and fillet radius `r`, the cylinder center is the
intersection of the two support planes offset inward by `r`. The two tangency
lines define a transverse extended wedge. Truck subtracts each wedge in
canonical reference order to produce closed, correctly shared trim topology.
CADX then promotes each unique bevel scaffold in place:

- its two endpoint chords become exact rational circular arcs;
- its planar surface becomes the exact swept cylindrical surface; and
- the two longitudinal tangency edges remain shared with the trimmed support
  faces.

Because Truck edge objects are shared by incident faces, replacing each chord
curve also updates the corresponding endpoint face boundary. No mesh geometry
or second approximate solid is introduced. The result must remain a nonempty
closed `Solid`; persistent naming, tessellation, and STEP validation must also
succeed. The entire modifier evaluation is panic-contained.

A 10 mm cube with a 2 mm fillet along one edge has 7 faces, 15 edges, and 10
vertices. Its closed-form volume is
`1000 - 40 * (1 - pi / 4)` mm^3. The regression checks the tessellated analysis
against this value within the default 0.05 mm tessellation tolerance and also
round-trips the exact seven-face B-Rep through STEP.

## Topology Naming

Faces retaining an upstream carrying surface derive their name from that source
face. Every new cylindrical blend derives its name from the faces adjacent to
its selected edge. Fragment ordering uses the same deterministic geometric
ordering as chamfers and booleans.

The kernel regression resizes the source box and verifies that the complete
fillet face-reference set remains unchanged. Lost or ambiguous edge intent
rejects the staged rebuild instead of rebinding geometrically.

## Document And Transaction Semantics

`Primitive::Fillet` has one graph dependency shared by its edge references.
Creation hides the source solid but keeps it in the feature graph, and source
deletion is blocked while the fillet exists. `SetFilletRadius` changes the
common radius parameter.

Fillets were introduced in CADX document schema version 9. Canonical edge sets
use version 10, while the v9 `edge` field remains readable as a one-element
set. Creation and edits
are evaluated against a staged document, so invalid geometry leaves the active
document, evaluated scene, revision, and undo history unchanged.

## Desktop And AI

Select an edge in edge-selection mode, Shift-click to toggle more edges on the
same body, then use Fillet in the Design toolbar. The shared edge-modifier
dialog retains the selected set, radius, and kernel error after a rejected
attempt. The inspector exposes every persistent dependency and supports live
radius editing.

AI can propose `create_fillet` and `set_fillet_radius` using a structured
`edges` array. The system prompt restricts proposals to the supported
vertex-disjoint linear convex edge contract. Every proposal still passes staged
domain and Truck validation and requires ordinary plan approval.

Rejected fillets return a structured `EdgeModifierDiagnostic`, including stable
reason codes for shared vertices, unsupported geometry, lost or ambiguous
references, construction failures, and invalid results. The desktop localizes
this evidence and AI receives it read-only. See
[`edge-modifier-diagnostics.md`](edge-modifier-diagnostics.md).

Truck declares vertex-disjoint fillet support through
`CadKernelCapabilities`; alternate kernels can advertise a broader contract
without changing desktop or AI schemas. See
[`kernel-capabilities.md`](kernel-capabilities.md).

## Deliberate Next Steps

- shared-vertex blends with exact cylinder-intersection and vertex corner patches;
- curved support surfaces and curved carrying edges;
- asymmetric chamfers and variable-radius blend profiles.
