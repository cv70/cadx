# Product Interference Analysis

CADX exposes interference analysis through the kernel-neutral `CadKernel`
port. Truck B-Reps remain private to `cadx-kernel-truck`; application, desktop,
and AI consumers receive only a serializable `InterferenceAnalysis` report.
The operation is read-only and never creates a document revision.

## Product Candidates

The candidate set represents engineering presence rather than presentation:

- effectively suppressed occurrence bodies are absent because Truck skips them
  before B-Rep reconstruction;
- feature visibility is ignored, so hiding a physical product does not clear a
  clash;
- a solid consumed by an evaluated Boolean, chamfer, or fillet is feature
  history rather than an additional product body; only the terminal result is
  retained; and
- bodies owned by the same multi-body occurrence are not paired with each
  other. Bodies in distinct occurrences are paired, including repeated uses of
  one component definition.

Candidate feature ids and pair iteration are sorted, making reports stable
across runs. `total_pair_count` excludes same-occurrence pairs.

## Evaluation

Truck materializes the document once and retains its private `NamedSolid` map
for the analysis pass. Definition-scoped imported STEP reconstruction reuse and
ordinary suppression rules therefore apply exactly as they do to viewport
evaluation.

For each eligible pair:

1. exact world-space B-Rep vertex bounds perform an AABB broad phase;
2. disjoint pairs increment `clear_pair_count` without allocating pair detail;
3. bounded topology proofs resolve coincident solids and non-crossing contact;
4. strict containment may return the contained original B-Rep after
   tolerance-tessellated closed boundaries prove no surface crossing and place
   every candidate boundary vertex inside the host; and
5. remaining pairs use Truck's B-Rep intersection operation.

Every retained pair identifies its method as `brep_boolean`,
`non_crossing_contact`, or `boundary_classified_containment`. The report stores
only AABB-overlapping pair details, so memory grows with broad-phase hits rather
than all possible pairs.

## Volume And Failure Evidence

Intersection topology is a Truck B-Rep. Truck does not expose analytic volume
integration for its curved modeling surfaces, so CADX triangulates that exact
intersection at the kernel chord tolerance and integrates the closed mesh.
Each successful outcome carries
`InterferenceVolumePrecision::Tessellated { chord_tolerance_mm }`. Volumes at or
below `chord_tolerance_mm^3` are classified clear, and the exact threshold is
included as `volume_tolerance_mm3`.

A kernel rejection, panic, invalid result, empty integration mesh, or non-finite
volume becomes a typed per-pair `Failed` outcome. It increments
`failed_pair_count` and makes `is_complete()` false. Failures are never coerced
to zero volume or counted clear. The report deliberately does not sum pair
volumes because intersections among three or more products can overlap and a
naive sum would double-count material.

## Consumers

`DocumentSession::analyze_interference()` exposes the active revision without
mutating history. The desktop Design toolbar opens a localized report and can
select a reported feature. A successful edit, undo, redo, new, or open action
invalidates the displayed report. When the kernel advertises
`interference_analysis`, a fresh optional report is included in `AiContext` as
read-only evidence; the model still has access only to reviewed declarative
commands.

Regression coverage includes disjoint bounds, touching contact, partial
overlap, containment, presentation-hidden products, terminal feature history,
deterministic ordering, repeated imported occurrences, suppression,
definition-reconstruction reuse, exact box overlap volumes, and multi-body
occurrence self-pair exclusion.
