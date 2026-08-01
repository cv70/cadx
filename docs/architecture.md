# CADX Architecture

CADX uses a layered workspace with inward-owned contracts. Domain and use-case
code do not depend on desktop frameworks, provider SDKs, concrete kernels, or
filesystem implementations.

## Crate Map

| Layer | Crate | Responsibility |
| --- | --- | --- |
| Geometry foundation | `cadx-sketch` | Exact kernel-neutral Line/Arc/rational-quadratic/cubic regions, curve validation, and capability-routed deterministic projection or bounded nonlinear constraints |
| Domain and ports | `cadx-core` | Parametric document, commands, invariants, persistent topology references, kernel-neutral meshes, kernel ports, pure document codec |
| Analysis | `cadx-analysis` | Read-only geometric, measurement, and physical analysis over evaluated scenes: topology relationships, bounds, area, volume, mass, center of mass, and inertia |
| Application | `cadx-app` | Kernel-validated transactions, document session, revisions, undo/redo, clean-state tracking |
| Configuration | `cadx-config` | Typed YAML models and the injectable `~/.cadx` store |
| Infrastructure | `cadx-kernel-truck` | Truck B-Rep evaluation, STEP reconstruction, booleans, tessellation, exact STEP encoding |
| Infrastructure | `cadx-io` | Atomic `.cadx` IO and validated STEP import/export, STL, and 3MF adapters |
| Infrastructure | `cadx-ai` | Provider-neutral AI contract and explicit rust-genai adapter configuration |
| Presentation | `cadx-render` | Kernel-neutral egui/wgpu viewport rendering and picking |
| Presentation | `cadx-i18n` | Translation resources and runtime language selection |
| Composition root | `cadx-desktop` | egui interaction, dialogs, asynchronous task wiring, and adapter construction |

## Dependency Direction

```text
cadx-sketch
    ^
cadx-core
    ^
    +-- cadx-analysis
    +-- cadx-app
    +-- cadx-io
    +-- cadx-kernel-truck
    +-- cadx-ai
    +-- cadx-render

cadx-config <--- cadx-ai

cadx-analysis <--- cadx-ai

cadx-desktop ---> cadx-app + all required adapters
```

`cadx-kernel-truck` uses `cadx-io` only as a development dependency for STEP
conformance tests. Production adapters do not depend on one another.

## State Changes

1. Presentation creates declarative `ModelCommand` values.
2. `DocumentSession` applies the complete batch to a cloned document.
3. The configured `CadKernel` evaluates the staged document.
4. Only a successful evaluation replaces active state and creates one revision.
5. The viewport consumes the committed `EvaluatedScene`.

Each evaluated solid also exposes kernel-neutral face, edge, and vertex
indexes. Persistent references are generated from feature semantics, topology
adjacency, and boolean lineage, then resolved against a rebuilt scene rather
than a kernel object id. Resolution explicitly distinguishes unique,
ambiguous, and lost references. The complete identity and invalidation
contract is documented in
[`topological-naming.md`](topological-naming.md).

Viewport face picking resolves a triangle ordinal through the evaluated face
partition. Edge and vertex picking use exact B-Rep curve samples and vertex
positions projected into screen space, with surface-depth rejection to avoid
selecting hidden topology. `EvaluatedScene` separates solid `parts`, resolved
`datum_planes`, and resolved `datum_points`. Datum features store a `FaceRef`
or `VertexRef` as an ordinary graph dependency. Truck resolves those references
against rebuilt upstream B-Rep topology and emits kernel-neutral reference
geometry, never a fake solid. The renderer draws these values as line overlays,
includes them in frame-all bounds, and supports feature picking on the overlay.
See [`reference-geometry.md`](reference-geometry.md).

Chamfer and fillet features apply the same rule to ordered `EdgeRef` sets: the
application stores topological intent, while Truck resolves every edge against
one source solid before constructing a real B-Rep modifier during staged
evaluation. Both operations share one planar-edge frame and accept convex
linear edges between planar faces. Vertex-disjoint chamfers use staged wedge
subtraction. Shared-vertex chamfers on a single-shell convex polyhedron use an
explicit half-space intersection and compressed-topology rebuild, so corner
miters do not depend on boolean execution order. Fillet promotes each trimmed
bevel scaffold to exact circular boundary curves and a cylindrical surface;
shared-vertex fillet sets remain unsupported. Unsupported geometry is rejected
before shape operations without rebinding. See [`edge-chamfer.md`](edge-chamfer.md)
and [`edge-fillet.md`](edge-fillet.md).

