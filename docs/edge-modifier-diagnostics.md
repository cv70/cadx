# Edge Modifier Diagnostics

CADX represents rejected chamfers and fillets as kernel-neutral
`EdgeModifierDiagnostic` values. A failed modifier remains a failed staged
transaction: the document, evaluated scene, revision, and undo history do not
change. The diagnostic crosses the kernel, application, desktop, and AI
boundaries without converting its cause into display text.

## Diagnostic Schema

Each diagnostic carries:

- the modifier feature id and `chamfer` or `fillet` operation;
- the optional source feature id and complete canonical `EdgeRef` set;
- a stable pipeline `stage` and machine-readable `reason`;
- the `distance` or `radius`, its value in millimeters, and kernel tolerance;
- zero-based offending edge indices when the backend can identify them; and
- backend `detail` for engineering inspection.

Consumers branch on `stage` and `reason`. The `detail` field is not a protocol,
and UI or AI code must not parse it to infer a correction.

## Stages

| Stage | Boundary |
| --- | --- |
| `reference_resolution` | Resolve the source feature and persistent edge names. |
| `geometry_validation` | Check tolerance, curve and support-surface types, convexity, corner compatibility, and source-shell requirements. |
| `construction` | Build cutters, exact blend surfaces, or an explicit corner miter. |
| `result_validation` | Require a nonempty, closed, geometrically usable B-Rep. |
| `topology_naming` | Assign deterministic persistent lineage to every result face. |

## Reasons

`empty_edge_set` and `mixed_source_features` describe invalid selection sets.
Normal document commands reject those sets before kernel evaluation, but the
diagnostic vocabulary also covers malformed or alternate frontends.

`lost_reference` and `ambiguous_reference` preserve the two distinct persistent
topology failures. CADX never resolves either by choosing a geometrically nearby
edge. `non_linear_edge`, `non_planar_support`, `non_convex_edge`,
`shared_vertex_unsupported`, and `non_convex_source` report explicit capability
boundaries before construction.

`parameter_below_tolerance` is emitted before a shape operation.
`parameter_exceeds_topology` is emitted only when CADX can prove the treatment
collapses or removes the supported topology. An opaque Truck rejection remains
`kernel_rejected`; CADX does not guess that a radius or distance is too large.

`kernel_panic`, `invalid_result_topology`, and `topology_naming_failed` identify
the remaining guarded boundaries. Panics are contained and never unwind through
the session or desktop.

## Desktop And AI

The edge-modifier dialog stays open after rejection. It renders localized
reason and stage labels plus parameter, source, tolerance, and offending-edge
evidence. Backend detail is kept in a collapsed technical section.

`CadxApp` retains the latest edge-modifier diagnostic until the next successful
transaction or document replacement. AI receives it as
`last_edge_modifier_failure` in read-only context. A proposed correction is
still only a `ModelCommand` batch and must pass staged domain and kernel
validation plus explicit user approval.

Diagnostics are transient application state and are not serialized in `.cadx`
documents.
