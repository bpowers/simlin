// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Pure, table-testable model of the Canvas discrete-interaction state machine.
 *
 * This module owns the DISCRETE gesture state -- the mutually-exclusive modes a
 * pointer interaction can be in (idle, panning, drag-selecting, moving a
 * selection, dragging a link/flow endpoint, moving a label, editing a name,
 * creating an element, pinching) -- as a tagged union, plus the pure
 * transitions between them. It has ZERO React/DOM dependencies.
 *
 * The imperative shell (drawing/Canvas.tsx) keeps everything that is not pure
 * discrete state: pointer capture, screen->canvas coordinate conversion, the
 * raw multi-touch activePointers map, the momentum rAF loop and velocity
 * tracking, the native wheel/Safari-gesture listeners, the Slate rich-text
 * editing value, and all rendering. The shell hit-tests raw DOM events into the
 * semantic InteractionEvents below, feeds them to reduceInteraction, then
 * executes the returned InteractionEffects (each effect is a command the shell
 * performs: pointer capture, prop callbacks, starting momentum, ...).
 *
 * Division of labor: the reducer owns every discrete *mode* transition and
 * chooses *which* effects fire and *in what order*. The shell owns geometry and
 * hit-testing: before raising an event it has already resolved the canvas-space
 * point, the hit element, the drag target, the move delta, and the faux-target
 * centers, and it passes those pre-resolved results into the event (so an effect
 * the reducer emits carries a payload the shell merely executes). This keeps the
 * reducer pure -- it never touches a ViewElement's geometry beyond the explicit
 * pure helpers (labelSideForPointer, isInDragSelectRect, isDrag) -- while still
 * driving all of the shell's discrete behavior through reduceInteraction.
 *
 * Continuous physics (pinch zoom math, pan offset math, momentum frames) stays
 * in the shell and calls props.onViewBoxChange directly; this reducer models
 * only the discrete pinch/pan *mode* transitions, not the per-frame geometry.
 *
 * Selection-set arithmetic lives in selection-logic.ts (already extracted and
 * table-tested); this reducer imports and composes it rather than duplicating
 * it. The point/coordinate math the transitions need (drag-vs-click threshold,
 * label-side quadrants, drag-select rectangle membership) is pure and lives
 * here so it is exercised by the same gesture-sequence tests.
 */

import { UID, ViewElement } from '@simlin/core/datamodel';

import type { Point } from './common';
import { ClickDragThresholdPx } from './pointer-utils';
import { computeMouseDownSelection, computeMouseUpSelection, type MouseDownSelectionResult } from '../selection-logic';

export type LabelSideName = 'top' | 'left' | 'bottom' | 'right';

/**
 * The discrete interaction mode. Each variant carries exactly the data that is
 * valid while in that mode -- replacing the former bag of mutually-exclusive
 * booleans (isMovingCanvas/isDragSelecting/isMovingArrow/...) and the loose
 * instance fields (deferredSingleSelectUid, draggingSegmentIndex, labelSide,
 * pinch fields, inCreation, ...) on the Canvas component.
 */
