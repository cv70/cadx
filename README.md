# CADX

CADX is an AI Native CAD desktop application implemented in Rust 2024 Edition.
It treats a CAD project as a design workspace where a task-oriented agent can
observe the model, plan, use constrained CAD tools, validate work, and save
editable results directly into a replayable history.

The first runnable vertical slice includes:

- a typed, deterministic CAD document and atomic command transactions;
- task-scoped write authority instead of a global AI write path;
- semantic commits, periodic snapshots, branches, and deterministic restore;
- a local `egui` workbench for tasks, model inspection, history, and version
  forks; and
- a deterministic demo planner behind the same tool and authorization boundary
  intended for future local, cloud, and enterprise model providers.

The prototype is a design-workspace foundation, not yet a full geometry kernel
or DXF/STEP/PDF interchange product.

## Documentation

- [Implementation specification](docs/implementation.md)

## Run

```sh
cargo run -p cadx-app
```

Try a task such as `Create a mechanical mounting bracket`, `Create a room`, or
`Create a drafting concept`. The demo planner saves its results as a semantic
commit. Clicking a commit in Design History opens that version on a new branch.

## License

MIT. See [LICENSE](LICENSE).
