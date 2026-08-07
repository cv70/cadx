# CADX Architecture

CADX uses a layered workspace with inward-owned contracts. Domain and use-case
code do not depend on desktop frameworks, provider SDKs, concrete kernels, or
filesystem implementations.

The rules on this page are prose. The parts a machine can check — layer
classification, inward-only dependencies, forbidden technologies per layer, and
the per-file size budget — are enforced by `cargo test -p cadx-arch` and
tabulated in [`module-map.md`](module-map.md).

## Industrial AI-CAD Architecture
```
┌──────────────────────────────────────────────────────────────────────────────────┐
│                                 UI Layer (egui)                                  │
│  ┌─────────────────────────┬─────────────────────────┬────────────────────────┐  │
│  │   wgpu Central Viewport │    Domain Inspectors    │   AI Chat & Agent UI   │  │
│  │   (3D/2D Multi-Canvas)  │  (Schema Driven Forms)  │ (Intent Bar / Command) │  │
│  └────────────┬────────────┴────────────┬────────────┴────────────┬───────────┘  │
└───────────────┼─────────────────────────┼─────────────────────────┼──────────────┘
                │ Interactivity/Gizmo     │ Form Change             │ Prompt/Cancel Token
                ▼                         ▼                         ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│                      Command / Transaction Pipeline (Core Bus)                   │
│  ┌───────────────────────────┬─────────────────────────┬──────────────────────┐  │
│  │ Transaction Manager       │ Undo/Redo Engine        │ Event Dispatcher     │  │
│  │ (Atomic, Streamable, CoW) │ (Delta-based History)   │ (Publish-Subscribe)  │  │
│  └───────────────────────────┴─────────────────────────┴──────────────────────┘  │
└───────────────┬───────────────────────────────────────────────────▲──────────────┘
                │ Stream / Apply Commands                           │ Streamed AI Cmds
                ▼                                                   │
┌──────────────────────────────────────────────┐  ┌─────────────────┴──────────────┐
│       Core Engine (Data & Geometry)          │  │    AI Native Engine (Agentic)  │
│ ┌──────────────────────────────────────────┐ │  │ ┌─────────────────────────────┐ │
│ │ Document Core (ECS: bevy_ecs)            │ │  │ │ Dynamic Context Collector   │ │
│ │  - Entities & Component Repositories     │ │  │ │  (Spatial, Focus, Topology) │ │
│ │  - Dirty Mark & Change Detection         │◄┼──┼─┤  (Vector/RAG Index Engine)    │ │
│ ├──────────────────────────────────────────┤ │  │ └──────────────┬──────────────┘ │
│ │ Geometry Kernel (Multi-threaded Workers) │ │  │                ▼                │
│ │  - B-Rep / Parametric Engine             │ │  │ ┌─────────────────────────────┐ │
│ │  - Rayon Parallel Constraint Solver     │ │  │ │ AI Planner & Tool Router    │ │
│ │  - Multi-LOD Tessellator (Async Engine)  │ │  │ │  (Tool Calling, RAG, LLM)   │ │
│ └─────────────────────┬────────────────────┘ │  │ └──────────────┬──────────────┘ │
│                       │ Sync Sparse Buffers  │  │                ▼                │
│ ┌─────────────────────▼────────────────────┐ │  │ ┌─────────────────────────────┐ │
│ │ Spatial Index (BVH / R-Tree Cache)       │ │  │ │ Ghost Sandbox (CoW Delta)   │ │
│ └──────────────────────────────────────────┘ │  │ │  (Preview & Diff Engine)    │ │
└───────────────────────┬──────────────────────┘  │ └──────────────┬──────────────┘ │
                        │                         └────────────────┼────────────────┘
                        │ Sync Render Data                         │ Ghost Geometry
                        ▼                                          │ Render Data
┌──────────────────────────────────────────────┐                   │
│        wgpu Pipeline (GPU Acceleration)      │                   │
│ ┌─────────────────────┬────────────────────┐ │                   │
│ │ Render Pass         │ Compute Pass       │◄┼───────────────────┘
│ │ - Instanced Mesh/Line│ - GPU Ray Picking  │ │
│ │ - Ghost/Overlay Pass│ - GPU DRC / Clash  │ │
│ │ - LOD Mesh Shading  │ - Spatial Sorting  │ │
│ └─────────────────────┴────────────────────┘ │
└───────────────────────▲──────────────────────┘
                        │ Pack Pipeline / Shader / Tool Registration
┌───────────────────────┴──────────────────────────────────────────────────────────┐
│                         Domain Pack SDK (Plugin Core)                            │
│  ┌────────────────────────────────────────────────────────────────────────────┐  │
│  │ Pack Trait Interface (Schema Injection, Solvers, Shaders, AI Tools)        │  │
│  └──────────────┬─────────────────────┬──────────────────────┬────────────────┘  │
│                 │                     │                      │                   │
│  ┌──────────────┴──────────┐ ┌────────┴─────────────┐ ┌──────┴─────────────────┐  │
│  │        MCAD Pack        │ │      AEC Pack        │ │      ECAD Pack        │  │
│  │ - Feature Tree          │ │ - BIM Attributes     │ │ - Netlist & Layers    │  │
│  │ - Extrude/Fillet Tools  │ │ - Wall/Slab Solvers  │ │ - Router & DRC Tools  │  │
│  │ - Assembly Constraints  │ │ - IFC Standard Data  │ │ - Footprint Library   │  │
│  └─────────────────────────┘ └──────────────────────┘ └───────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────────┘
```

