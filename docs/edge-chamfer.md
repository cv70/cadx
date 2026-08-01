# Persistent Multi-Edge Chamfer

CADX supports one equal-distance chamfer feature over a canonical set of
persistent topological edges. The feature stores `Vec<EdgeRef>` and a distance
in millimeters; it never stores a Truck object id, mesh index, or viewport
coordinate. Every reference is resolved again when the dependency graph
rebuilds. All edges must belong to one source solid.

## Supported Geometry

The Truck adapter currently accepts a chamfer only when all of these conditions
hold:

- every edge reference resolves uniquely against its source feature;
- the set is non-empty, sorted, unique, and all references share one feature id;
- the carrying curve is a finite, non-degenerate line;
- exactly two manifold faces meet at the edge;
- both adjacent faces are planar and have consistent outward orientation;
- the edge is convex; and
- the distance is finite, positive, and greater than the modeling tolerance.

Vertex-disjoint selections can modify any source accepted by the ordinary
planar-edge path. When selected edges share a vertex, the source must be one
closed, convex, all-planar shell with linear edges and one simple boundary per
face. Curved edges, curved adjacent surfaces, concave shared-vertex sources,
ambiguous references, and lost references fail closed. CADX does not
reinterpret them as the nearest available edge. Constant-radius rounding is a
separate [`Fillet`](edge-fillet.md) feature with a narrower corner contract.

## B-Rep Construction

For each selected edge, CADX derives the inward direction on both adjacent
faces from the oriented manifold incidence. The requested distance places one
setback line on each face. Those points define that edge's new planar bevel.

Each cutter is an extended wedge prism swept past both edge endpoints. Its cut
line extends beyond both support faces, which keeps the intersection transverse
and avoids placing cutter vertices exactly on the source boundary. Truck
subtracts vertex-disjoint cutters in canonical reference order inside one
staged feature.

Shared-vertex sets do not use overlapping boolean cutters. CADX forms an
outward half-space for every source face and selected-edge setback plane,
enumerates the finite intersections of every plane triple, and keeps only
points inside all half-spaces. Each face polygon is ordered around its outward
normal, then the complete solid is rebuilt through Truck's compressed topology
with globally shared vertices and edges. `Shell::extract` checks wire
connectivity and `Solid::try_new` checks closed orientation. This produces an
explicit deterministic corner miter for two or three edges meeting at a vertex.

The result is accepted only when Truck returns a nonempty solid and
`Solid::try_new` confirms closed manifold topology. Persistent face naming and
the ordinary evaluated-topology pipeline must also succeed. Kernel calls and
the enclosing chamfer evaluation are panic-contained.

For a 10 mm cube with a 2 mm chamfer along one edge, the result is gated at 980
mm^3 with 7 faces, 15 edges, and 10 vertices. The original corner line is absent.
Two perpendicular edges sharing a corner produce 8 faces, 17 edges, 11 vertices,
and `1000 - (40 - 8/3)` mm^3. Three incident edges produce 9 faces, 21 edges,
14 vertices, an explicit three-plane vertex at `(9, 9, 9)`, and 946 mm^3. The
regressions also validate STEP output and a non-axis-aligned convex triangular
prism.

## Topology Naming

Faces retaining an upstream carrying surface receive a derived name from that
source face. Every new bevel receives a derived name from the two faces adjacent
to its own selected edge. Fragment ordering uses the same deterministic
geometric ordering as boolean results.

This makes the feature rebuild-stable when an upstream parameter edit preserves
the source names. Kernel tests resize both a single-edge box and a three-edge
corner miter and verify that the complete chamfer face-reference sets remain
unchanged.

## Document And Transaction Semantics

`Primitive::Chamfer` has one graph dependency shared by all its edge references.
Creating the feature hides the source body but retains it in the graph; deleting
the source is blocked while the chamfer depends on it. `SetChamferDistance`
edits the common parameter without changing edge intent.

Chamfers were introduced in CADX document schema version 8. Canonical edge sets
were introduced in version 10; versions 8 and 9 using `edge` decode as a
single-element `edges` set. A command is always
evaluated against a staged document, so an invalid distance, lost reference, or
kernel rejection leaves the active document, scene, revision, and undo history
unchanged.

## Desktop And AI

Select an edge in the View toolbar, Shift-click to toggle more edges on the same
body, then use Chamfer in the Design toolbar. The dialog retains the complete
set and distance after a failed evaluation. The feature inspector shows every
persistent edge reference and provides live distance editing.

AI receives selected edges as read-only structured context. It can propose
`create_chamfer` with an `edges` array and `set_chamfer_distance`, but those
commands still pass the same staged domain and Truck validation and require
ordinary plan approval.

Rejected chamfers return a structured `EdgeModifierDiagnostic`. The desktop
shows localized stage/reason fields and the relevant parameter, source, edge
set, tolerance, and offending-edge evidence; AI receives the same value as
read-only context. See
[`edge-modifier-diagnostics.md`](edge-modifier-diagnostics.md).

Truck advertises this geometry contract through `CadKernelCapabilities`, so
desktop and AI adapters do not hard-code backend support. See
[`kernel-capabilities.md`](kernel-capabilities.md).

## Deliberate Next Steps

- shared-vertex fillets with exact cylinder-intersection and vertex corner patches;
- corner miters on non-convex and mixed curved/planar source shells.
