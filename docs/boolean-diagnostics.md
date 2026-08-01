# Boolean Diagnostics

CADX treats a failed boolean as modeling data, not as an unstructured backend
message. The Truck adapter emits a kernel-neutral `BooleanDiagnostic`, and the
application preserves that value without committing the failed transaction.
The desktop can therefore explain the failure and AI can inspect the same
read-only evidence without parsing display text.

## Transaction Contract

A union, subtraction, or intersection is evaluated against a staged document.
The document, history, revision, and evaluated scene change together only when
the complete transaction succeeds. On failure:

- the active document and scene remain unchanged;
- the desktop status and boolean dialog show the structured diagnostic;
- the dialog remains open so the operation or operands can be corrected; and
- the most recent boolean diagnostic is included in subsequent AI context as
  read-only evidence.

A later successful transaction clears the retained failure. Creating or opening
a document also clears it. Diagnostics are transient application state and are
not serialized into `.cadx` documents.

## Diagnostic Schema

`BooleanDiagnostic` identifies the boolean feature, operation, and two operand
feature IDs. It also carries:

- `stage`: the pipeline phase that observed the failure;
- `reason`: a stable machine-readable classification;
- `tolerance_mm`: the tolerance used by the last actual attempt;
- `attempts`: ordered tolerance, stage, reason, operand-healing, and
  result-healing evidence for every failed attempt;
- `left_bounds` and `right_bounds`: finite model-space AABBs when available;
- `detail`: backend text intended for engineering inspection only.

Consumers must branch on `stage` and `reason`. The `detail` field is not a
protocol and may change with the kernel implementation or dependency version.

## Stages And Reasons

| Stage | Reasons | Meaning |
| --- | --- | --- |
| `operand_resolution` | `missing_operand` | A referenced upstream solid was not available during graph evaluation. |
| `operand_validation` | `invalid_operand_topology`, `invalid_operand_geometry`, `kernel_panic` | An input is not a finite closed manifold or failed the backend consistency check before shape operations. |
| `broad_phase` | `disjoint_operands` | Intersection operand AABBs are farther apart than the active tolerance. |
| `kernel_operation` | `kernel_rejected`, `kernel_panic` | Truck returned no result or panicked while executing the shape operation. |
| `result_validation` | `empty_result`, `invalid_result_topology`, `result_evaluation_failed`, `kernel_panic` | The returned solid was empty, structurally invalid, geometrically unusable, or failed during downstream evaluation. |
| `topology_healing` | `healing_failed` | Bounded topology normalization, planar contact sewing, local surface refitting, or rigid B-Rep alignment could not reconstruct a valid solid. |
| `topology_naming` | `topology_naming_failed`, `kernel_panic` | Persistent face lineage could not be assigned to the boolean result. |

Truck shape operations expose only `Option<Solid>`. CADX therefore reports a
kernel rejection when Truck returns `None`; it does not invent unsupported
surface-intersection, classification, or topology-healing causes.

## Tolerance Policy

`cadx-core::tolerance::BooleanTolerancePolicy` is a kernel-neutral runtime
contract, not document geometry and not a `.cadx` persistence field. It
contains an absolute tolerance, a relative model-scale term, a hard maximum,
a retry multiplier, a maximum attempt count, and a healing mode. Every field
is finite and validated before the policy is installed.

For the largest span `L` of the combined operand AABB, the first tolerance is:

```text
min(maximum_mm, max(absolute_mm, L * relative))
```

Later attempts multiply by `retry_multiplier`, are deduplicated, and never
exceed `maximum_mm` or `max_attempts`. The default Truck policy resolves from
`0.05 mm` and permits at most the deterministic sequence `0.05, 0.1, 0.2 mm`.
`TruckKernel::new(t)` remains a compatibility constructor: it installs one
fixed attempt at `t` and disables healing. A custom policy is installed with
`with_boolean_tolerance_policy`, which rejects invalid configuration before
evaluation.

The nominal first tolerance owns AABB broad-phase semantics. A later, larger
tolerance is never used to silently reclassify a known-disjoint intersection.
Kernel panics stop the sequence immediately; retrying the same unsafe backend
path at a looser tolerance would not be useful evidence. A contact
classification that proves an empty solid also stops immediately because a
larger tolerance cannot turn that mathematical result into a body.

## Disjoint Operands

CADX computes finite operand bounds before calling Truck. When the AABBs are
separated by more than the modeling tolerance, the mathematically correct
result is resolved without invoking a shape operation:

- intersection returns `broad_phase / disjoint_operands` because the requested
  result contains no solid body;
- subtraction returns the left solid unchanged; and
- union creates one valid multi-shell solid from cloned operand boundaries.

The AABB separation is reported per axis in millimeters. A zero component means
the projected intervals overlap or touch on that axis.

## Validation Boundary

Before a shape operation, both operands must pass `Solid::try_new`, have finite
vertex bounds, and pass Truck's geometric consistency check when their curve
types support it. Boolean results repeat the same closed-manifold, finite
geometry, and supported consistency checks before persistent topology naming.
This pre/post contract is adapter-neutral through the stable diagnostic stages
and reasons, while the actual B-Rep checks remain inside Truck.

Truck leaves its consistency check unimplemented for intersection curves, and
its `IncludeCurve` path recurses indefinitely for a surface formed by revolving
a `Line`. CADX capability-gates that backend check for both representations so
a cone cannot abort the process with a stack overflow. Those solids are still
validated by closed-manifold reconstruction, finite bounds, persistent topology
naming, finite face tessellation, edge sampling, adjacency construction, and
vertex extraction.