## Crate Map

| Layer | Crate | Responsibility |
| --- | --- | --- |
| Geometry foundation | `cadx-sketch` | Exact kernel-neutral Line/Arc/rational-quadratic/cubic regions, curve validation, and capability-routed deterministic projection or bounded nonlinear constraints |
| Domain and ports | `cadx-core` | Parametric document, explicit assembly definitions and occurrences, commands, invariants, persistent topology references, kernel-neutral meshes, kernel ports, pure document codec |
| Domain Pack SPI | `cadx-domain-api` | Geometry-neutral `DomainPack` trait, runtime registry, typed schema validation, executable tool requests, solver/shader/AI descriptors, actions, diagnostics, artifacts, NL routes, and export contracts |
| MCAD pack | `cadx-mcad-*` | Feature dependency regeneration, assembly constraint proposals, sketch/extrude/chamfer/fillet tools, GB/T/ISO/ASME standards, DFM, grouped BOM, and standard parts |
| AEC pack | `cadx-aec-*` | Validated BIM models, parametric wall/slab actions, levels, schedules, quantities, spatial clash analysis, and deterministic IFC4/IFC4X3 exchange |
| ECAD pack | `cadx-ecad-*` | Validated schematic netlists, copper/dielectric stackups, footprints, placement, orthogonal routing, pad/via/trace DRC, and Gerber/drill export |
| Analysis | `cadx-analysis` | Read-only geometric, measurement, and physical analysis over evaluated scenes: topology relationships, bounds, area, volume, mass, center of mass, and inertia |
| Application | `cadx-app` | `CoreBus` source tagging, streamable command buffering, event dispatch, kernel-validated transactions, STEP import planning, document session, revisions, undo/redo, clean-state tracking |
| Configuration | `cadx-config` | Typed YAML models and the injectable `~/.cadx` store |
| Infrastructure | `cadx-kernel-truck` | Truck B-Rep evaluation, STEP reconstruction, booleans, product interference, tessellation, exact STEP encoding |
| Infrastructure | `cadx-io` | Atomic `.cadx` IO and validated STEP import/export, STL, and 3MF adapters |
| Infrastructure | `cadx-ai` | Provider-neutral AI contract, context collector, domain tool registry, intent diff, and explicit rust-genai adapter configuration |
| Presentation | `cadx-render` | Kernel-neutral egui/wgpu viewport rendering and picking |
| Presentation | `cadx-i18n` | Translation resources and runtime language selection |
| Composition root | `cadx-desktop` | egui interaction, dialogs, asynchronous task wiring, and adapter construction |
| Architecture contract | `cadx-arch` | The executable layering contract: layer classification, inward-only dependency audit, per-layer forbidden technologies, and the per-file size budget |

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

