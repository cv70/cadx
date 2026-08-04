# AI Transaction Sandbox

CADX treats an AI edit as an untrusted proposal until one exact document
revision, one declarative command batch, and one kernel-evaluated result agree.
The preview is a transaction artifact, not UI-only temporary geometry.

## Lifecycle

1. The desktop snapshots the current document, assigns a monotonically unique
   request ID, and records its revision in the asynchronous AI request. The
   tracked Tokio task exposes an abort handle to the host UI. The same revision
   owns a bounded typed context snapshot derived from selection/topology,
   viewport focus, prompt matches, feature-graph neighborhoods, spatial AABB
   distance, active domain schema, and committed scene analysis.
2. The user may cancel the active provider request from the review panel or with
   Escape. A successful document transaction, undo/redo, new document, or open
   document also cancels a request whose snapshot revision is no longer current.
   Cancellation aborts the provider future and immediately clears pending UI
   state; it never waits for the provider timeout.
3. Only a response whose request ID still identifies the active task may enter
   planning review. A response already queued by an aborted or superseded task
   is discarded without changing the active task. A matching response whose
   request revision no longer matches the live revision is rejected before
   command evaluation.
4. The provider may return one primary command batch and at most two independent
   alternatives. Alternatives contain summaries and commands only; engineering
   metrics are not part of the AI contract.
5. For each candidate, `CoreBus::preview_with_metadata` delegates separately to
   `DocumentSession::preview`. The session clones the same live `CadDocument`,
   applies that complete command batch, and asks the configured `CadKernel` to
   evaluate the staged document. A candidate never inherits another candidate.
6. Success returns an opaque `TransactionPreview` containing the base revision,
   command count, staged document, evaluated scene, created feature IDs, and
   `DocumentDiff`. Failure returns the same typed domain or kernel diagnostics as
   a direct edit and cannot expose partial geometry.
7. CADX compares each accepted scene with the committed scene locally. It
   computes body/triangle deltas, volume, surface area, available mass and
   center-of-mass changes, and exact staged-document interference when supported.
8. The desktop shows the command list, structural diff, and local engineering
   evidence. Switching candidates changes only the selected ghost scene.
   `cadx-render` keeps committed geometry in the normal pass and draws changed
   preview geometry in an independent alpha-blended ghost pass.
9. Approval consumes the selected preview through
   `commit_preview_with_metadata` and explicitly discards all other candidates.
   `DocumentSession` first compares the preview base revision with the active
   revision, then installs the already evaluated document and scene as exactly
   one undoable revision.
10. Rejection consumes no document state and publishes `PreviewDiscarded` for
   every candidate.

## Provider Context Budget

- Feature ranking preserves selected and prompt-matched identities before
  dependency, spatial, and recent fallbacks. Two graph hops are retrieved. The
  default/hard detailed-feature budgets are 32/64.
- Spatial entities are ordered by selected state, AABB distance to the resolved
  focus, and feature ID. The default/hard budgets are 16/32. Per-part engineering
  detail uses this set while aggregate scene metrics are retained.
- Only retrieved feature records include complete parametric values. Embedded
  STEP source is replaced with a byte-count marker; opaque domain values become
  namespace entry counts.
- Related product structure is capped at eight assemblies and 32 occurrences
  per assembly while retaining definitions, hierarchy ancestors, and complete
  mate endpoints. Interference detail is capped at 64 candidate IDs and 32
  relevance-ranked pairs.
- Feature, spatial, assembly, occurrence, definition, mate, interference ID,
  interference pair, selected-edge, and domain-schema omissions are explicit.
  The provider is instructed to return no speculative edit when the required
  identity is absent or ambiguous.

## Diff Contract

`DocumentDiff` is deterministic and uses persistent document identities:

- `added_features`: IDs present only in the staged document.
- `modified_features`: IDs present in both documents whose declarative feature
  values differ.
- `removed_features`: IDs present only in the committed document.
- `changed_assemblies`: unioned assembly IDs whose records differ.
- `changed_domain_namespaces`: unioned namespaces whose persisted maps differ.
- `document_name_changed`: document identity metadata changed.

The diff is evidence for review and rendering. It does not replace kernel
evaluation and is never replayed as an edit script.

## Viewport Semantics

- Committed geometry remains normally shaded and is the only source for picking,
  measurement, analysis, and export.
- Added and modified staged solids/reference geometry are cyan and translucent.
- Removed committed solids/reference geometry are red and translucent.
- Ghost solids use a separate WGPU pipeline with alpha blending and depth writes
  disabled. Ghost reference lines use a corresponding blended line pipeline.
- Discard, new/open, undo/redo, or any successful non-AI transaction clears the
  pending preview and rebuilds viewport buffers from committed state.

## Concurrency And Failure Rules

- Request revision and preview revision are separate checks. The first prevents
  planning from stale context; the second prevents committing a preview after
  any later edit.
- Request identity is independent of document revision. It prevents a late
  response from an explicitly canceled request from completing a newer request
  created against the same revision.
- At most one provider task is tracked. Installing a replacement task aborts
  the previous handle defensively, even though the desktop normally disables
  submission while one is active.
- `TransactionPreview` fields are private. External callers cannot manufacture a
  supposedly validated staged scene.
- A failed preview does not change revision, dirty state, undo/redo stacks,
  document, or evaluated scene.
- `analyze_preview_interference` rejects a stale base revision and operates on
  the staged document without changing live session state or history.
- A failed or stale commit publishes `TransactionRejected` and does not mutate
  the session.
- Approval is one history entry regardless of command count.

These rules are the shared foundation for future DFM fixes and
simulation-driven proposals. Those tools may add further locally computed
evidence, but geometry mutation must still terminate at this boundary.
