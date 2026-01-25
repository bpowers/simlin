// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type { Point } from './common';

/**
 * Determines whether to show variable details panel after a pointer interaction.
 *
 * We only want to show details on a pure click on an element body - not when
 * dragging elements, clicking arrowheads/sources (which are for repositioning),
 * or clicking on empty canvas.
 */
export function shouldShowVariableDetails(
  hadSelection: boolean,
  moveDelta: Point | undefined,
  isMovingArrow: boolean,
  isMovingSource: boolean,
  isMovingLabel: boolean,
): boolean {
  const hadMovement = moveDelta !== undefined && (moveDelta.x !== 0 || moveDelta.y !== 0);
  if (!hadSelection) return false;
  if (hadMovement) return false;
  if (isMovingArrow || isMovingSource) return false;
  if (isMovingLabel) return false;
  return true;
}