export type InteractionState =
  | { readonly mode: 'idle' }
  // Single-finger / shift-drag canvas pan. The per-frame offset is continuous
  // physics owned by the shell; the mode only records that a pan is in progress.
  | { readonly mode: 'panning' }
  // Rubber-band selection rectangle from mouseDownPoint to the current pointer.
  | { readonly mode: 'dragSelecting' }
  // Moving the current selection. `deferredSingleSelectUid` is set when the
  // user pressed an already-selected element without a modifier: on pointer-up
  // with no drag, selection collapses to that element (Figma-style); with a
  // drag, the group selection is preserved. `segmentIndex` is the flow segment
  // being dragged (undefined = valve / whole element). Name editing is never
  // deferred here -- a double-click on a label is a terminal `dblclick` and
  // enters `editingName` synchronously in the shell's handleSetSelection.
  | {
      readonly mode: 'movingSelection';
      readonly deferredSingleSelectUid: UID | undefined;
      readonly segmentIndex: number | undefined;
    }
  // Dragging a link or flow endpoint. `endpoint` distinguishes the arrowhead
  // (sink) from the source. `pointerType` drives the touch-is-always-straight
  // rule for links. Whether the dragged element is a not-yet-persisted creation
  // (link/flow tool or flow-from-stock) is NOT recorded here: the shell already
  // owns the concrete in-creation element (`CanvasState.inCreation`) as the
  // single source of truth, and reads it directly on pointer-up.
  | {
      readonly mode: 'movingEndpoint';
      readonly endpoint: 'arrow' | 'source';
      readonly pointerType: string;
    }
  // Dragging an element's label to a new side.
  | { readonly mode: 'movingLabel'; readonly side: LabelSideName }
  // Editing a name (the Slate value itself lives in the shell). `onPointerUp`
  // stages the "start editing once the creation drag finishes" handoff used by
  // the aux/stock/module creation tools. `creatingFlow` marks the just-created
  // flow whose name edit, if cancelled, deletes the flow.
  | { readonly mode: 'editingName'; readonly onPointerUp: boolean; readonly creatingFlow: boolean }
  // Two-finger pinch. The continuous zoom math is the shell's; the mode records
  // the fixed reference captured at pinch start.
  | {
      readonly mode: 'pinching';
      readonly initialDistance: number;
      readonly initialZoom: number;
      readonly modelPoint: Point;
    };

export const idleState: InteractionState = { mode: 'idle' };

/**
 * Semantic, hit-tested inputs to the reducer. The shell produces these from raw
 * DOM events: it has already resolved which element was hit, the canvas-space
 * point, modifier keys, and pointer type. The reducer never sees a DOM event.
 */
export type InteractionEvent =
  // Pressed on empty canvas. `pan` is true for touch / shift (pan), false
  // otherwise (rubber-band drag-select).
  | { readonly kind: 'canvasPointerDown'; readonly pan: boolean }
  // Pressed with a creation tool active (aux/stock/module): stage an element.
  | { readonly kind: 'createToolPointerDown'; readonly tool: 'aux' | 'stock' | 'module' }
  // Pressed with the flow tool on empty canvas: stage a flow + its source cloud.
  | { readonly kind: 'flowToolPointerDown'; readonly pointerType: string }
  // A second touch point started while a single-finger gesture was active: the
  // shell has already computed the fixed pinch reference geometry.
  | {
      readonly kind: 'pinchStart';
      readonly initialDistance: number;
      readonly initialZoom: number;
      readonly modelPoint: Point;
    }
  // The pinch gesture ended (a pinch pointer lifted/cancelled). Returns to idle
  // -- continuing with a single finger after a pinch is intentionally dropped.
  | { readonly kind: 'pinchEnd' }
  // Began dragging an element's label; the shell resolved the quadrant side.
  | { readonly kind: 'labelDragStart'; readonly side: LabelSideName };

/**
 * Whether a pointer move is far enough to count as a drag rather than the
 * incidental jitter of a click. Mirrors pointer-utils.isDragMovement, kept here
 * as the reducer's own threshold check so gesture-sequence tests exercise it.
 */
export function isDrag(moveDelta: Point | undefined, zoom: number): boolean {
  if (moveDelta === undefined) {
    return false;
  }
  return Math.hypot(moveDelta.x, moveDelta.y) * zoom >= ClickDragThresholdPx;
}

/**
 * The side a label snaps to given the pointer position relative to the
 * element's center. Pure quadrant math extracted from Canvas.handleLabelDrag.
 *
 * `angle` is atan2(cy - py, cx - px) in degrees: the direction from the pointer
 * toward the element center. The quadrants are intentionally asymmetric to
 * match the original (a pointer to the LEFT of the center -> angle ~0 -> the
 * label sits on the 'left').
 */
