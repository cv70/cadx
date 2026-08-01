# Ruled Loft

CADX implements a bounded, parametric ruled loft through ordered sketch
sections. The initial behavior follows a narrow, predictable baseline: a loft
needs at least two profiles, every profile has the same segment count, and hole
loops are unsupported. CADX adds explicit source dependencies,
model-space validity proofs, stable face names, versioned persistence, exact
STEP export, and application-level atomic failure.

## Feature Contract

`CreateLoftFromSketches` accepts 2 to 32 unique, non-zero sketch feature ids.
Their order is geometric intent: the first id is the start section, adjacent
ids define transitions, and the last id is the end section. The desktop dialog
lists all hole-free sketches, supports explicit inclusion and ordering, and
checks segment-count and local-winding compatibility before submission. AI uses
the same ordered-id command and schema limits.

Every section must satisfy all of these conditions:

- exactly one valid closed outer loop and no holes;
- the same exact segment count as the first section;
- the same local traversal direction as the first section; and
- a solved Line, Arc, positive-weight rational-quadratic, or cubic-Bezier loop.

The feature stores the ordered ids and one solved profile cache per id. The
feature graph retains those dependencies in order and blocks deletion of a
referenced source sketch. Truck solves each current source again during staged
evaluation, so the source sketch remains the editable geometry truth.

## Model-Space Proof

Local loop agreement is not enough when sketches use different work planes.
Truck resolves every work-plane frame and applies source and loft transforms
before construction. It then forms an axis from the first section profile
centroid (the mean of its persistent segment starts) to the last and requires
every ordered centroid projection to advance by more than the modeling
tolerance. A folded or repeated sequence is rejected.

For each section, the sign of its exact loop area selects the oriented work-plane
normal. That area direction must have a non-tangent dot product with the loft
axis, and all sections must agree on its sign in model space. This rejects
flipped frames and inconsistent winding before B-Rep construction. The common
sign also determines whole-shell orientation.

These checks deliberately define a narrow, reproducible solid operation. They
do not attempt to infer a better section order or silently reverse an
individual wire.

## Exact B-Rep And Naming

Each section wire is built from its exact stored curve type. Truck's exact wire
homotopy constructs one ruled side face for every pair of adjacent, same-index
segments. Tessellated sketch or viewport samples never enter the B-Rep. Exact
Arc and rational-quadratic sections therefore remain NURBS geometry in STEP,
while cubic sections remain B-spline curves.

Construction must produce exactly one geometrically consistent closed shell
with this face count:

```text
2 + (section_count - 1) * segment_count
```

Faces receive semantic identities directly from construction order:

- `StartCap` for the first section;
- `EndCap` for the last section; and
- `LoftSide { transition, segment }` for the side generated between sections
  `transition` and `transition + 1` from the same indexed source segment.

No generic geometric fallback is allowed for loft faces. A face-count mismatch
or incomplete semantic coverage is a kernel error. Editing an intermediate
section without changing its segment topology retains the complete face-name
set. STEP export uses the same exact solid and has regression coverage for one
closed shell with rational B-spline surfaces.

## Transactions And Limits

Creating a loft or editing any linked source sketch stages the complete document
and evaluates it with the active kernel. Only a valid closed result commits.
Constraint failure, incompatible profiles, folded ordering, tangent section
planes, inconsistent model-space winding, homotopy failure, or invalid shell
construction leaves the document, evaluated scene, history, and cached loft
profiles unchanged.

The current feature is intentionally ruled and open-ended. It does not support
hole loops, guide curves, continuity controls, arbitrary segment
correspondence, smooth multi-section interpolation, periodic/closed lofts, or
branching section networks.
