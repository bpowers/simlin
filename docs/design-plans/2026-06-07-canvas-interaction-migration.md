# Canvas Interaction-Model Migration (tech-debt #65 + #66) and Hooks Conversion

Working plan for the PR #721/#722 follow-up. Lean by request: this doc exists to
brief implementation sub-agents and anchor the adversarial reviews, not as full
design-plan ceremony.

## Definition of Done

1. A reconciler-level (RTL/jsdom) gesture-sequence test suite that drives
   `<Canvas>` through real PointerEvent sequences (down -> move -> up/cancel)
   covering the discrete gestures: click-select, modifier toggle,
   deferred-single-select collapse, group drag, drag-select rectangle, canvas
   pan, pinch mode transitions, label drag, link/flow endpoint drag, creation
   tools (aux/stock/module/flow/link), and name-edit entry/commit/cancel --
   written against CURRENT behavior first and green on the unmodified Canvas.
2. Canvas's boolean `CanvasState` modes + loose instance fields replaced by a
   single `InteractionState` field; `handlePointerDown` / `handleSetSelection`
   / `handlePointerCancel` driven through `reduceInteraction` with effects
   applied; render reads the union. Measure: reads of
   `isMovingArrow|isMovingSource|isMovingCanvas|isDragSelecting|isPinching|isMovingLabel`
   in Canvas.tsx drop to ~0. Continuous pinch/pan/momentum physics stay
   shell-internal by design. Dead `constrainFlowMovement` /
   `constrainCloudMovement` / `constrainStockMovement` deleted (#66).
3. Canvas converted from a class to a `React.memo` function component with
   hooks, gesture suite green throughout, SVG parity test green.
4. One PR on a feature branch; each big piece implemented by a fresh sub-agent
   and adversarially reviewed until no material issues; pre-commit hooks pass.

## Execution shape

Two pieces, sequential, on branch `canvas-interaction-migration`:

- **Piece 1**: (a) gesture test suite against current behavior (own commit),
  then (b) the tagged-union migration + #66 deletion (own commit). One
  adversarial review of the whole piece; supervisor additionally reviews the
  test suite before (b) starts.
- **Piece 2**: hooks-ify Canvas (own commit). Fresh adversarial review.

Sub-agents run one at a time in the main working tree (interdependent edits;
no parallel fan-out). Reviews iterate with fix agents until SHIP.

## Current state inventory (what migrates where)

`CanvasState` booleans / fields -> `InteractionState` variant:

| today | union variant |
|---|---|
| (none of the below true) | `idle` |
| `isMovingCanvas` | `panning` |
| `isDragSelecting` | `dragSelecting` |
| `moveDelta`-driven element move + instance `deferredSingleSelectUid`/`deferredIsText`, state `draggingSegmentIndex` | `movingSelection {deferredSingleSelectUid, deferredIsText, segmentIndex}` |
| `isMovingArrow` / `isMovingSource` + instance `dragPointerType` + `inCreation` presence | `movingEndpoint {endpoint, pointerType, inCreation}` |
| `isMovingLabel` + `labelSide` | `movingLabel {side}` |
| `isEditingName` + `editNameOnPointerUp` + `flowStillBeingCreated` | `editingName {onPointerUp, creatingFlow}` |
| `isPinching` + `initialPinchDistance` + `initialPinchZoom` + `pinchModelPoint` | `pinching {initialDistance, initialZoom, modelPoint}` |

Stays OUTSIDE the union (continuous / shell-owned), per the module's design
comment: `moveDelta`, `movingCanvasOffset`, `dragSelectionPoint`, the Slate
`editingName` value, `inCreation` / `inCreationCloud` concrete elements,
`svgSize`, `initialBounds`, `mouseDownPoint`, `selectionCenterOffset`,
`pointerId`, `activePointers`, velocity/momentum fields.

Known wrinkle: during an aux/stock/module creation drag the current code is
simultaneously "moving" (`moveDelta` set) and `editNameOnPointerUp` -- in the
union this is `editingName {onPointerUp: true}` with `moveDelta` continuing as
a continuous field, matching `reduceInteraction`'s `createToolPointerDown`.

## Piece 1a: gesture test suite

- New file(s) under `src/diagram/tests/` (e.g. `canvas-gestures.test.tsx`),
  jest + jsdom + `@testing-library/react` (all already devDeps).
- Harness: render `<Canvas>` with a small fixture `Project`/`Model`/
  `StockFlowView` built from plain datamodel objects (see
  `canvas-editing-name-done.test.ts` for element factories) and jest.fn()
  callbacks for every `on*` prop. Drive gestures by dispatching pointer events
  on the rendered SVG / element nodes. Assert on (a) callback invocations and
  payloads, (b) rendered DOM consequences (drag rect, EditableLabel overlay,
  element transform updates).
- jsdom gaps to stub/polyfill in the harness (NOT in production code):
  `ResizeObserver`, `Element.prototype.setPointerCapture` /
  `releasePointerCapture` / `hasPointerCapture`, `getBoundingClientRect`
  (return a fixed rect), `PointerEvent` if missing (feature-detect; events must
  carry `pointerId`, `pointerType`, `isPrimary`, `buttons`, `clientX/Y`,
  modifier keys). `requestAnimationFrame` exists in jest-environment-jsdom.
- The suite MUST pass against the unmodified Canvas (it pins current
  behavior). It must also avoid reaching into Canvas instance internals --
  props in, events in, callbacks/DOM out -- so it survives the class->hooks
  conversion unchanged. (This is the entire point: it is the migration gate.)
- Keep runtime well under the pre-commit budget (target: a few seconds).

Gesture coverage checklist (each is a down -> [moves] -> up/cancel sequence):

1. Click empty canvas: selection cleared on up; no drag rect for a wobble
   under the 5px threshold.
2. Drag-select: rect renders during drag; up selects elements whose centers
   fall inside (and aux corner-hit rule); selection replaces.
3. Canvas pan via shift-drag (mouse) and single-finger touch: onViewBoxChange
   called with the panned viewBox on up; selection NOT cleared by a pan.
4. Click element: immediate selection replace (onSetSelection with that uid);
   details panel callback (`onShowVariableDetails`) on clean click; no
   onMoveSelection for sub-threshold wobble.
5. Modifier-click: toggle in/out of selection.
6. Press already-selected element, no modifier, release without drag:
   collapses to single element (deferred single select).
7. Press already-selected element, drag past threshold: group selection
   preserved; `onMoveSelection` with the delta on up.
8. Flow segment drag: `segmentIndex` plumbed through `onMoveSelection`.
9. Label drag: `onMoveLabel` with quadrant side; label-side preview during
   drag.
10. Link arrowhead drag (existing link): `onAttachLink` when released over a
    valid target; delete (`onDeleteSelection`) when released over empty space
    with no invalid-target flag; cloud reattach selection swap on press.
11. Flow endpoint drag: source vs sink (`onMoveFlow` payloads, faux-target
    center when unattached).
12. Creation tools: aux/stock/module press -> staged element renders, drag
    moves it, release enters name editing (EditableLabel appears); commit
    fires `onCreateVariable`; cancel of a created flow fires
    `onDeleteSelection` (flowStillBeingCreated).
13. Link tool / flow tool from stock: press on named element starts endpoint
    drag with `inCreation`.
14. Pinch: two touch pointers enter pinch (single-finger state cleared);
    onViewBoxChange called with zoomed viewBox during moves; pointer-up exits
    pinch cleanly and a subsequent single-finger gesture works.
15. Name editing: double-click(isText) on single named element enters editing;
    Enter commits (`onRenameVariable`), Escape cancels; editing ends when the
    selected tool changes.
16. pointercancel mid-gesture resets to a clean state (no stuck modes).

## Piece 1b: tagged-union migration

- `CanvasState` gains `interaction: InteractionState`; the eight boolean/field
  groups above are removed.
- Expand `canvas-interaction.ts` as needed so the three shell entry points
  (`handlePointerDown`, `handleSetSelection`, `handlePointerCancel`) hit-test
  the raw event into semantic `InteractionEvent`s and apply returned
  `InteractionEffect`s. New event kinds (e.g. pointer-up variants) and effects
  (e.g. moveSelection/moveLabel/attachLink/moveFlow/deleteSelection/
  showDetails/startMomentum/enterNameEdit) are expected; the shell keeps
  geometry/hit-testing and passes results in. Reducer stays pure,
  table-tested; update the SHELL-DRIVEN vs MODEL-ONLY comments (they should
  largely disappear).
- `selection-logic.pointerStateReset` shrinks/adapts to the union.
- Render paths read `state.interaction.mode` (e.g. `isValidTarget`, `cloud()`
  hidden flags, `connector()` drag branch, EditableLabel overlay).
- Delete `constrainFlowMovement` / `constrainCloudMovement` /
  `constrainStockMovement` (+ any imports they alone pull in) -- #66.
- Update `canvas-interaction.test.ts` (reducer surface grows) and
  `canvas-editing-name-done.test.ts` (internals changed); gesture suite stays
  UNTOUCHED and green -- if a gesture test must change, that is a behavior
  change and needs explicit supervisor sign-off.

## Piece 2: hooks-ify Canvas

- `Canvas` becomes a function component (likely `React.memo`), keeping the
  exact `CanvasProps` contract. No consumer holds a ref (verified: Editor and
  StaticDiagram pass props only).
- Interaction state via `useReducer`/`useState`; instance fields become refs
  (`elements`/`derived` caches, `activePointers`, momentum/velocity,
  `pointerId`, `mouseDownPoint`, `selectionCenterOffset`); mount/unmount
  effects own the ResizeObserver and native non-passive wheel/gesture
  listeners.
- MUST remain SSR-safe: `render-common.tsx` calls
  `renderToString(<Canvas embedded .../>)` and the Rust-vs-TS SVG byte-parity
  test (`svg-rendering.test.ts`) gates it. No browser globals at module scope
  or during render; effects don't run under renderToString.
- `deriveRenderState` render-purity invariant carries over: one derivation per
  render, event handlers read the committed derivation (ref written during
  render is acceptable here as it replaces `this.*` writes with identical
  semantics; document it).
- `canvas-editing-name-done.test.ts` converts to harness-driven (it currently
  constructs the class directly).
- Gesture suite and SVG parity stay green, unchanged.

## Review protocol

Fresh adversarial reviewer per piece, zero prior context, briefed with this
doc. Mandate: line-by-line semantic-equivalence proof against the pre-change
code (git diff + both file versions), hunting for dropped branches, reordered
setState consequences, stale-closure bugs (piece 2), and test-suite
weakening. Verdict SHIP or NEEDS-WORK with concrete file:line issues; fix
agents address; re-review until SHIP.