export function labelSideForPointer(center: Point, pointer: Point): LabelSideName {
  const angle = (Math.atan2(center.y - pointer.y, center.x - pointer.x) * 180) / Math.PI;
  if (-45 < angle && angle <= 45) {
    return 'left';
  } else if (45 < angle && angle <= 135) {
    return 'top';
  } else if (-135 < angle && angle <= -45) {
    return 'bottom';
  }
  return 'right';
}

/**
 * Whether an element falls within a drag-selection rectangle. Each element type
 * has its own containment rule, extracted verbatim from
 * Canvas.handlePointerCancel's drag-select loop:
 *   - clouds, stocks, flows, modules, aliases: center-point containment
 *   - aux: center containment OR any rectangle corner inside the aux circle
 *     (passed in via `auxCornerHit` because the circle hit-test is geometry the
 *     shell already owns in Auxiliary.auxContains)
 * Links and groups are never drag-selected.
 */
export function isInDragSelectRect(
  element: ViewElement,
  rect: { left: number; right: number; top: number; bottom: number },
  auxCornerHit: (element: ViewElement) => boolean,
): boolean {
  const centerInside =
    element.x >= rect.left && element.x <= rect.right && element.y >= rect.top && element.y <= rect.bottom;
  switch (element.type) {
    case 'cloud':
    case 'stock':
    case 'flow':
    case 'module':
    case 'alias':
      return centerInside;
    case 'aux':
      return centerInside || auxCornerHit(element);
    default:
      return false;
  }
}

/**
 * The set of element UIDs inside a drag-selection rectangle. Pure given the
 * element list and the aux corner-hit predicate.
 */
export function computeDragSelection(
  elements: Iterable<ViewElement>,
  rect: { left: number; right: number; top: number; bottom: number },
  auxCornerHit: (element: ViewElement) => boolean,
): Set<UID> {
  const selected = new Set<UID>();
  for (const element of elements) {
    if (isInDragSelectRect(element, rect, auxCornerHit)) {
      selected.add(element.uid);
    }
  }
  return selected;
}

/**
 * Mouse-down selection decision (Figma/Illustrator pattern). This is the
 * existing, separately-tested selection-logic.computeMouseDownSelection,
 * re-exported through the reducer's vocabulary so the shell and gesture tests
 * have a single entry point. The selection-set arithmetic deliberately stays in
 * selection-logic.ts (it predates this module and has its own table tests);
 * the reducer composes it rather than duplicating it.
 */
export type MouseDownSelection = MouseDownSelectionResult;

export function decideMouseDownSelection(
  currentSelection: ReadonlySet<UID>,
  clickedUid: UID,
  isModifier: boolean,
): MouseDownSelection {
  return computeMouseDownSelection(currentSelection, clickedUid, isModifier);
}

/**
 * Resolve a deferred single-select on pointer-up: collapse to the deferred
 * element only when no drag occurred (otherwise the group is preserved).
 * Delegates to selection-logic.computeMouseUpSelection.
 */
export function resolveDeferredSelection(deferredUid: UID | undefined, didDrag: boolean): ReadonlySet<UID> | undefined {
  return computeMouseUpSelection(deferredUid, didDrag);
}

/**
 * Commands the shell executes after a transition. The reducer never performs
 * side effects; it returns these for the imperative shell to carry out (call a
 * prop callback, capture the pointer, start the momentum animation, ...). Order
 * is significant and preserved as emitted.
 */
export type InteractionEffect =
  // Capture the pointer on the pressed target so moves keep flowing during a
  // drag even if the cursor leaves the element.
  { readonly kind: 'capturePointer' };

/** Read-only environment a transition needs from the shell. */
export interface InteractionContext {
  /** The currently committed selection. */
  readonly selection: ReadonlySet<UID>;
}

export interface InteractionResult {
  readonly state: InteractionState;
  readonly effects: readonly InteractionEffect[];
}

