# Reference Geometry

CADX stores reference geometry as parametric document features, not as
synthetic solids. This preserves dependency, history, visibility, color, and
AI command semantics without contaminating B-Rep export or physical analysis.

## Document Contract

Two persistent datum features are available:

- `DatumPlane { face: FaceRef, offset }` depends on one planar face. The scalar
  offset is measured in millimeters along the oriented source-face normal.
- `DatumPoint { vertex: VertexRef, offset }` depends on one topological vertex.
  The three-axis offset is measured in model coordinates in millimeters.

Both sources must be solid features. Sketches and other datum features cannot
serve as indirect source topology. The dependency graph blocks deletion of a
referenced source. Datum features cannot be boolean or edge-modifier operands
and cannot carry physical material.

Sketches may consume a datum plane through
`SketchPlane::DatumPlane { datum_id }`. This is a feature dependency rather
than copied geometry, so changing the datum offset or its source face rebuilds
the sketch-driven solids. A datum cannot attach to another datum, but any
number of sketches may depend on one datum plane.

Sketches may instead retain a complete planar `FaceRef` through
`SketchPlane::PlanarFace { face }`. This creates a direct dependency on the
solid without inserting hidden reference geometry. The selected face becomes
the work plane at its resolved centroid; explicit `DatumPlane` remains the
mechanism for an offset plane.

`CreateDatumPlane`, `SetDatumPlaneOffset`, `CreateDatumPoint`, and
`SetDatumPointOffset` are ordinary `ModelCommand` values. Direct UI edits and
AI proposals therefore use the same staged validation, approval, undo/redo,
and persistence path. Datum points were introduced in `.cadx` schema version
11; sketch work-plane attachments were introduced in version 12; direct
planar-face sketch attachment was introduced in version 13. Older versions
remain readable through explicit migration.

## Evaluation Contract

Truck evaluates source solids in feature-graph order even when a source is
hidden. It then resolves each complete persistent reference against the
rebuilt source topology:

- lost and ambiguous references reject the staged transaction;
- a datum-plane or direct sketch source must expose an analytic plane and a
  stable oriented face normal;
- its normalized parameter-U direction and an orthogonal Y direction form the
  local sketch frame, with `X cross Y` equal to the oriented normal;
- a direct sketch origin is the resolved face centroid projected onto the
  analytic plane;
- a datum point uses the exact evaluated B-Rep vertex position;
- no nearest-coordinate, geometric similarity, or mesh fallback is allowed.

Successful evaluation produces `EvaluatedDatumPlane`, `EvaluatedDatumPoint`,
and visible solved `EvaluatedSketch` values beside `EvaluatedPart` values in
`EvaluatedScene`. Resolved positions and frames are derived state and are never
persisted in the document. Hidden datum and sketch features are validated but
omitted from their visible scene collections; hidden solid sources still
evaluate when an attachment depends on them.

## Viewport and Export

The viewport renders datum planes as a bounded square aligned to their local
X/Y frame with a normal indicator, and datum points as a three-axis marker. The
display extent is a view concern inferred from the visible scene; it is not
part of the plane's mathematical definition. Visible sketches render as a
closed solved profile with a local X/Y marker. Datum and sketch overlays
participate in selection highlighting, feature picking, occlusion, and
frame-all bounds.

STEP, STL, and 3MF export consume only `EvaluatedScene::parts` or exact solid
B-Rep state. Datum geometry therefore contributes no shell, triangle, volume,
mass, center of mass, or inertia. This separation is intentional and is tested
at the scene and kernel boundaries.

## Failure Boundary

Face-backed attachments do not use nearest geometry, mesh normals, or a
reconstructed world-axis frame. If the persistent source face is lost, becomes
ambiguous or non-planar, or no longer exposes a consistent analytic frame,
every dependent datum, sketch, and solid is rejected as part of the staged
transaction.
