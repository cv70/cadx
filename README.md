# CADX

CADX is an AI-native parametric 3D CAD desktop application written in Rust 2024. It uses egui for the workbench, wgpu for the 3D viewport, Truck as the first CAD kernel, and rust-genai for provider-neutral model access.

The current foundation supports boxes, cylinders, spheres, cones, toruses, polygon extrusions, exact Line/Arc/rational-quadratic/cubic sketch regions with explicit outer and hole loops on world, parametric datum, or directly referenced planar-face work planes, exact holed sketch extrusions, single-loop revolves, and ordered multi-section ruled lofts, embedded STEP B-Rep import with explicit editable AP242 repeated and nested assembly occurrences, configuration-aware AP242 product-structure export with reusable component-local exact B-Reps, persisted full-frame fixed/revolute/slider assembly mates with deterministic nested forward kinematics, inherited occurrence-subtree suppression, definition-scoped imported-B-Rep reconstruction reuse, WGPU indexed mesh instancing for compatible repeated definition bodies, and deterministic product interference analysis with exact B-Rep intersection topology and declared volume precision, parametric union/subtract/intersect booleans with bounded tolerance policy, pre/post validation, topology normalization, planar and curved analytic identity classification, bounded planar interface sewing with exact curved boundaries, local B-spline/NURBS homotopy boundary refitting including degree-elevated affine multi-row surfaces, and rigid B-Rep gap alignment, reproducible regression diagnostics, and atomic multi-edge planar chamfers and constant-radius fillets; live parameter editing; feature duplication, color, and persistent material editing; translation and rotation transforms; per-part and assembly mass, center-of-mass, and inertia analysis; deterministic dependency-graph validation and impact analysis; persistent face, edge, and vertex references with boolean, loft, and edge-modifier lineage; visible, pickable sketch, face-dependent datum-plane, and vertex-dependent datum-point overlays; topology-level viewport picking; visibility and deletion; undo/redo; versioned `.cadx` persistence; orbit/pan/zoom and frame-all navigation; and reviewable, kernel-validated atomic AI-generated edit plans.

## Workspace

```text
crates/
  cadx-core/          Domain model, kernel ports, versioned document codec
  cadx-analysis/      Kernel-neutral geometric and mass-property analysis
  cadx-sketch/        Kernel-neutral sketch constraints and deterministic solver
  cadx-app/           UI-neutral use cases, transactions, history, document session
  cadx-config/        Typed ~/.cadx configuration and preference store
  cadx-io/            Atomic document IO and validated STEP/STL/3MF adapters
  cadx-kernel-truck/  Truck B-Rep construction and tessellation adapter
  cadx-ai/            AiAssistant trait and rust-genai tool-calling adapter
  cadx-i18n/          Runtime-switchable English and Simplified Chinese resources
  cadx-render/        egui-wgpu custom 3D renderer and orbit camera
  cadx-desktop/       Native egui presentation adapter and composition root
```

The dependency flow is intentionally one-way. `cadx-core` has no filesystem, UI, GPU, AI provider, or concrete CAD-kernel dependency. `cadx-app` coordinates domain commands through the kernel ports without depending on desktop or storage adapters. Truck-native shapes remain private to `cadx-kernel-truck`; other crates receive only kernel-neutral values.

AI responses cannot mutate geometry directly. `cadx-ai` exposes a JSON-schema tool that returns `ModelCommand` values and receives read-only scene, interference, and kernel capability context when available. `cadx-app::DocumentSession` validates the complete batch against a staged document and commits it only after the CAD kernel accepts the resulting geometry. See [`docs/kernel-capabilities.md`](docs/kernel-capabilities.md) for the conservative capability contract used by desktop and AI adapters.

Solid features can persist a named material and density in kg/m^3. The desktop inspector offers common engineering presets while keeping both fields editable. `cadx-analysis` uses these assignments to compute mass, center of mass, and centroidal inertia tensors in kg mm^2. An explicit analysis density remains available as a uniform override; otherwise aggregate mass properties are omitted when any visible part is unassigned. The complete contract and numerical method are documented in [`docs/materials-and-mass-properties.md`](docs/materials-and-mass-properties.md).

AI plans are never applied on receipt. The workbench presents the validated command list for explicit approval, and an approved plan enters history as one undoable transaction.