/**
 * The discrete-interaction transition function. Pure: given the current mode,
 * a semantic (hit-tested) event, and the read-only context, it returns the next
 * mode and the effects the shell should perform.
 *
 * What the reducer owns: the empty-canvas press (pan vs rubber-band drag-select),
 * the three creation tools, the flow tool, pinch enter/exit, and label-drag
 * start. These are mode transitions whose decision is NOT geometry-dominated, so
 * they live here and are table-tested. The shell (`Canvas.handlePointerDown`,
 * `handleLabelDrag`, `handlePointerCancel`) raises the matching event and
 * executes the returned effects. The reducer only ever emits `capturePointer`
 * today: selection replacement and tool-clearing are done by the shell directly
 * (it owns the geometry/hit-testing those depend on), so they are not modeled
 * as effects.
 *
 * What the reducer deliberately does NOT own: element / arrowhead / source press
 * resolution. That is geometry-dominated (which element was hit, cloud-vs-flow
 * reattachment, link/flow-tool staging, the deferred-single-select dance) and is
 * interleaved with shell-only concerns (building the staged ViewElement,
 * deserializing the Slate name value, the reattachment selection override). It
 * therefore lives entirely in the shell's `handleSetSelection`, which composes
 * the pure helpers re-exported here (`decideMouseDownSelection`,
 * `resolveSelectionForReattachment`) and constructs the next `InteractionState`
 * variant directly. There is no `elementPointerDown` event: a single source of
 * truth (the shell) avoids a parallel, easy-to-skew model of press handling.
 *
 * Pointer-UP RESOLUTION likewise stays in the shell by design: it is dominated
 * by geometry (which element is under the cursor, the move delta, faux-target
 * centers, the dragged-link arc) and interleaved branches (a deferred-select
 * resolution falls through into the generic move-commit path). The shell reads
 * `state.interaction`, composes the pure helpers exported here
 * (`resolveDeferredSelection`, `isDrag`, `computeDragSelection`,
 * `labelSideForPointer`), and constructs the next InteractionState directly.
 * Continuous physics (per-frame pan offset, pinch zoom math, momentum frames) is
 * likewise shell-internal; the reducer only marks the pan/pinch *mode*.
 */
export function reduceInteraction(
  _state: InteractionState,
  event: InteractionEvent,
  _ctx: InteractionContext,
): InteractionResult {
  switch (event.kind) {
    case 'canvasPointerDown': {
      // Empty-canvas press: pan (touch/shift) or rubber-band drag-select.
      // Selection is not cleared here -- that happens on pointer-up so a press
      // that turns into a pan does not flicker the selection away.
      return {
        state: event.pan ? { mode: 'panning' } : { mode: 'dragSelecting' },
        effects: [],
      };
    }

    case 'createToolPointerDown': {
      // Aux/stock/module creation tool: stage an element (the shell builds the
      // concrete ViewElement) and enter the editing-on-pointer-up handoff.
      return {
        state: { mode: 'editingName', onPointerUp: true, creatingFlow: false },
        effects: [{ kind: 'capturePointer' }],
      };
    }

    case 'flowToolPointerDown': {
      // Flow tool on empty canvas: stage a flow + source cloud and immediately
      // enter arrowhead-drag so the user drags the sink into place.
      return {
        state: { mode: 'movingEndpoint', endpoint: 'arrow', pointerType: event.pointerType },
        effects: [],
      };
    }

    case 'pinchStart': {
      // Two-finger pinch began: capture the fixed reference. The shell has
      // already cleared its single-finger pointer state.
      return {
        state: {
          mode: 'pinching',
          initialDistance: event.initialDistance,
          initialZoom: event.initialZoom,
          modelPoint: event.modelPoint,
        },
        effects: [],
      };
    }

    case 'pinchEnd': {
      // A pinch pointer lifted: return to idle for a clean restart.
      return { state: idleState, effects: [] };
    }

    case 'labelDragStart': {
      return { state: { mode: 'movingLabel', side: event.side }, effects: [] };
    }
  }
}
