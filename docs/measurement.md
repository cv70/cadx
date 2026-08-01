# Persistent Topology Measurement

CADX measurements are read-only queries over the committed `EvaluatedScene`.
They never mutate the document, retain a kernel object, or treat a viewport
coordinate as model geometry. Each operand is a persistent `FaceRef`,
`EdgeRef`, or `VertexRef` and is resolved again after every rebuild.

## Supported Results

| Selection | Result | Geometry contract |
| --- | --- | --- |
| One edge | Arc length | Analytic for lines; adaptive Gauss-Kronrod integration for other B-Rep curves |
| Two vertices | Distance and signed XYZ delta | Exact B-Rep vertex coordinates in model space |
| Two edges | Unoriented angle from 0 to 90 degrees | Linear edges only |
| Two faces | Unoriented angle from 0 to 90 degrees | Analytic planar support surfaces only |
| Two parallel faces | Support-plane spacing | Analytic planar support surfaces only |

An unoriented angle is independent of arbitrary edge traversal and face-normal
sign. Parallel-face spacing measures the infinite supporting planes. It is not
presented as the minimum distance between two bounded face regions.

Curved-edge angle needs an explicit evaluation parameter or picked point;
general face-to-face minimum distance needs a kernel extremum solver over
trimmed domains. CADX reports those selections as unsupported until those
contracts exist rather than substituting tessellation proximity.

## Length Precision

Line length is the Euclidean distance between exact B-Rep endpoints and is
tagged `LengthPrecision::Exact`. Other Truck curves are integrated from their
parametric derivative with an adaptive 15-point Gauss-Kronrod rule. Integration
is split across the curve's display parameter intervals and targets an absolute
error of `1e-8 mm`; the Gauss/Kronrod difference is returned as an estimated
absolute error.

If derivative evaluation is non-finite or integration does not converge within
the bounded recursion depth, rendering can still use the sampled polyline but
measurement returns `LengthAccuracyUnavailable`. The sampled chord length is
never relabeled as a precision-qualified engineering result.

## Failure Semantics

`cadx-analysis::measure` distinguishes lost and ambiguous topology. It also
rejects mixed entity kinds, non-linear edge-angle requests, non-planar face
relationships, degenerate directions, and curve lengths without accuracy
metadata. There is no nearest-entity or nearest-triangle fallback.

The desktop measurement set is transient UI state. It keeps up to two operands
of the same kind, highlights them separately from the current property
selection, and draws a model-space guide between two vertices. Document edits,
undo, and redo retain operands only while they still resolve uniquely; document
replacement clears the set. A successfully computed result is serialized into
`AiContext` as read-only evidence and does not grant the model a new mutation
capability.
