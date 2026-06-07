// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { setAdd, setDelete } from '@simlin/core/common';

import { UID } from '@simlin/core/datamodel';

import type { InteractionState } from './drawing/canvas-interaction';

export interface MouseDownSelectionResult {
  newSelection: ReadonlySet<UID> | undefined;
  deferSingleSelect: UID | undefined;
}

/**
 * Determines selection state on mouseDown. This implements the standard
 * selection pattern used by Figma, Illustrator, etc:
 *
 * - Modifier key: toggle element in/out of selection immediately
 * - No modifier, element already in selection: defer -- don't change
 *   selection yet, so group drag can proceed without dissolving
 * - No modifier, element NOT in selection: replace selection
 */
export function computeMouseDownSelection(
  currentSelection: ReadonlySet<UID>,
  clickedUid: UID,
  isMultiSelect: boolean,
): MouseDownSelectionResult {
  if (isMultiSelect) {
    if (currentSelection.has(clickedUid)) {
      return { newSelection: setDelete(currentSelection, clickedUid), deferSingleSelect: undefined };
    } else {
      return { newSelection: setAdd(currentSelection, clickedUid), deferSingleSelect: undefined };
    }
  }

  // No modifier key
  if (currentSelection.has(clickedUid)) {
    // Element is already selected -- defer to mouseUp so group drag works
    return { newSelection: undefined, deferSingleSelect: clickedUid };
  }

  // Element not in current selection -- select it immediately
  return { newSelection: new Set([clickedUid]), deferSingleSelect: undefined };
}

/**
 * When cloud re-attachment is activated (clicking a cloud triggers flow
 * source/sink drag mode), the selection must contain the flow UID -- not
 * the cloud UID. Downstream mouseUp handlers read `only(selection)` and
 * expect a FlowViewElement for attachment handling.
 */
export function resolveSelectionForReattachment(
  newSelection: ReadonlySet<UID>,
  enteredReattachmentMode: boolean,
  reattachFlowUid: UID,
): ReadonlySet<UID> {
  if (enteredReattachmentMode) {
    return new Set([reattachFlowUid]);
  }
  return newSelection;
}

/**
 * State fields that must be cleared when pointer interactions end. Used by
 * clearPointerState and the deferred-click early-return for non-named
 * elements (clouds) to ensure no stale pointer state leaks into subsequent
 * renders or interactions.
 *
 * Post tagged-union migration (#65): the eight former boolean modes plus the
 * loose labelSide / flowStillBeingCreated / draggingSegmentIndex fields all
 * collapse into a single `interaction: idle`. (Notably, `editingName.creatingFlow`
 * -- formerly `flowStillBeingCreated` -- is reset here so that once name editing
 * ends, a later Escape-cancel of an unrelated rename can't see a stale `true`
 * and delete that variable via the cancel-of-newly-created-flow path.) Only the
 * continuous companions that travel alongside the discrete mode are also
 * cleared; the Slate `editingName` value and `movingCanvasOffset` are
 * intentionally left untouched, matching the pre-migration reset.
 */
export interface PointerStateReset {
  interaction: InteractionState;
  moveDelta: undefined;
  dragSelectionPoint: undefined;
  inCreation: undefined;
  inCreationCloud: undefined;
}

export function pointerStateReset(): PointerStateReset {
  return {
    interaction: { mode: 'idle' },
    moveDelta: undefined,
    dragSelectionPoint: undefined,
    inCreation: undefined,
    inCreationCloud: undefined,
  };
}

/**
 * Resolves deferred selection on mouseUp.
 *
 * If a deferred UID was set on mouseDown and no drag occurred,
 * collapse the selection to just that element.
 */
export function computeMouseUpSelection(deferredUid: UID | undefined, didDrag: boolean): ReadonlySet<UID> | undefined {
  if (deferredUid === undefined) {
    return undefined;
  }
  if (didDrag) {
    return undefined;
  }
  return new Set([deferredUid]);
}
