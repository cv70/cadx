# Sketches and Work Planes

CADX sketches are kernel-neutral exact 2D regions. A `Primitive::Sketch` stores
a parametric `SketchPlane`, one editable outer `SketchLoop2D`, zero or more
editable hole loops, up to 128 independent construction segments, and an
ordered constraint list; it stores neither kernel objects nor tessellated
geometry. Region loops and construction use first-class
`SketchSegment2D::Line`, `SketchSegment2D::Arc`,
`SketchSegment2D::RationalQuadratic`, and `SketchSegment2D::CubicBezier` values. Keeping the outer
loop, holes, construction, and segment types explicit avoids inferring region
semantics from winding, loop order, display samples, or a visual style flag.

Each Line stores `start` and `end`. Each Arc stores `start`, `end`, `center`,
and `ccw`; both endpoints must have the same finite positive radius from the
center. Adjacent endpoints must coincide, including the final-to-first pair. A
complete circle is represented by two connected semicircular Arc segments,
because a single segment with identical endpoints would be ambiguous and is
rejected. Segment indices are persistent modeling identities.

Construction segments are independent exact curves, not a second region. They
need not close, connect, avoid intersections, or lie inside the profile, but
each must remain finite, non-degenerate, and (for an Arc) exactly circular.
They participate in constraint solving and sketch overlays while remaining
absent from extrusion/revolve wires, topology naming, STEP/STL/3MF solids, and
physical analysis.

Profile ids retain their legacy prefix. For a profile with `N` segments, its
vertices are `P0..P(N-1)` and its segments are `S0..S(N-1)`. Construction
segment `i` is `S(N+i)` and owns the independent endpoints `P(N+2i)` and
`P(N+2i+1)`. Arc centers are addressed through their segment id rather than as
point ids. Hole loops never enter this namespace, so hole edits cannot rebind a
constraint. Adding or removing a profile segment can shift appended
construction ids and is therefore validated atomically with the full
definition.

`cadx-sketch` exposes the kernel-neutral mathematical foundation used by these
trimmed curve segments. `RationalQuadraticBezier2D` represents a rational
quadratic Bezier on `[0, 1]` with unit endpoint weights and one finite positive
internal weight; `CubicBezier2D` represents a cubic Bezier on the same closed
domain. Both provide exact point and analytic first/second derivative
evaluation plus signed curvature. Non-finite inputs, parameters outside the
trimmed domain, unrepresentable arithmetic, and degenerate curvature tangents
fail with structured errors.

Internal controls use `SketchControlPointRef { segment, control }`. The control
slot is local to its persistent segment and remains outside `PointId`, so adding
curve editing later will not reinterpret endpoint identities. Resolution checks
both the segment owner and the curve-specific slot count. Adaptive display
sampling is deterministic De Casteljau subdivision: positive rational weights
preserve its control-hull bound, and callers must supply a finite positive
tolerance, maximum depth, and maximum point count. Exhausting either budget is
an error rather than a partial polyline. Samples always include the exact
endpoints and remain evaluated data, never exact or persistent geometry.

The persistent variants retain those exact values in the version 21 `.cadx`
schema. Internal controls remain segment-local identities and never change the
endpoint namespace. Desktop and AI editors expose the same controls. Positive
rational weights make each subdivided curve lie inside the convex hull of its
projected homogeneous controls; bounded recursive hull intersection detects
crossings and tangencies without treating a display polyline as exact geometry.
Exhausting its pair or depth budget rejects the edit.

## Work-Plane Contract

`SketchPlane` has five persistent forms:

- `WorldXy`, `WorldXz`, and `WorldYz` use right-handed model frames;
- `DatumPlane { datum_id }` depends on an existing `Primitive::DatumPlane`; and
- `PlanarFace { face: FaceRef }` depends directly on one solid feature and
  retains the complete persistent face name.

Attachments retain their feature or topology reference, not a copied origin or
normal. The feature graph therefore orders every source, sketch, and generated
solid and rejects missing references, wrong feature types, deletion of an
in-use source, and dependency cycles. A direct face attachment avoids creating
an implicit datum; an explicit datum remains the model for offset work planes.

Truck resolves an attached analytic plane to `origin + X + Y + normal`. X
follows the supporting plane's normalized parameter-U direction. Y is
orthogonalized so `X cross Y` equals the oriented face normal. A direct
`PlanarFace` origin is the resolved face centroid projected onto that analytic
plane; a datum origin follows the supporting plane equation plus its offset.
These frames follow rigid source transforms and inherited supporting surfaces
without choosing a new world-axis projection. Lost, ambiguous, non-planar,
degenerate, or inconsistent frames fail the complete staged transaction.

