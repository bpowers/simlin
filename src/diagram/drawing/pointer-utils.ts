// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type { Point } from './common';

/**
 * A pointerdown→pointerup whose cursor wobbled less than this many *screen*
 * pixels is a click, not a drag. Physical trackpad/mouse clicks routinely
 * move a pixel or two as the button is pressed and released, and touch taps
 * move even more; treating that jitter as a drag both nudges the element and
 * (because dragging suppresses it) leaves the variable-details panel closed.
 */
export const ClickDragThresholdPx = 5;

/**
 * Whether a pointer move is far enough to count as a drag rather than the
 * incidental jitter of a click.
 *
 * `moveDelta` is in model/canvas coordinates (screen pixels divided by the
 * view zoom), so we multiply by `zoom` to compare against a fixed
 * screen-pixel threshold: the user moves a finger/mouse in screen space, so
 * the same model-coord delta should count as a drag when zoomed in and as
 * jitter when zoomed far out.
 */
export function isDragMovement(moveDelta: Point | undefined, zoom: number): boolean {
  if (moveDelta === undefined) {
    return false;
  }
  return Math.hypot(moveDelta.x, moveDelta.y) * zoom >= ClickDragThresholdPx;
}

/**
 * Determines whether to show variable details panel after a pointer interaction.
 *
 * We only want to show details on a pure click on an element body - not when
 * dragging elements, clicking arrowheads/sources (which are for repositioning),
 * or clicking on empty canvas. A sub-threshold pointer wobble during a click
 * still counts as a click (see `isDragMovement`).
 */
export function shouldShowVariableDetails(
  hadSelection: boolean,
  moveDelta: Point | undefined,
  zoom: number,
  isMovingArrow: boolean,
  isMovingSource: boolean,
  isMovingLabel: boolean,
): boolean {
  if (!hadSelection) return false;
  if (isDragMovement(moveDelta, zoom)) return false;
  if (isMovingArrow || isMovingSource) return false;
  if (isMovingLabel) return false;
  return true;
}