Before entering Truck shape operations, the default healing policy runs a
strict single-shell non-crossing classifier. Identity uses matching compressed
topology plus representation-aware geometry: vertices and Line/Plane geometry
use modeling distance tolerance, B-spline and NURBS definitions must match
exactly, and revolved surfaces must have the same entity curve, origin, axis,
transform, and orientation. An unordered all-planar fallback preserves the
earlier tolerance-aware identity behavior for equivalent polyhedra.

The second proof recognizes exactly one complete opposing planar interface.
Its loops require one-to-one vertices and geometrically matching edges,
opposite normals, support-plane separation, and a uniform normal offset within
the active tolerance. A zero-offset interface may be bounded by exact analytic
curves and retain curved side faces, so stacked cylinders sew through their
shared circular cap. For a nonzero offset, planar side faces retain the local
Line repair. Clamped B-spline and NURBS homotopy sidewalls may instead refit one
uniquely matched parameter boundary when every cross-parameter control sequence
is the affine interpolation of its endpoints at the normalized Greville
abscissae. This includes degree-elevated, multi-row ruled surfaces. The same
B-spline coefficients distribute displacement from the contact boundary to
zero at the remote boundary, so the remote row and cap remain fixed. Each NURBS
cross sequence must additionally have one finite, nonzero weight; its points
use weight-preserving homogeneous translation. If neither local proof applies,
a final bounded path may translate the complete right-hand B-Rep by the one
proven normal offset. Every interface
Line, B-spline, or NURBS definition must coincide at numerical precision after
the same translation, and Truck intersection curves are rejected.
Preclassification is necessary because Truck can return a structurally valid
but semantically wrong two-shell union for a small gap.

For identical solids, union and intersection retain the left solid while
subtraction reports an empty result. For one complete planar interface,
intersection reports an empty result and subtraction retains the left solid.
Union removes the two interface faces, merges their paired vertices and edges,
preserves exact curve geometry when no vertices move, rebuilds affected Line
geometry for a local planar gap, locally refits proven homotopy sidewalls, or
rigidly transforms the complete right B-Rep, and extracts one closed shell.
Refitted supporting surfaces carry explicit source-to-result replacements;
rigidly aligned surfaces carry their shared translation. Persistent topology
validation checks the healed B-Rep first, then topology naming consumes that
provenance.

After an ordinary result-validation failure, or before a later tolerance
attempt, the policy also permits topology normalization. The Truck adapter
round-trips the B-Rep through its compressed topology representation, which
reconstructs and revalidates every vertex, edge, face, shell, and solid
relationship without moving points or refitting curves and surfaces. The
normalized result must pass the full postcondition again. Attempt evidence uses
`result_healing: applied` for successful planar contact recovery, local surface
refitting, rigid B-Rep alignment, or result normalization.

The resolver deliberately rejects partial face overlap, edge-only or
vertex-only contact, multiple matching interfaces, a curved contact surface,
nonuniform offsets, translated freeform boundaries whose complete definitions
do not match, intersection-curve alignment, and multi-shell operands. Sewing
across curved contact surfaces, non-clamped or non-affine cross-parameter
surface refitting, varying NURBS weights along a cross sequence, general
freeform deformation, small-edge removal, and arbitrary vertex welding remain
unsupported. Truck's stronger
closed-edge/closed-face healer cannot be applied to its polymorphic modeling
`Curve` type. CADX never accepts a merely plausible repaired solid; stronger
backend healing can replace these bounded implementations behind the same
policy and diagnostic contract.

Shape operations, result validation, topology naming, and the enclosing boolean
feature evaluation are panic-contained. A backend panic becomes a
`kernel_panic` diagnostic rather than unwinding through the session or desktop.

## Reproducible Regression Corpus

[`boolean-regression-corpus.json`](../crates/cadx-kernel-truck/tests/fixtures/boolean-regression-corpus.json)
stores versioned primitive inputs, the complete tolerance policy, and a tagged
expected success or typed failure. Its integration test rebuilds every case
twice and compares the complete result before checking either the result face
count or the diagnostic stage, reason, tolerance sequence, healing sequence,
bounds, and technical detail.

The first corpus covers typed disjoint intersection plus successful coincident
and full-face-contact unions. New backend regressions should be minimized into
this data format before a fix is attempted. A changed classification or result
must be reviewed by updating the fixture explicitly; it must not disappear
through a permissive assertion.

Companion integration regressions exercise coincident cylinder, sphere, cone,
and torus set semantics, exact and gapped circular-interface sewing, STEP
closed-shell export, two independently rebuilt copies of the same imported
curved STEP shell, and local boundary refitting of independently imported
B-spline/NURBS sidewalls, including degree-elevated multi-row STEP surfaces,
without moving the remote cap. These cases validate
document evaluation and persistent topology rather than only the private
contact classifier.

## UI And AI Boundaries

The desktop localizes stage and reason labels, displays operand separation when
bounds are available, and keeps backend detail in a collapsible technical
section. It does not infer failure causes from text.

AI receives `last_boolean_failure` alongside the document and optional analysis
context. This field is evidence about the last rejected edit, not authority to
mutate the document. Any proposed correction still returns as `ModelCommand`
values and passes the ordinary staged kernel validation and user approval flow.