`Feature.translation` on a sketch is a local `[X, Y, normal]` offset. Its Euler
Z component is an in-plane rotation; non-zero X or Y rotation is rejected.
World XY preserves the historical interpretation of translation as model XYZ.
The transform on an extrusion or revolve remains an additional model-space
result transform and does not replace the sketch attachment.

## Solving and Solid Generation

For construction-free regions containing only Line segments and only the
legacy projection-compatible constraint set, `cadx-sketch` provides a
deterministic bounded projection solver for:

- `Coincident` point pairs;
- `Horizontal` and `Vertical` cyclic profile segments;
- `Fixed` point coordinates; and
- `Distance` point dimensions.

Constraints are validated against the complete profile/construction namespace
before solving. Non-convergence, degenerate output, and self-intersection are
errors rather than partial geometry.

When the outer loop contains a curved segment, construction is present, or any advanced
relationship is used, `cadx-sketch` switches to a deterministic, bounded
Levenberg-Marquardt solve. Its parameters are every shared profile vertex, both
independent endpoints of every construction segment, an independent center
for every Arc, and every segment-local rational/cubic control. Rational weights
remain explicit positive shape parameters. Each Arc contributes an implicit equal-radius residual between
its start and end, preserving circular geometry while the following explicit
constraints are solved together:

- the same `Coincident`, `Fixed`, and `Distance` point constraints;
- signed `HorizontalDistance` and `VerticalDistance` between two distinct
  points;
- non-negative `PointLineDistance` from a point to one Line support;
- `LineThroughCenter` between one Line support and one Arc center;
- `Horizontal` and `Vertical` on Line segments only;
- `Length` with a finite positive dimension on one Line segment;
- `EqualLength`, `Parallel`, and `Perpendicular` between two distinct Line
  segments;
- `Angle` between an ordered pair of distinct Line segments;
- `PointOnCurve` from a point to one finite exact segment;
- `Midpoint` from a point to a Line whose endpoints are different point
  entities;
- `Symmetric` between two distinct points about one Line axis;
- `Radius` and `FixedCenter` on one Arc segment;
- `EqualRadius` and `Concentric` between two Arc segments; and
- `Tangent` between adjacent segments when at least one is curved; and
- `CurvatureContinuous` between two adjacent curved segments.

`Angle` is the signed directed angle from the first segment's start-to-end
direction to the second segment's start-to-end direction. It is stored in
degrees in the inclusive range `[-180, 180]`; positive values rotate
counterclockwise in sketch coordinates. Reversing the ordered pair therefore
changes the constraint semantics. Advanced Line relationships cannot reference
a curved segment, and a two-segment relationship cannot reference the same segment twice.
`PointOnCurve` uses the closest point on the bounded segment, not an infinite
support line or circle. Midpoint rejects either endpoint of its own target Line,
and a Symmetric axis must be a Line. Horizontal and vertical point dimensions
store `second - first`, so negative values are meaningful and reversing the
point order changes the equation. Point-line distance uses the infinite Line
support rather than the finite segment and retains the point's initial side;
line-through-center likewise uses the infinite Line support.

Tangency uses the analytic unit tangent at the shared persistent vertex for
Arc, rational-quadratic, and cubic segments. Clockwise and counterclockwise Arc
traversal are both supported. The
curvature-continuity constraint is G2: it drives the signed angle between unit
tangents in traversal direction to zero, which rejects an anti-parallel cusp,
and equates signed
curvature (analytic derivatives for Beziers and `+1/r` counterclockwise,
`-1/r` clockwise for Arcs). Its curvature residual is nondimensionalized by
the larger segment length. Only adjacent outer-loop curved pairs are eligible;
construction pairs and Line/Line pairs fail validation before iteration.

The
nonlinear solver uses a finite-difference Jacobian, damped normal equations,
bounded iteration count, and deterministic pivoting. It never commits its best
partial iterate: the residual must meet tolerance, then the rebuilt exact
`SketchRegion2D` and every construction segment must pass radius, closure,
intersection, area, hole-containment, finite-curve membership, and
dependent-feature validation. Wrong entity kinds, non-adjacent tangency,
non-finite dimensions, conflicting systems, and non-convergence therefore fail
closed.

