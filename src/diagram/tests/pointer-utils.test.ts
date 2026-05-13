// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { ClickDragThresholdPx, isDragMovement, shouldShowVariableDetails } from '../drawing/pointer-utils';

describe('isDragMovement', () => {
  it('is false for an undefined delta (no pointer move at all)', () => {
    expect(isDragMovement(undefined, 1)).toBe(false);
  });

  it('is false for a zero delta', () => {
    expect(isDragMovement({ x: 0, y: 0 }, 1)).toBe(false);
  });

  it('is false for sub-threshold jitter at zoom 1', () => {
    // a click commonly wobbles a pixel or two -- not a drag
    expect(isDragMovement({ x: 1, y: 1 }, 1)).toBe(false);
    expect(isDragMovement({ x: ClickDragThresholdPx - 0.5, y: 0 }, 1)).toBe(false);
  });

  it('is true once the move reaches the threshold at zoom 1', () => {
    expect(isDragMovement({ x: ClickDragThresholdPx, y: 0 }, 1)).toBe(true);
    expect(isDragMovement({ x: 50, y: 0 }, 1)).toBe(true);
  });

  it('measures the threshold in screen pixels, not model units', () => {
    // moveDelta is in model coords (screen px / zoom). At 4x zoom a 2-unit
    // model delta is 8 screen px -- a real drag. The same 2-unit delta at
    // 0.5x zoom is only 1 screen px -- jitter.
    const delta = { x: 2, y: 0 };
    expect(isDragMovement(delta, 4)).toBe(true);
    expect(isDragMovement(delta, 0.5)).toBe(false);
  });

  it('uses Euclidean distance, not per-axis', () => {
    // each axis is below threshold but the combined move is not
    const half = (ClickDragThresholdPx / Math.SQRT2) + 0.1;
    expect(isDragMovement({ x: half, y: half }, 1)).toBe(true);
  });
});

describe('shouldShowVariableDetails', () => {
  it('returns true for a click on element body (no pointer move)', () => {
    expect(shouldShowVariableDetails(true, undefined, 1, false, false, false)).toBe(true);
  });

  it('returns true when moveDelta is zero', () => {
    expect(shouldShowVariableDetails(true, { x: 0, y: 0 }, 1, false, false, false)).toBe(true);
  });

  it('returns true when the move is only incidental click jitter', () => {
    // regression: a stock click that wobbled a pixel used to leave the
    // details panel closed (the deselect/reselect-fixes-it bug)
    expect(shouldShowVariableDetails(true, { x: 1, y: 1 }, 1, false, false, false)).toBe(true);
    expect(shouldShowVariableDetails(true, { x: 2, y: 0 }, 1, false, false, false)).toBe(true);
  });

  it('returns false when actually dragging an element', () => {
    expect(shouldShowVariableDetails(true, { x: 50, y: 50 }, 1, false, false, false)).toBe(false);
  });

  it('returns false when a small model-coord move is a real drag at high zoom', () => {
    expect(shouldShowVariableDetails(true, { x: 3, y: 0 }, 4, false, false, false)).toBe(false);
  });

  it('returns false when dragging an arrowhead', () => {
    expect(shouldShowVariableDetails(true, { x: 40, y: 20 }, 1, true, false, false)).toBe(false);
  });

  it('returns false when dragging a source', () => {
    expect(shouldShowVariableDetails(true, { x: 20, y: 40 }, 1, false, true, false)).toBe(false);
  });

  it('returns false when dragging a label', () => {
    expect(shouldShowVariableDetails(true, undefined, 1, false, false, true)).toBe(false);
  });

  it('returns false when clicking on empty canvas', () => {
    expect(shouldShowVariableDetails(false, undefined, 1, false, false, false)).toBe(false);
  });

  it('returns false when clicking on arrowhead without movement', () => {
    expect(shouldShowVariableDetails(true, undefined, 1, true, false, false)).toBe(false);
  });

  it('returns false when clicking on source without movement', () => {
    expect(shouldShowVariableDetails(true, undefined, 1, false, true, false)).toBe(false);
  });
});