The same path is used for direct edits and approved AI plans. AI receives a
document snapshot plus an optional read-only `cadx-analysis` scene summary and
can only propose commands; it cannot access mutable application state, the
kernel, files, or the renderer. Material changes use the same command boundary.
Analysis is computed from the committed evaluated scene and never becomes an
alternate source of geometry truth.

`CadKernelCapabilities` exposes operation availability and edge-modifier
geometry restrictions without leaking a concrete backend. Its default disables
modifiers until an adapter opts in. The desktop uses it for command
availability and AI receives it as read-only planning context; staged kernel
validation remains authoritative for each actual selection. See
[`kernel-capabilities.md`](kernel-capabilities.md).

Failed booleans and edge treatments cross the same boundaries as typed
`BooleanDiagnostic` and `EdgeModifierDiagnostic` values. `cadx-kernel-truck`
classifies only failures it can observe, `cadx-app` retains the diagnostic while
rejecting the staged transaction, and `cadx-desktop` renders localized
stage/reason fields and structured evidence. The last diagnostic can enter AI
context as read-only evidence; neither UI nor AI parses backend text to recover
a cause. See [`boolean-diagnostics.md`](boolean-diagnostics.md) and
[`edge-modifier-diagnostics.md`](edge-modifier-diagnostics.md).

Boolean tolerance is a validated `cadx-core` policy rather than an adapter
literal. Truck derives a bounded deterministic attempt sequence from finite
operand bounds, validates both operands before shape operations, validates the
closed-manifold result before naming, and records per-attempt healing evidence.
Its current bounded recovery includes lossless compressed-topology
normalization, representation-aware identity for planar and curved analytic
solids, and strict single-interface planar contact classification. Exact
curved boundary edges can be retained when the interface does not move. A
nonzero gap uses local Line/Plane repair, a single-boundary refit of a clamped
B-spline/NURBS homotopy surface whose cross-control sequences are proven affine
at normalized Greville abscissae, or one proven rigid transform of the complete
right B-Rep. This proof includes degree-elevated multi-row ruled surfaces and
requires constant finite NURBS weight along each cross sequence. Refitted and
transformed source surfaces carry explicit naming lineage. Non-affine freeform
deformation and curved contact surfaces remain outside this bounded proof.
Every union must extract one validated closed shell. `boolean/contact.rs` owns
contact classification and topology sewing; `boolean/contact/refit.rs` owns
control-net eligibility,
weight-preserving boundary edits, and surface replacement. The policy boundary
admits stronger future kernel adapters without exposing their B-Rep types or
weakening atomic transactions. Versioned fixtures and STEP integration
regressions keep both successful and failed backend behavior reproducible
across dependency and policy changes.

Transient measurement selections contain persistent topology references, not
kernel ids or copied UI coordinates. `cadx-analysis` resolves those references
against the committed scene and returns typed results for edge length, point
distance, linear-edge angle, and planar-face relationships. Truck supplies
analytic plane equations and curve-length precision metadata through
`cadx-core`; unsupported geometry and uncertain accuracy fail closed. A valid
active result is included in AI context as read-only evidence. See
[`measurement.md`](measurement.md).

`cadx-analysis` is deliberately a consumer of `EvaluatedScene`, not of Truck
types. `EvaluatedPart` carries an optional copy of kernel-neutral material
metadata from its feature. Analysis reports metric units explicitly (`mm`,
`mm^2`, `mm^3`, `kg`, and `kg mm^2`) and rejects malformed meshes, invalid
density, non-finite coordinates, and out-of-bounds indices rather than
returning plausible-looking numbers. See
[`materials-and-mass-properties.md`](materials-and-mass-properties.md).