Every converged solve produces a read-only `SketchSolveDiagnostic`. CADX forms
the numerical Jacobian at the committed solution and incrementally computes its
row rank, counting each Arc's implicit equal-radius equation before ordered user
constraints. Remaining DOF is `parameter_count - rank`. A user constraint is
reported as redundant when at least one of its equations adds no rank after all
earlier constraints; redundancy does not make a consistent sketch invalid.
When solving fails, non-zero per-constraint residual ranges identify ordered
conflict candidates. Core retains the stable reason, zero-based indices,
iteration count, and residual while rolling the complete staged edit back.
Desktop presents one-based row numbers, while AI receives the machine-readable
zero-based report.

Every loop must be finite, closed, simple, non-degenerate, and contain between
2 and 128 segments. A sketch supports at most 32 holes and 1024 segments across
all loops. Relationships among Line, Arc, rational-quadratic, and cubic segments
are checked through analytic primitives or bounded positive-weight control-hull
subdivision. Every hole must lie strictly inside the outer loop. Holes
cannot touch or intersect the outer loop or another hole, and holes cannot
overlap or nest. Both loop windings are accepted; the Truck adapter normalizes
inner-wire orientation privately without changing document order or persistent
segment ids.

`CreateSketchRegion` creates exact region and optional construction geometry.
`SetSketchDefinition` atomically replaces region, construction, and constraints;
`SetSketchRegion` remains a region-only compatibility edit that preserves
construction.
Legacy `CreateSketch`, `ResizeSketch`, and `SetSketchHoles` remain compatible
Line-only conveniences and convert point arrays into Line loops. All sketch
commands use the ordinary `ModelCommand` transaction boundary. Before an edit
commits, CADX solves the proposed region and validates every linked extrusion,
revolve, and ruled loft against its proposed cache. A curve that would cross a
dependent revolve axis, an unsupported hole, a loft segment-count or winding
mismatch, or an invalid constraint leaves the sketch and every dependent cache
unchanged.

Linked `ExtrusionFromSketch` features keep cached solved exact outer and hole loops
for persistence and inspection, while Truck always solves and reads the
authoritative source again during evaluation. Extrusion constructs one exact
B-Rep from an outer wire plus inner wires. Lines become analytic Truck lines;
Arcs and rational quadratics become exact NURBS curves; cubics become exact
B-spline curves. Holes are not approximated with a
boolean subtraction. Topology naming must recover both caps and at least one
side face for every persistent outer and hole segment; evaluation fails closed
if this semantic coverage is incomplete. Construction is solved with the source
but never passed to either wire builder. Revolve uses the same exact wire
construction and rejects any profile that touches or crosses its axis,
including a curved segment whose conservative support range reaches the axis. Sketch holes are not yet
supported by revolve.

Ruled loft consumes 2 to 32 sketches as explicitly ordered feature-graph
dependencies. Every source must have one outer loop, no holes, the same exact
segment count, and the same local traversal direction. Truck resolves the
current solved curves and work-plane frames again, then rejects folded centroid
order, tangent section planes, inconsistent model-space winding, or a result
that is not one geometrically consistent closed shell. Same-index exact curves
form each transition, and the resulting faces keep stable cap and
transition/segment identities. See [`loft.md`](loft.md).

A visible sketch produces a kernel-neutral `EvaluatedSketch` containing
bounded-angle world-space display polylines, the resolved frame, and ordered
constraint-annotation geometry computed from the committed exact solve. This is
evaluated presentation data, not persistent geometry. The annotation snapshot
uses sketch-local witness points, axes, rays, and Arc centers; it contains no
egui state, pixel coordinates, or backend object ids. Invalid entity references
therefore fail during evaluation instead of producing plausible labels from a
sampled polyline.

The viewport draws every closed outline, each open construction polyline, and
the local X/Y marker. Arc extrema and midpoints participate in feature picking,
selection highlighting, occlusion, frame-all, and reference-extent inference.
Construction uses a distinct overlay color without becoming topology. A hidden
sketch still resolves and drives dependent solids but emits no overlay.

## Constraint Annotations

Constraint annotations appear only for the selected visible sketch so they do
not obscure unrelated solids. `cadx-render` projects the exact local witnesses
through the active camera, then creates fixed-pixel glyphs and drafting-style
dimension lines with deterministic collision avoidance. Zooming changes model
scale without changing glyph, arrow, extension-line, or label size. Normal
geometric constraints and driving dimensions use distinct colors; ordered
redundant constraints use warning color and the most recent attributed failed
edit uses failure color. The indices remain zero-based internally and match the
solver/AI contract.

