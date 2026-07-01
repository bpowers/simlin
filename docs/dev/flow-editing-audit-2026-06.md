# Flow-Editing Audit (June 2026)

Deep audit of the interactive flow-editing path in `src/diagram` (the TypeScript
editor -- not the engine's auto-layout). Sources: full read of `drawing/Flow.tsx`,
`flow-attach.ts`, `group-movement.ts`, `drawing/Canvas.tsx`,
`drawing/canvas-interaction.ts`, the Editor/controller flow handlers, and the
flow test suites; plus live exploration against the dev server (the
`bpowers/fooz` scratch project). This document records what was found and which
fixes were chosen, so the PR built from it has a durable rationale.

## P1: Correctness bugs found live

### 1. Rename leaves the live view stale; the next edit persists the divergence

`Editor.handleRename` is the only handler that mutates the view through a patch
op (`upsertView` bundled with `renameVariable`) instead of going through
`ProjectController.updateView`. The patch applies correctly in the engine and
the save is correct, but `updateProject`'s unconditional `preserveLiveView`
keeps the active model's (pre-rename) live view, so the on-screen label never
changes -- the rename looks like it silently failed. Any subsequent
geometry-editing gesture then round-trips that stale view back through
`updateView`, overwriting the engine's corrected view: the model says the new
ident, the view says the old one, and the divergence is saved.

Observed end-to-end in `bpowers/fooz`: a flow rename left the model variable
`drain_` with a view element still named "New Flow 1"; clicking that flow's
valve crashes the editor on every page load (see finding 3).

Fix: `handleRename` applies the content op (`renameVariable`) via
`applyPatchOrReportError` and commits the renamed view through
`updateView(updatedView, { recordHistory: true })` like every other discrete
edit, so the optimistic live view is the renamed one. Defense in depth: the
controller's `applyPatch` path now adopts the engine view (skips
`preserveLiveView`) when the patch it just applied contains an `upsertView` op
for the active model -- an explicit view op is newer user intent than the
optimistic live view by construction.

### 2. Plain Enter does not commit a name edit

`EditableLabel.handleKeyPress` committed only on ctrl/shift/alt+Enter; a plain
Enter inserted a newline into the (multi-line-capable) Slate label. Every
"create element, type name, press Enter" interaction therefore left the editor
open with a trailing newline staged. On click-away the name committed with the
embedded newline; `handleRename` escaped it (`replace('\n', '\\n')`, first
occurrence only) and `canonicalize` mapped the result to idents like `drain_`.

Fix: plain Enter (and NumpadEnter) commits; shift+Enter inserts the newline
(standard convention, still supporting multi-line labels); Escape still
cancels. Name commits are sanitized: trailing/leading whitespace and blank
lines are trimmed, interior newlines are preserved as intentional line breaks,
and a commit that produces an empty name is treated as cancel (which for a
just-created flow keeps the existing delete-on-cancel semantics).

### 3. A model/view divergence crashes the whole editor and cannot be repaired

`Editor.getDetails` resolves the selected element's variable with `getOrThrow`
during render, so a view element whose variable is missing takes down the
entire editor via the ErrorBoundary. Because the details panel is ALSO the only
delete affordance, and there is no keyboard delete, a corrupt element cannot be
removed: clicking it to delete crashes first. (Same hardening class as the
dangling-ref rendering fixes #812/#817.)

Fix: `getDetails` degrades gracefully (no panel + console.warn) when the
variable is missing, and keyboard deletion (finding 4) provides a
details-independent repair path.

### 4. No keyboard delete; clouds and corrupt elements have no delete path at all

The Editor's global keydown only handles undo/redo. Deletion requires opening
the variable-details panel -- which unnamed elements (clouds) do not have, and
which crashes for divergent elements (finding 3).

Fix: Delete/Backspace deletes the current selection (guarded by
`isEditableElement` so text fields are unaffected, and inert while a name edit
is showing). Escape clears an armed creation tool, else the selection.

## P2: Geometry robustness

### 5. Slightly-diagonal legacy flows are misrouted

All routing classification uses exact coordinate equality (`p1.y === p2.y`).
The pre-fix creation bugs left persisted flows that are a few pixels off-axis
(the fooz "New Flow" drifts 5.5px); imported models can carry the same. Such a
visually-horizontal flow classifies as *vertical* (its endpoint y's differ), so
dragging its stock routes an L the wrong way -- valve and label end up stacked
on the source cloud.

Fix: orientation decisions in the routing paths (`computeFlowRoute` /
`getFlowAttachmentInfo` / `adjustFlows`) classify by the dominant axis of the
anchor-side segment instead of exact equality. Exact equality remains the
definition of *orthogonal* for rendering/normalization; dominance only breaks
ties for classification so near-axis data routes the way it looks.

### 6. Flow creation leaves the source endpoint at the stock's center

Dragging a new flow out of a stock stages the source point at the stock CENTER
and `computeFlowAttachment` routes only the sink, so the persisted source
endpoint sits at the center (hidden by z-order) instead of on the stock edge,
violating the LAYOUT.md attachment rule until the next stock drag re-pins it.

Fix: at commit, creation routes the source point onto the stock edge facing the
sink (same `UpdateCloudAndFlow` routing the sink already gets).

## P3: Duplication and dead weight (behavior-preserving cleanup)

- `group-movement.ts` contains the cloud L-shape conversion three times
  (`routeCloudEndpointFlow` plus two ~50-line copies inside
  `routeUnselectedFlows`' source/sink loops, which are themselves
  near-duplicates); the copies' `cloudIsSelected` guard is always true by
  construction (the maps are keyed by selected endpoints). Extracted into one
  helper; the source/sink loops parameterized.
- `preProcessSelectedFlows` returns a `sideEffects` array that is always
  empty; `hasSelectedEndpoints` reduces to `selection.size > 0`;
  `routeUnselectedFlows` takes two element iterables that every caller passes
  identically; the deprecated `Point2D` alias. All removed/simplified.
- `processSelectedFlow` repeats the "proposed valve, clamp to new path" block
  three times; extracted.
- `adjustFlows` (Flow.tsx) carries a `FIXME: reduce this duplication` --
  near-identical cloud/non-cloud valve-fraction math whose duplication caused
  the #818 NaN guard to be needed twice. Unified.
- The creation sentinel UIDs are duplicated between `Canvas.tsx` and
  `flow-attach.ts` with a "must stay in sync" comment and a pinning test;
  moved to one shared module (`drawing/creation-sentinels.ts`) imported by
  both.
- The straight-flow -> L-shape conversion exists independently in
  `UpdateCloudAndFlow`, `UpdateFlow`, and `routeCloudEndpointFlow`; the
  perpendicular-dominance threshold logic is shared now.

## Deferred (tracked as issues, not in this PR)

- Straight stock-to-stock flows cannot be offset at all: the perpendicular
  valve-drag reroute requires a cloud endpoint, and segment drags require
  interior segments. Offsetting needs a Z-shape (two corners), a new
  capability.
- The failed-patch path in `handleFlowAttach` silently discards the drawn
  flow (toast only); consider preserving the gesture or a clearer error
  surface.
- `bpowers/fooz` still contains the pre-fix corrupt pair (model `drain_` /
  view "New Flow 1") plus scratch elements; after this PR the corrupt flow can
  be deleted in the UI (keyboard delete + non-crashing details).

## Testing approach

Controller-level reproduction tests (fake engine) pin finding 1; jsdom
component tests pin findings 2-4; pure-function tests pin findings 5-6 and all
P3 refactors (characterization tests written against current behavior before
each extraction).
