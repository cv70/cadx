# CADX Module Map

This document is the structural contract for the workspace. `docs/architecture.md`
explains *why* the boundaries exist; this file records *where* everything lives
and which rules a machine enforces.

Every rule below is checked by `cargo test -p cadx-arch`. That crate has no
dependencies and reads the workspace from disk, so a rule can never change
shipped behavior — it can only fail the build.

## Layers

Layers are not one linear stack. The geometry column (`Geometry` → `Domain` →
`Analysis`/`Application`) and the plugin column (`Spi` → `Pack`) are deliberately
independent, and only the composition root may see both.

| Layer | Crates | May depend on |
| --- | --- | --- |
| `Geometry` | `cadx-sketch` | nothing |
| `Domain` | `cadx-core` | `Geometry` |
| `Spi` | `cadx-domain-api` | nothing |
| `Pack` | `cadx-mcad*`, `cadx-aec*`, `cadx-ecad*` | `Spi`, `Pack` |
| `Analysis` | `cadx-analysis` | `Domain`, `Geometry` |
| `Configuration` | `cadx-config` | nothing |
| `Application` | `cadx-app` | `Domain`, `Geometry` |
| `Infrastructure` | `cadx-kernel-truck`, `cadx-io`, `cadx-ai` | `Domain`, `Geometry`, `Analysis`, `Configuration`, `Spi` |
| `Presentation` | `cadx-render`, `cadx-i18n` | `Domain`, `Geometry` |
| `Composition` | `cadx-desktop` | everything |
| `Contract` | `cadx-arch` | nothing |

`cadx-arch::CRATE_LAYERS` is the authoritative list. A new workspace member
without an entry fails `every_member_is_classified_into_exactly_one_layer`, and
an entry naming a crate that no longer exists fails the same test.

## Forbidden technologies

Inward layers must not name outward technologies even transitively through a
manifest. `Layer::forbidden_technologies` lists crate-name prefixes matched
against a dependency's first `-`-separated segment, so `truck` covers
`truck-modeling` and `truck-topology`.

| Layer | Must not depend on |
| --- | --- |
| `Geometry`, `Domain`, `Spi`, `Pack`, `Analysis` | UI (`egui`, `eframe`, `rfd`, `iconflow`), GPU (`wgpu`, `bytemuck`, `glam`), kernels (`truck`, `ruststep`, `lib3mf`), providers (`genai`, `tokio`), filesystem (`home`, `tempfile`) |
| `Application`, `Configuration` | the same, minus the filesystem group |
| `Infrastructure` | UI only — an adapter owns its backend but never draws |
| `Presentation` | kernels and providers — it owns the GPU, nothing else |

## Ratchets

Two lists record debt that is known, bounded, and scheduled. Both may only
shrink: a dedicated test fails when an entry becomes unnecessary, which forces
the entry to be deleted rather than quietly outliving the problem.

- `cadx-arch::PENDING_INVERSION` — outward crate dependencies that exist today.
  `stale_inversions` fails once the edge is gone.
- `cadx-arch::PENDING_DECOMPOSITION` — source files over the line budget.
  `stale_exemptions` fails once the file fits.

Adding an entry to either list is an architecture decision and must be recorded
in the table below with its planned resolution.

| Ratchet | Entry | Why it exists | Planned resolution |
| --- | --- | --- | --- |
| `PENDING_INVERSION` | `cadx-app` → `cadx-io` | `plan_step_import` consumes `cadx-io`'s parsed STEP DTOs directly | Move the kernel-neutral STEP import DTOs down into `cadx-core` so the use case depends on the contract, not the adapter |

## File budget

No `.rs` file may exceed **1000 lines** (`cadx-arch::FILE_LINE_BUDGET`). A module
that outgrows that stops being reviewable in one sitting and starts hiding
coupling. The remedy is always to split it into cohesive submodules — never to
raise the budget.

The budget applies to `src/` and `tests/` in every workspace member.

## Crate submodules

Submodule layout per crate. A crate root (`lib.rs`) is a module declaration and
re-export surface; it holds no logic beyond the type that defines the crate's
entry point.

<!-- SUBMODULES -->
