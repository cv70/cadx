# Materials and Mass Properties

CADX stores physical material metadata in the kernel-neutral document model.
Each solid `Feature` may have one `Material` with a trimmed name of at most 80
characters and a density greater than zero and no more than 100,000 kg/m^3.
Sketches, datum planes, and datum points cannot carry material because they do
not represent physical volume. `SetMaterial` and `ClearMaterial` are ordinary
`ModelCommand` values, so direct edits and AI proposals share staged kernel
validation, undo/redo, and atomic batch behavior. Duplicating a feature keeps
its material assignment.

Material fields were introduced in `.cadx` schema version 7 and are optional when decoding
v1-v6 documents and default to unassigned. Invalid names, densities, or
material on reference geometry fail validation before a loaded document can
replace active state. Kernels copy validated material metadata onto
`EvaluatedPart`; no kernel-native material object or process-local id crosses
the kernel port.

## Analysis Contract

`cadx-analysis::analyze_scene(scene, density_override_kg_m3)` consumes only the
evaluated triangle scene. A supplied density is a uniform compatibility
override. Without one, each part uses its own material density. Geometry
metrics remain available for unassigned parts, while total mass, center of
mass, and inertia are `None` unless every visible part has a valid density.

Results use these units:

- bounds and centroids: mm;
- surface area: mm^2;
- volume: mm^3;
- density: kg/m^3;
- mass: kg;
- centroidal inertia tensor: kg mm^2.

For each consistently wound closed mesh, analysis integrates the signed
tetrahedra formed by every oriented triangle and a local reference point. It
accumulates volume, first moments, and all six independent second moments,
normalizes reversed winding, and shifts the origin tensor to the part centroid
with the parallel-axis theorem. Using a local reference avoids catastrophic
cancellation for geometry far from the world origin. Scene center of mass is
mass weighted, and part tensors are shifted and summed about that center.

Analysis rejects empty triangle sets, incomplete triangle index triples,
out-of-bounds indices, non-finite coordinates, and invalid density. It does not
repair open or inconsistently wound meshes: their solid volume and inertia are
not physically meaningful, and the CAD kernel remains responsible for
producing closed solid tessellations.

AI receives the serialized analysis as read-only context. It may propose
material commands through the explicit tool schema, with density stated in
kg/m^3, but cannot write computed mass or inertia back into the document.