Sketches retain one editable exact outer loop, explicit non-overlapping hole
loops, up to 128 independent non-solid construction exact-curve segments, and
first-class rational-quadratic and cubic curves with segment-local control
identities. Ordered sketch constraints cover
Coincident, Horizontal, Vertical, Fixed, Distance, Radius, FixedCenter,
EqualRadius, Concentric, Length, EqualLength, Parallel, Perpendicular, directed
Angle, signed HorizontalDistance/VerticalDistance, PointLineDistance,
LineThroughCenter, PointOnCurve, Midpoint, Symmetric, and adjacent exact-curve
Tangent relationships, plus signed-curvature G2 continuity between adjacent
curved segments. A construction-free Line region whose constraints are
limited to Coincident, Horizontal, Vertical, Fixed, and Distance uses
deterministic projection. Construction geometry, an advanced relationship, or
an Arc uses a bounded damped nonlinear solve over shared and independent
vertices plus exact arc centers. PointOnCurve targets the finite curve, and
construction never enters B-Rep, export, or physical analysis. Entity
mismatches, conflicts, non-convergence, and invalid solved geometry fail closed.
Each successful evaluation reports numerical Jacobian rank, remaining DOF, and
ordered redundant constraints. Rejected edits retain structured conflict reason,
constraint indices, iterations, and residual for the desktop and AI context.
Selecting a visible sketch overlays fixed-pixel geometric-constraint glyphs and
drafting-style driving dimensions derived from the committed solved geometry.
Distance, signed horizontal/vertical distance, point-line distance, Length,
Angle, and Radius labels are directly editable; each edit reuses the complete
kernel-validated atomic sketch transaction. AI receives the selected sketch's
same zero-based editable-dimension inventory as structured read-only context.
A complete circle is two exact semicircular Arc segments, not a polygon
approximation. Holed
extrusions are constructed as one exact inner-wire B-Rep with rational NURBS
arcs and persistent outer and hole-side names; revolve and ruled loft use the
same exact curves and explicitly reject holes. These operations retain
source-feature dependencies and rebuild atomically from the latest solved
region, rejecting deletion of a still-referenced sketch. A ruled loft consumes
2 to 32 explicitly ordered, compatible sketches and preserves cap and
transition/segment face identities; model-space section order and orientation
must pass the bounded proof documented in [`docs/loft.md`](docs/loft.md).
Revolve axes are explicit 2D
origin/direction vectors in the resolved sketch frame and support partial or
full turns. A sketch can depend on world XY/XZ/YZ, an existing DatumPlane, or a
complete persistent planar `FaceRef` on a solid. A direct face attachment uses
the resolved face-centroid projection as its local origin without creating
implicit reference geometry. Datum offset and source transforms rebuild the
downstream exact B-Rep while frame-aware cap and side names remain persistent.
Visible solved outer, hole, and construction curves render as pickable,
highlightable overlays and remain separate from B-Rep export and physical
analysis.

Documents use a versioned envelope rather than serializing UI or kernel state. Imported STEP features embed validated source data, an outer-shell identity, oriented void-shell references, and source units instead of a filesystem path or Truck object id, so they remain portable and rebuildable. Assemblies persist reusable component definitions, hierarchical occurrences, direct suppression state, source entity identities, right-handed rigid placements, and constrained mate state. Loading rejects unknown formats, future schema versions, duplicate feature, assembly, or mate IDs, dependency or occurrence cycles, invalid mate drivers, axes, frames, limits, or state, active dependencies on suppressed bodies, inconsistent ownership and transforms, non-finite values, invalid dimensions, invalid colors, and invalid material metadata before replacing the active document. Every command is evaluated against a staged document and the active document is replaced only after the CAD kernel accepts the resulting graph and geometry. See [`docs/assemblies.md`](docs/assemblies.md).

## Run

Rust 1.97.1 or newer is required.

```bash
cargo run -p cadx-desktop --bin cadx
```

The native workbench supports `Cmd/Ctrl+N`, `Cmd/Ctrl+O`, `Cmd/Ctrl+S`, `Cmd/Ctrl+Z`, `Cmd/Ctrl+Shift+Z`, `Cmd/Ctrl+D`, `Delete`, and `Escape`. The View toolbar switches among face, edge, and vertex selection. Clicking visible topology returns a persistent reference; Shift-click toggles additional edges on the same body, and all selected topology is highlighted. A selected planar face changes the Sketch tool into a direct face-attached command and can also create an offset datum plane; a selected DatumPlane creates a datum-attached sketch. The sketch inspector can switch among world planes, existing datums, and its current persistent face. A selected vertex can create a model-space-offset datum point, and supported edge sets can create one real parametric chamfer or fillet from the Design toolbar. The Design toolbar also runs an on-demand product interference report whose result rows select the first reported feature. Datum and solved-sketch overlays participate in visibility, feature picking, selection highlighting, and frame-all without becoming fake solids. The Measure tool reports edge length with precision provenance, vertex-to-vertex distance, linear-edge angle, planar-face angle, and parallel support-plane spacing. Double-clicking the viewport frames all visible geometry. See [`docs/reference-geometry.md`](docs/reference-geometry.md), [`docs/sketch.md`](docs/sketch.md), [`docs/loft.md`](docs/loft.md), [`docs/measurement.md`](docs/measurement.md), [`docs/interference-analysis.md`](docs/interference-analysis.md), [`docs/edge-chamfer.md`](docs/edge-chamfer.md), and [`docs/edge-fillet.md`](docs/edge-fillet.md) for supported geometry and failure semantics.

