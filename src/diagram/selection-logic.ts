// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Set } from 'immutable';

import { UID } from '@simlin/core/datamodel';

export interface MouseDownSelectionResult {
  newSelection: Set<UID> | undefined;
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
  currentSelection: Set<UID>,
  clickedUid: UID,
  isMultiSelect: boolean,
): MouseDownSelectionResult {
  if (isMultiSelect) {
    if (currentSelection.has(clickedUid)) {
      return { newSelection: currentSelection.delete(clickedUid), deferSingleSelect: undefined };
    } else {
      return { newSelection: currentSelection.add(clickedUid), deferSingleSelect: undefined };
    }
  }

  // No modifier key
  if (currentSelection.has(clickedUid)) {
    // Element is already selected -- defer to mouseUp so group drag works
    return { newSelection: undefined, deferSingleSelect: clickedUid };
  }

  // Element not in current selection -- select it immediately
  return { newSelection: Set([clickedUid]), deferSingleSelect: undefined };
}

/**
 * Resolves deferred selection on mouseUp.
 *
 * If a deferred UID was set on mouseDown and no drag occurred,
 * collapse the selection to just that element.
 */
export function computeMouseUpSelection(deferredUid: UID | undefined, didDrag: boolean): Set<UID> | undefined {
  if (deferredUid === undefined) {
    return undefined;
  }
  if (didDrag) {
    return undefined;
  }
  return Set([deferredUid]);
}
