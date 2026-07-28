# CADX Implementation Specification

## 1. Product Definition

CADX is a local-first, AI Native CAD desktop application. It is designed as a
design workspace analogous to a coding-agent workspace: a user provides a goal,
the agent observes the current project, plans and invokes typed tools, validates
its work, and persists incremental results. The working model remains fully
editable throughout.

CADX is not a conversational command palette. Chat is one way to create and
inspect tasks, while the primary artifacts are the model graph, task timeline,
semantic history, branches, and visible design evidence.

### Product principles

1. **Intent produces editable structure.** An agent creates parameters,
   constraints, semantic building objects, sketches, features, annotations, and
   dependencies. It must not flatten a result into uneditable presentation data.
2. **Autonomy is task-scoped.** A user grants a task narrowly scoped tool
   capabilities. Within that authority an agent can write directly; no global
   model write privilege exists.
3. **Saving is continuous.** A result is saved after each valid action. Task
   completion is an agent lifecycle state, not a condition for persistence.
4. **History explains changes.** Every write has a human intent, typed command
   transaction, document diff, validation evidence, task source, and parent
   version. Users can compare, restore, or fork any version.
5. **Geometry is locally authoritative.** Document validation, geometric
   constraints, feature regeneration, rendering, and final serialization run on
   device. A remote provider may propose tools but cannot mutate a document.

### Value delivered by one task runtime

| Task template | Desired outcome |
| --- | --- |
| Intent to model | Translate requirements, constraints, and manufacturing rules into an editable parametric model. |
| Understand and modify | Inspect imported drawings/models, locate related intent, and apply repeatable changes. |
| Explore alternatives | Fork a model, generate options under stated constraints, evaluate them, and retain comparable branches. |

Mechanical, architectural, and 2D workflows use the same runtime and history
semantics. Their geometry, validation, and exchange capabilities are delivered
as domain packages rather than as separate applications.

## 2. Current Workspace

The repository is a Rust 2024 workspace:

```text
cadx/
  crates/
    cadx-core/       # Document, typed commands, tasks, history, snapshots
    cadx-agent/      # Planner contract and task runner
    cadx-app/        # Native egui desktop workbench
  docs/
```

`cadx-core` has no AI, renderer, or window-system dependency. The app is a
local `egui` workbench using `eframe`; future high-performance viewport drawing
belongs in an isolated `cadx-render` crate that consumes immutable render scene
data. `cadx-io` and remote-provider adapters will be added only once their
format and credential contracts are implemented.

## 3. Core Data and Mutation Contract

The native document stores stable IDs, units, layers, editable entities, and
parameters. Entities initially cover basic drafting, closed sketch profiles and
extrudes, and architectural walls and rooms. The in-memory model is versioned
from the start so migrations can occur before editing.

```rust
pub struct CadDocument {
    pub schema_version: u32,
    pub metadata: DocumentMetadata,
    pub units: Units,
    pub layers: BTreeMap<LayerId, Layer>,
    pub entities: BTreeMap<EntityId, Entity>,
    pub parameters: BTreeMap<ParameterId, Parameter>,
}

pub enum CadCommand {
    CreateLayer { layer: Layer },
    CreateEntity { entity: Entity },
    UpdateEntity { entity: Entity },
    DeleteEntity { id: EntityId },
    SetParameter { parameter: Parameter },
}
```

`CommandTransaction` validates against an isolated temporary document before
changing the real one. It produces a `DocumentDiff` and either applies every
command or none. Human tools, import adapters, and agents all use this same
path. Direct task authority does not bypass command validation.

## 4. Tasks, Agent Runtime, and Authority

A `DesignTask` holds a title, goal, authority, lifecycle status, tool/event
log, and resulting semantic commits. `TaskAuthority::DirectWrite` grants an
explicit set of capabilities such as `Mechanical`, `Architecture`, `Drafting`,
or `Parameters`; `ReviewOnly` can observe and plan but cannot write.

The runner implements this loop:

```text
observe document and task
  -> plan typed CAD actions
  -> record each tool call
  -> validate and atomically apply one transaction
  -> create semantic commit and snapshot when scheduled
  -> continue, pause, fail, or complete
```