Sketch geometry follows the same command boundary. `cadx-core` stores one
exact `SketchRegion2D`, an independent non-solid construction segment array,
and an ordered constraint list. Profile ids retain the legacy prefix while
construction segment and endpoint ids append deterministically; holes are never
constraint entities. `cadx-sketch` validates exact curve continuity,
intersection, containment, construction geometry, and bounded complexity.
Solver routing follows capabilities rather than segment kind alone: a
construction-free Line region using Coincident, Horizontal, Vertical, Fixed,
and Distance retains deterministic projection. Construction, Length,
EqualLength, Parallel, Perpendicular, directed Angle, PointOnCurve, Midpoint,
Symmetric, signed horizontal/vertical point dimensions, point-line distance,
line-through-center, or any Arc uses bounded Levenberg-Marquardt over shared
profile vertices, independent construction endpoints, arc centers, and
segment-local Bezier controls. Entity
validation, bounded iterations, residual tolerance, finite-curve membership,
and final exact validation all fail closed. The converged numerical Jacobian is
ranked incrementally to expose remaining DOF and ordered redundant constraints;
failed systems retain typed conflict indices without committing their partial
iterate. `SetSketchDefinition` stages region, construction, and constraints as
one command. Before commit, Core validates every proposed linked extrusion,
revolve, and ordered loft cache, including exact Arc extrema against a revolve
axis and segment-count/winding compatibility across every loft section.

Truck solves the source again at evaluation time, so the region remains the
single editable truth. It builds Lines directly, circular Arcs and rational
quadratics as NURBS, and cubics as B-splines, attaches inner wires to one exact
planar face, and assigns kernel-neutral
outer and hole-side topology names. Extrusion naming must cover both cap roles
and every persistent outer and hole segment before patch disambiguation; missing
semantic coverage is a kernel error rather than a fallback name. Solved
construction is emitted only as an open `EvaluatedSketch` overlay; it is never
passed to B-Rep construction, export, topology naming, or analysis. `SketchPlane`
remains kernel-neutral and stores a world-plane choice, a DatumPlane dependency,
or a complete persistent planar `FaceRef`. Truck resolves the analytic frame,
maps exact curves through it, and only then samples a separate `EvaluatedSketch`
display polyline for the renderer. Tessellation and overlay sampling never flow
back into persistence, topological naming, or STEP construction. Hidden sources
are evaluated for dependency resolution without entering the visible scene or
export.

Ruled loft extends the same source-of-truth rule across 2 to 32 ordered sketch
dependencies. Core retains one solved cache per source and synchronizes every
affected cache in the same sketch-edit transaction. Truck resolves all section
frames in model space, proves monotonic centroid advance and consistent
non-tangent area direction, and constructs exact adjacent-wire homotopies. A
valid result must be one geometrically consistent closed shell. Face roles are
assigned directly as `StartCap`, `EndCap`, and
`LoftSide { transition, segment }`; generic primitive naming is forbidden for
this feature. See [`loft.md`](loft.md).
The same evaluated sketch carries ordered, kernel-neutral annotation anchors
derived from the committed exact solution. `cadx-render` owns camera projection,
fixed-pixel drafting layout, collision avoidance, painting, and label hit tests.
`cadx-desktop` owns the transient dimension editor and replaces one validated
dimension through `SetSketchDefinition`; neither layer can write into evaluated
scene data. The selected sketch's indexed driving dimensions enter AI context as
read-only values, while reviewed commands and staged kernel evaluation remain
the only mutation path.
The constraint semantics and file-compatibility contract are documented in
[`sketch.md`](sketch.md).

## Storage Boundaries

- `cadx-core::persistence` is a pure, versioned JSON codec with validation.
- `cadx-io` owns document paths, reads, and atomic writes.
- Imported STEP features embed their validated physical-file source and the
  source shell entity id. This keeps a CADX document portable while leaving
  STEP parsing and Truck geometry conversion behind their respective adapters.
- `cadx-config::ConfigStore` owns `config.yaml` and `preferences.yaml` below an
  explicit root. Production discovers exactly `~/.cadx`; tests inject a root.
- Provider configuration and software preferences are never sourced from
  environment variables. The platform home lookup only locates `~/.cadx`.
- Provider secrets are redacted from diagnostics. Atomic settings files use
  owner-only creation permissions on Unix.

## Composition Rules

- Concrete adapters are constructed only in `cadx-desktop`.
- Domain code cannot perform IO or spawn asynchronous work.
- Application state transitions cannot depend on egui types or file dialogs.
- Kernel-native topology cannot cross the `CadKernel` or `ExchangeKernel` ports.
- Persistent references cannot contain kernel object ids or mesh traversal
  indices. Edges derive from adjacent faces and vertices derive from incident
  edges.
- Face- and vertex-dependent features must fail closed when their persistent
  reference no longer resolves uniquely after a topology-changing edit.
- External data is parsed and validated before replacing active state or a
  destination file.
