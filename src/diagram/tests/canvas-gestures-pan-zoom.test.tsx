/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Reconciler-level gesture tests for canvas pan and pinch-zoom of the React
// `Canvas` (Piece 1a; see
// docs/design-plans/2026-06-07-canvas-interaction-migration.md). The gestures
// end stationary so the momentum rAF loop does not fire (calculateVelocity
// returns zero once a frame is >40ms old, and only velocities above
// VELOCITY_THRESHOLD start momentum); each test asserts on the LAST
// onViewBoxChange call (the gesture's committed viewBox), tolerating the single
// mount-time fit call that clearMountCalls already drops.

import { makeAux, pointerDown, pointerMove, pointerUp, renderCanvas } from './canvas-gesture-harness';

interface ViewBoxCall {
  x: number;
  y: number;
  zoom: number;
}

function lastViewBox(fn: jest.Mock): ViewBoxCall | undefined {
  const calls = fn.mock.calls;
  const last = calls[calls.length - 1];
  if (!last) {
    return undefined;
  }
  return { x: last[0].x, y: last[0].y, zoom: last[1] };
}

describe('Canvas gestures: pan (checklist 3)', () => {
  it('shift-drag with a mouse pans the viewBox and does NOT clear the selection', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    pointerDown(h.svg, 500, 500, { shiftKey: true });
    pointerMove(h.svg, 530, 540, { shiftKey: true, buttons: 1 });
    pointerUp(h.svg, 530, 540, { shiftKey: true });

    // newOffset = viewBox(0,0) + (curr - mouseDown) = (30, 40); zoom unchanged.
    expect(lastViewBox(h.callbacks.onViewBoxChange)).toEqual({ x: 30, y: 40, zoom: 1 });
    // A pan must not clear the selection (handlePointerCancel uses
    // clearSelection = !isMovingCanvas, ~line 1243).
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
  });

  it('a single-finger touch drag pans the viewBox and preserves the selection', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    pointerDown(h.svg, 500, 500, { pointerType: 'touch', isPrimary: true });
    pointerMove(h.svg, 540, 560, { pointerType: 'touch', isPrimary: true, buttons: 1 });
    pointerUp(h.svg, 540, 560, { pointerType: 'touch', isPrimary: true });

    expect(lastViewBox(h.callbacks.onViewBoxChange)).toEqual({ x: 40, y: 60, zoom: 1 });
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
  });
});

describe('Canvas gestures: pinch (checklist 14)', () => {
  it('a second touch enters pinch mode and a pinch-apart zooms the viewBox in', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    // First finger starts a single-finger pan; the second finger must clear that
    // and switch to pinch (handlePointerDown's activePointers.size === 2 branch).
    pointerDown(h.svg, 100, 100, { pointerId: 1, pointerType: 'touch', isPrimary: true });
    pointerDown(h.svg, 200, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });

    // Spread the fingers: distance 100 -> 200, scale 2 -> zoom 1 -> 2.
    pointerMove(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false, buttons: 1 });

    const zoomed = lastViewBox(h.callbacks.onViewBoxChange);
    expect(zoomed).toBeDefined();
    // Pinch changed the zoom (only pinch/wheel do; pan keeps zoom == 1).
    expect(zoomed!.zoom).toBeCloseTo(2, 5);
  });

  it('pointer-up exits pinch cleanly so a subsequent single-finger pan works', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 100, { pointerId: 1, pointerType: 'touch', isPrimary: true });
    pointerDown(h.svg, 200, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });
    pointerMove(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false, buttons: 1 });
    pointerUp(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });

    // A fresh single-finger gesture after the pinch must pan (no stuck pinch).
    h.callbacks.onViewBoxChange.mockClear();
    pointerDown(h.svg, 100, 100, { pointerId: 3, pointerType: 'touch', isPrimary: true });
    pointerMove(h.svg, 130, 130, { pointerId: 3, pointerType: 'touch', isPrimary: true, buttons: 1 });
    pointerUp(h.svg, 130, 130, { pointerId: 3, pointerType: 'touch', isPrimary: true });

    expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
  });
});