The runner accepts an interchangeable `TaskPlanner`. The current
`HeuristicPlanner` is a deterministic local demonstration that maps goals to
drafting, mechanical, or architecture command transactions. A production
planner may use local, cloud, or enterprise-hosted models, but must return the
same typed `PlannedAction` values. It is not permitted a mutable document
reference.

Before a cloud planner receives context, the app must show the endpoint, model,
capability request, selected-object count, whether source files are included,
and an exact payload summary. API credentials live in the operating-system
credential store and never in a project, commit, ordinary log, or crash report.

## 5. Semantic History

`History` records an initial snapshot, one `SemanticCommit` per successful
transaction, periodic full snapshots, named branch heads, and the active branch.
Each commit contains its parent, originating task, user/agent intent,
transaction, diff, and `ValidationReport`.

Restoration starts at the closest ancestor snapshot and deterministically
replays command transactions to the requested commit. Opening a historical
version in the workbench creates or activates a branch, preserving the current
line of work. This supports long-running autonomous tasks, crashes, option
exploration, comparison, and recovery without treating the model as an opaque
binary blob.

The `.cadx` persistence adapter will serialize the document, commits, snapshots,
task records, branches, and schema manifest as a versioned archive. It is the
lossless source of editability; DXF, STEP, and PDF are exchange boundaries.

## 6. Workbench Interaction

The current desktop shell provides:

- a task panel for entering goals, creating tasks, enabling/disabling direct
  save authority, running the agent, and reading its event log;
- a central model-space viewport that renders the current editable entities;
- a model graph and inspector for layers, entities, feature relationships, and
  basic properties;
- a semantic history panel where selecting a commit opens it on an option
  branch; and
- a persistent status bar identifying local state, units, branch, and automatic
  history saving.

The production viewport must add pan, zoom, orbit, snapping, selection,
constraint feedback, layers, off-screen GPU picking, and 2D/3D scene
extraction. Rendering receives immutable document-derived data and cannot write
model state.

## 7. Domain Packages and Exchange Roadmap

The initial core establishes only a vertical slice. The next domain packages
extend stable semantic types and tool schemas rather than replace the document
or history model.

| Package | Additions |
| --- | --- |
| 2D drafting | Arcs, dimensions, styles, snapping, DXF import/export, and PDF drawing export. |
| Mechanical | Constraint solver, parameter expressions, feature graph, B-rep kernel, booleans, drawings, and STEP exchange. |
| Architecture | Floors, doors, windows, levels, schedules, room calculations, and building-rule validation. |
| Recognition | Local PDF/image extraction plus reviewable import tasks; ambiguity is preserved as evidence, never silently guessed. |

Assemblies, CAM, simulation, complete IFC/BIM coverage, real-time collaboration,
and arbitrary third-party executable plugins remain outside the first product
boundary.

## 8. Quality Gates

- Unit tests prove transaction atomicity, geometry/reference validation,
  capability enforcement, snapshot-plus-replay restoration, and branch
  isolation.
- Agent tests prove a planner cannot bypass workspace authority and that each
  auto-saved result can be restored deterministically.
- Renderer tests will cover transforms, picking IDs, device resize/recovery, and
  workbench screenshots on supported desktop platforms.
- Format fixtures will verify lossless native save/load and documented DXF,
  STEP, and PDF subsets.
- Recorded-provider integration tests will verify context disclosure, consent,
  invalid tool rejection, tool budgets, task pause/resume, and recovery from
  interrupted execution.

## 9. Delivery Sequence

1. Complete native project persistence, migrations, history comparison, and
   task pause/resume around the implemented core contracts.
2. Add a dedicated GPU render crate and complete 2D authoring/picking.
3. Add a provider adapter, operating-system credential storage, disclosure UI,
   recorded-response testing, and execution budgets.
4. Deliver mechanical and architectural capability packages as focused vertical
   slices, then their exchange adapters.
5. Add recognition, packaging, accessibility, performance profiling, and sample
   task workspaces for macOS, Windows, and Linux.
