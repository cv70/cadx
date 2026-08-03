# Domain Pack Architecture

CADX keeps one geometry implementation and layers industry behavior above it.
The geometry kernel is owned by `cadx-core` and `cadx-kernel-truck`; no domain
pack depends on a kernel handle or reimplements B-Rep/2D solving.

```text
UI (egui)                    cadx-desktop
  viewport / schema forms / AI chat
             |
AI native layer              cadx-ai
  context collector / tools / intent diff
             |
Domain Pack Bus              cadx-domain-api::DomainRegistry
  registration + enable/disable + schema validation + execute + NL route
             |
Domain packs                 cadx-mcad-* / cadx-aec-* / cadx-ecad-*
  executable tools, solvers, shaders, AI tools, BIM, DFM, DRC, export
             |
Core runtime                 cadx-app::CoreBus + cadx-core
  source-tagged atomic commands, domain data, events, history
             |
Single geometry kernel       cadx-core + cadx-kernel-truck
  exact sketches and B-Rep evaluation
```

## Pack Boundaries

`cadx-domain-api` is a geometry-neutral SPI. A Pack exposes a manifest, tool
descriptors, schema-driven inspector panels, solver/shader/AI descriptors,
natural-language routing, and export validation. `execute_tool` resolves and
validates typed form parameters, then returns a `DomainExecution` containing
business actions, diagnostics, and artifacts.

The desktop translates all geometry and metadata actions from one execution
into one kernel-validated `ModelCommand` transaction. Namespaced opaque state
is stored in `CadDocument::domain_data`, so BIM and ECAD data participate in
save, undo, redo, and transaction rollback. Pack crates remain independent
from egui, `cadx-core`, Truck, and concrete document entities.

## MCAD

- `cadx-mcad-model`: feature dependency validation, dirty propagation,
  deterministic regeneration order, assembly mate validation, freedom reports,
  and frame proposals.
- `cadx-mcad-standards`: GB/T, ISO, and ASME drawing sheets, annotation types,
  and deterministic standards inspection.
- `cadx-mcad-dfm`: kernel-neutral wall, hole, envelope, material, and process
  manufacturability checks.
- `cadx-mcad-bom`: stable grouped bill-of-materials contracts.
- `cadx-mcad`: aggregate Pack, standard-part catalog, schema, executable tools,
  solver/shader/AI descriptors, and intent routing.

## AEC

- `cadx-aec-bim`: validated projects, storeys, typed properties, classified
  elements, bounds, schedules, wall/slab specifications, and quantities.
- `cadx-aec-analysis`: deterministic broad-phase BIM clash reports.
- `cadx-aec-ifc`: validated deterministic IFC4/IFC4X3 STEP physical-file export.
- `cadx-aec`: aggregate Pack with executable wall, slab, BIM property, schedule,
  quantity, clash, and IFC tools.

## ECAD

- `cadx-ecad-netlist`: schematic component, pin, net, ownership, and impedance
  validation.
- `cadx-ecad-layout`: board outline, alternating copper/dielectric stackup,
  components, nets, pads, vias, traces, keepouts, and electrical rules.
- `cadx-ecad-router`: deterministic orthogonal routing and differential-pair
  proposals.
- `cadx-ecad-drc`: component, trace, edge, copper-clearance, pad, drill, via,
  layer, and net checks.
- `cadx-ecad-export`: validated Gerber copper/edge-cut and Excellon drill bundles
  plus STEP board-outline exchange.
- `cadx-ecad`: aggregate Pack, footprint/net-class libraries, executable tools,
  schema, solver/shader/AI descriptors, and intent routing.

## Runtime Flow

The desktop registers built-in Packs at composition time. Tool buttons with an
associated panel render a form directly from `DomainPanelSchema`; submitted
values become a typed `DomainToolRequest`. Tools without forms execute
immediately. AI domain intents produce the same `DomainAction` values and enter
the same transaction path.

ECAD board and component actions create ordinary core solids and persist board
metadata under `ecad.layout`. AEC wall and slab actions create core solids and
persist BIM identity under `aec.bim` in the same transaction. MCAD feature,
edge, assembly, DFM, drawing, and BOM operations retain core ownership of exact
topology and use Pack logic for domain decisions and reports.

## Adding A Pack

1. Add one or more `cadx-<domain>-<capability>` crates without `cadx-core`,
   `cadx-kernel-truck`, or egui dependencies.
2. Add an aggregate `cadx-<domain>` crate implementing `DomainPack`.
3. Declare inspector schemas and map form-backed tools with `tool_panel`.
4. Implement `execute_tool`, returning actions, issues, and artifacts.
5. Register the aggregate at the composition root and add localized tool IDs.
6. Translate geometry and opaque domain-data actions into one `ModelCommand`
   transaction; keep reports and export generation inside Pack crates.