The directly editable driving dimensions are:

- `Distance`;
- signed `HorizontalDistance` and `VerticalDistance`;
- non-negative `PointLineDistance`;
- positive Line `Length`;
- directed `Angle` in `[-180, 180]`; and
- positive Arc `Radius`.

Double-clicking one of these value labels opens a compact dimension editor.
Confirming clones the complete current sketch definition, replaces only the
indexed value after checking its variant and domain, and submits one
`SetSketchDefinition` command. Core and Truck then solve every relationship and
rebuild all dependents against a staged document. Success creates one undoable
revision. Conflict, non-convergence, or downstream B-Rep failure leaves the
document unchanged, retains the editor value, and colors any attributed rows
and annotations from the structured failure. `Fixed` and `FixedCenter` remain
placement-lock glyphs because their independent X/Y coordinates are not one
driving dimension.

The AI context exposes the selected sketch's committed editable dimensions as
`constraint_index`, `kind`, and `value`, alongside rank, DOF, redundancy, and
the last failed edit. This is read-only evidence; an AI edit still proposes an
ordinary reviewed command batch and cannot mutate an annotation or renderer.

Segment points map to `origin + x * X + y * Y`. Extrusions sweep along
`normal * height`. Revolve axis origins and directions are also expressed in
the same 2D frame. Hidden solids and datum planes still participate in this
resolution chain. STEP uses the same resolved frame and exact B-Rep build as
viewport evaluation; STL and 3MF consume the resulting evaluated solids.

## Persistence

Work-plane attachments were introduced in `.cadx` schema version 12. A missing
`plane` field defaults to world XY during deserialization. Files written before
version 12 are migrated to preserve the older kernel's distinct conventions:
legacy extrusion drivers become world-XY sketches, legacy revolve drivers
become world-XZ sketches, and a sketch shared by both operations is split into
two equivalent drivers. Their legacy sketch transforms are cleared because
they did not previously participate in solid evaluation. Version 12 documents
do not run this migration. Direct persistent planar-face attachment was added
in schema version 13 and round-trips the complete `FaceRef`. Explicit sketch
and linked-extrusion hole loops were added in schema version 14. Exact typed
Line/Arc segments were added in version 15. Version 15 retains the JSON field
names `profile` and `holes`, but each loop is now a typed segment array. The
loop deserializer accepts version 14 point arrays and deterministically upgrades
each cyclic point pair to a Line segment. No polygon sampling is introduced by
migration. Version 16 adds the persistent `radius`, `fixed_center`,
`equal_radius`, `concentric`, and `tangent` constraint variants. It does not
change the version 15 loop representation and requires no geometry migration.
Version 17 adds persistent `length`, `equal_length`, `parallel`,
`perpendicular`, and directed `angle` constraint variants. It likewise leaves
the version 15 typed-loop representation unchanged and requires no geometry
migration.
Version 18 adds the optional persistent `construction` segment array plus
`point_on_curve`, `midpoint`, and `symmetric` constraint variants. It retains
the version 15 typed-loop representation, defaults missing construction to an
empty list, and requires no migration of older geometry.
Version 19 adds signed `horizontal_distance` and `vertical_distance`,
non-negative `point_line_distance`, and `line_through_center` constraint
variants. Solve diagnostics remain evaluated state rather than persistent
geometry, so version 18 documents require no migration.
Constraint glyphs, screen-space layout, editable-dimension dialogs, and solved
annotation anchors are likewise evaluated or transient state. They reuse the
existing constraint values and do not change the version 19 document schema.
Version 20 adds the persistent `curvature_continuous` relationship. Older
documents require no geometry migration because the new constraint is opt-in;
screen-space G2 glyph placement remains evaluated state.
Version 21 adds persistent rational-quadratic and cubic-Bezier segment variants.
Older typed Line/Arc arrays remain valid without migration; unknown future
variants and non-positive rational weights fail during parse or validation.
The desktop inspector exposes only entity-compatible segment choices, and the
AI command schema carries the same reference rules.
Version 22 adds ordered ruled-loft sketch ids and their solved exact profile
caches. Older documents require no migration because the new feature is opt-in;
decoded lofts must pass the complete dependency and profile compatibility
contract before replacing active state.

Constraint, hole, and construction fields remain optional for older files and
default to empty lists. Documents are validated after migration, including the
complete feature graph and strict loop relationships, before they can replace
active state.