cadx-domain-api <--- cadx-mcad-* + cadx-aec-* + cadx-ecad-*
cadx-domain-api <--- cadx-desktop

cadx-analysis <--- cadx-ai

cadx-desktop ---> cadx-app + all required adapters
```

`cadx-kernel-truck` uses `cadx-io` only as a development dependency for STEP
conformance tests. Production adapters do not depend on one another.

`cadx-arch` depends on nothing. It reads the workspace manifests and sources
from disk rather than linking against them, so a rule added there can only fail
the build — it can never change shipped behavior. Two ratchets record known
debt: `PENDING_INVERSION` for outward crate edges that still exist, and
`PENDING_DECOMPOSITION` for files still over the line budget. Both lists may
only shrink; a dedicated test fails once an entry becomes unnecessary, forcing it
to be deleted. See [`module-map.md`](module-map.md).

## State Changes

1. Presentation, AI, import, or a domain pack creates declarative
   `ModelCommand` values.
2. `CoreBus` tags the source, buffers streamable command batches when needed,
   and publishes transaction, preview, undo/redo, document, and stream events.
3. `DocumentSession` applies the complete batch to a cloned document.
4. The configured `CadKernel` evaluates the staged document.
5. Direct edits install a successful evaluation immediately as one revision.
   AI edits retain the staged document, evaluated scene, created IDs, structural
   diff, command count, and base revision in an opaque `TransactionPreview`.
6. The WGPU viewport keeps committed geometry as the picking and analysis source
   while rendering preview changes through independent translucent ghost passes.
7. Approval consumes the exact preview only when its base revision is still
   active. Rejection or revision mismatch cannot replace document state.

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

`cadx-core::assembly` keeps reusable component definitions, hierarchical
occurrences, full-frame fixed/revolute/slider mates, source entity identities,
and rigid local placements independent of feature history and kernel objects.
Validation proves hierarchy, single-driver kinematics, ownership, and
feature/world-transform consistency. `cadx-app::plan_step_import` expands
each STEP body occurrence into a concrete imported feature, composes nested
placements, and creates the complete product structure in the same atomic
transaction. Direct occurrence placement and mate state commands update the
local occurrence and every materialized descendant world transform before
kernel evaluation; driven children reject conflicting direct placement.
`SetOccurrenceSuppressed`
persists a direct occurrence state whose effect is inherited by descendants.
Core rejects an active feature that depends on a suppressed body. Truck skips
those bodies before B-Rep reconstruction, so viewport, picking, analysis, mesh
exchange, and exact STEP export consume the same unsuppressed product state.
A kernel-neutral
feature-instance lookup identifies each ordered definition body. Truck uses it
to reconstruct eligible repeated imported STEP bodies once per evaluation or
export attempt, then topologically clones, transforms, tessellates, and names
each occurrence independently. It additionally emits kernel-neutral local mesh
definitions and rigid render instances for compatible repeated native or
imported bodies. WGPU uploads each definition once and draws its occurrences
from an instance buffer, while world-space `EvaluatedPart` meshes remain
available to analysis, picking, and exchange. Exact STEP export builds a
kernel-private ownership plan from the same feature-instance identity,
inverse-transforms one representative solid per active definition/body slot,
and emits AP242 product definitions plus local occurrence relationships.
Standalone visible bodies remain independent products, suppressed subtrees are
excluded, and incompatible repeated definitions fail before serialization. See
[`assemblies.md`](assemblies.md).

Product interference is a separate read-only `CadKernel` operation because
exact B-Reps cannot cross the inward-owned core port. Truck rebuilds the
document once, selects unsuppressed terminal solid products independently of
visibility, excludes pairs within one multi-body occurrence, performs AABB
broad-phase culling, and retains typed clear, interfering, or failed evidence
for every surviving pair. Exact intersection topology stays native; volume
integration declares its chord-tolerance tessellation precision. The desktop
runs the report on demand, and AI receives a fresh optional copy as read-only
context. See [`interference-analysis.md`](interference-analysis.md).

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

Before a generic provider request, `cadx-desktop` establishes the active domain,
analyzes the committed scene, and asks `cadx-ai::ContextCollector` for one typed,
revision-bound context snapshot. Retrieval is deterministic: selected features
and persistent face/edge/vertex owners, prompt name or ID matches, two upstream
and downstream dependency hops, spatial AABB distance from the most specific
resolved selection (falling back to viewport target), and then recent features.
The default/hard budgets are 32/64 detailed features and 16/32 spatial entities;
selected edge references are capped at 64 and oversized domain schemas are
omitted above 64 KiB. Every truncated category records an omitted count.

The provider adapter builds a second compact document view. It serializes full
parameters only for retrieved feature IDs, summarizes opaque domain namespaces
by entry count, redacts embedded STEP physical-file source, and retrieves at
most eight related assemblies with 32 occurrences each. Occurrence retrieval
retains selected or prompt-matched instances, hierarchy ancestors, adjacent
children, referenced definitions, and mates whose endpoints are both present.
Interference aggregates remain complete while candidate IDs and pair detail are
relevance-ranked and capped at 64 and 32. Per-part scene analysis is restricted
to the retrieved spatial set while total area, volume, mass, center of mass, and
inertia remain the committed aggregate evidence. The system prompt forbids edits
against omitted or ambiguous identifiers.

The desktop assigns every asynchronous AI request a unique identity, records
the revision used to construct it, and retains a Tokio abort handle. The user
can cancel planning directly or with Escape; a successful edit, undo/redo, new,
or open operation proactively aborts planning when its snapshot becomes stale.
Only the currently tracked identity may complete, so a response already queued
by an aborted request cannot clear or replace a newer request even when both
share a document revision. A matching response is still discarded if the
document advanced while the provider was planning. A current response may
contain one primary command batch and up to two independent alternatives. Each
batch enters `DocumentSession::preview`
separately against the same committed document; candidates never build on one
another. The complete batch and kernel evaluation run against a copy-on-write
document without changing dirty state, history, or active scene.

The resulting `DocumentDiff` reports stable added, modified, and removed
feature identities, changed assemblies, and changed domain-data namespaces.
`cadx-analysis::compare_scenes` derives body and triangle deltas, volume,
surface area, and available mass and center-of-mass changes from committed and
candidate `EvaluatedScene` values. When the active kernel advertises support,
`analyze_preview_interference` also runs exact interference analysis over the
staged document. These values are local engineering evidence and are absent
from the AI plan contract, so a provider cannot assert its own score as fact.
The renderer draws the selected candidate in cyan and removed committed
geometry in red without making ghost geometry pickable. `commit_preview`
performs an optimistic revision check and consumes only the selected validated
staged document as one undoable transaction; all other candidate previews are
explicitly discarded.
See [`ai-transaction-sandbox.md`](ai-transaction-sandbox.md).

`CadKernelCapabilities` exposes operation availability, interference analysis,
and edge-modifier
geometry restrictions without leaking a concrete backend. Its default disables
all optional operations until an adapter opts in. The desktop uses it for command
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
  source DATA-section index, outer-shell identity, oriented void-shell list,
  and length-unit interpretation. This keeps a CADX document portable,
  dimensionally deterministic, and faithful to `BREP_WITH_VOIDS` ownership
  while leaving STEP parsing and Truck geometry conversion behind their
  respective adapters. STEP presentation styles are reduced to a feature color
  only when the mapping is complete and unambiguous. Export partitions
  disjoint outer shells from contained voids before regenerating entity-level
  styles from the same generic feature metadata. AP242 product definitions,
  occurrences, canonical relationship links, and local placements are reduced
  into the kernel-neutral assembly domain on import. Export derives the inverse
  mapping without leaking Truck solids: compatible definition bodies become
  component-local exact B-Reps, occurrences retain local rigid placements, and
  standalone bodies remain independent products. See
  [`step-import.md`](step-import.md) and [`assemblies.md`](assemblies.md).
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
- A new workspace member must be classified in `cadx-arch::CRATE_LAYERS` and
  documented in [`module-map.md`](module-map.md) before it will compile in CI.
- A crate root holds module declarations, re-exports, and at most the type that
  defines the crate's entry point. Logic lives in a named submodule.
- No source file exceeds 1000 lines. A module that outgrows the budget is split
  into cohesive submodules; the budget is not raised.