Boolean and edge-modifier failures remain typed from Truck through the session, desktop, and AI context. Failed edits do not commit; their dialogs stay open with localized stage/reason details, structured evidence, and collapsible backend detail. Disjoint intersection reports a typed `disjoint_operands` failure, while disjoint subtraction and union resolve without invoking Truck shape operations. Chamfer and fillet failures preserve the parameter, source, complete edge set, and offending edge indices when known. See [`docs/boolean-diagnostics.md`](docs/boolean-diagnostics.md) and [`docs/edge-modifier-diagnostics.md`](docs/edge-modifier-diagnostics.md) for the complete contracts.

The file-input button imports named STEP solids from every DATA section. Standalone bodies remain editable document features, while AP242 product structure expands repeated and nested uses into distinct assembly-owned features with composed placements and shared component-definition identity. `BREP_WITH_VOIDS` retains its oriented cavity shells as one solid instead of creating false standalone bodies. Declared SI and conversion-based length units are converted into CADX's millimeter model space on the exact B-Rep; conflicting unit assignments fail closed and legacy unitless imports are visibly marked as assumed millimeters. AP214 entity colors and complete uniform boundary/face colors become feature RGBA, while mixed or partial styles produce an explicit warning instead of a false body color. STEP export writes the same feature colors and transparency back as entity-level presentation styles, splits disjoint outer components into independent solids, and retains proven contained cavities as voids. Documents with active assemblies export AP242 product definitions, nested usages, full rigid local placements, one component-local B-Rep per compatible definition body, and independent products for standalone visible solids. Effectively suppressed subtrees are absent, and occurrence-specific geometry, visibility, color, or child structure fails export instead of corrupting reusable definition identity. See [`docs/step-import.md`](docs/step-import.md) and [`docs/assemblies.md`](docs/assemblies.md). The download menu exports all currently visible, unsuppressed solids as exact STEP B-Rep, tessellated binary STL, or colored multi-body 3MF. STEP and 3MF output are parsed in tests; STL/3MF mesh output validates finite vertices, index bounds, and degenerate faces. Every format is written through a synchronized sibling temporary file before atomically replacing the destination. `Cmd/Ctrl+Shift+E` remains the direct STL shortcut.

AI and desktop preferences are loaded from `~/.cadx/config.yaml` and `~/.cadx/preferences.yaml`:

```yaml
version: 1
provider:
  endpoint: "https://api.openai.com/v1"
  model: "gpt-4.1-mini"
  api_key: "..."
  timeout_seconds: 45
```

For an OpenAI-compatible gateway, set `provider.endpoint` and optionally `provider.adapter: openai`. The API key is held in memory only for the configured client and is redacted from configuration diagnostics.

CADX settings are not read from provider or preference environment variables. `ConfigStore` discovers the user's home only to locate the fixed `~/.cadx` root; tests and alternate hosts inject an explicit root directory.

## Language

The active language is read from `~/.cadx/preferences.yaml` and can be switched immediately from the language menu in the title bar:

```yaml
version: 1
language: zh-CN
```

The native UI loads an installed CJK font on macOS, Windows, and common Linux distributions. Set `cjk_font` in `~/.cadx/preferences.yaml` to a `.ttf`, `.otf`, or `.ttc` file when using a custom or minimal system image.

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Industrial roadmap

The next modeling milestones are deliberately ordered around stable product contracts:

1. Extend bounded boolean healing beyond planar interfaces to curved contact surfaces and locally provable tensor-product surface families beyond affine ruled homotopies, and grow the imported freeform B-Rep regression corpus.
2. Extend persisted assembly mates beyond tree-structured single-axis motion to broader joints and closed-loop constraint solving.
3. Persistent per-face styles, PMI, BOM, and manufacturing metadata workflows across AP242 import and export.
4. Sandboxed AI tools for analysis, design alternatives, DFM, and simulation.

The kernel boundary, versioned document format, atomic command transactions, and AI approval gate in this repository are the foundation for those additions.
