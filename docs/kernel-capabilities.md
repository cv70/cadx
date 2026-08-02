# Kernel Capabilities

`CadKernelCapabilities` is the kernel-neutral declaration of modeling behavior
that changes command availability. It prevents desktop and AI adapters from
assuming that every `CadKernel` has Truck's exact feature set.

The default capability value disables chamfer, fillet, and product interference
analysis. A kernel must opt in explicitly. Each edge-modifier declaration records:

- whether the operation and multi-edge selections are supported;
- whether all edges must belong to one source feature;
- whether edges must be linear and convex;
- whether both support faces must be planar; and
- the shared-vertex support level.

Shared-vertex support is one of `unsupported`, `convex_polyhedral_source`, or
`supported`. The middle value is intentionally distinct: it describes CADX's
explicit convex-polyhedron chamfer miter without claiming general curved or
non-convex corner support.

Truck currently declares multi-edge chamfer and fillet over one source feature,
with linear convex edges and planar support faces. Chamfer declares
`convex_polyhedral_source` shared-vertex support; fillet declares
`unsupported` because exact cylinder intersections and vertex corner patches
are not yet available.

Truck also declares `interference_analysis: true`. This advertises the
read-only report operation, not a document command. The report retains its own
per-pair method, precision, and failure evidence; see
[`interference-analysis.md`](interference-analysis.md).

`DocumentSession` exposes the declaration without leaking a concrete kernel.
The desktop uses it to disable absent operations, unsupported multi-edge
selection counts, and unavailable analysis tools. AI receives the complete value in read-only context and is
instructed to plan only within the declared contract.

A capability is not proof that a particular selection is valid. Persistent
reference resolution, geometric checks, exact construction, result validation,
and topology naming still run inside the staged kernel transaction. Rejections
return an [`EdgeModifierDiagnostic`](edge-modifier-diagnostics.md) and never
commit partial state.
