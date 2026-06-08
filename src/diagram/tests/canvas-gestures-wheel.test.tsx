/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Reconciler-level tests for wheel/trackpad pan and zoom of the React `Canvas`.
// Issue #707: a wheel gesture has no native end event, so each wheel event
// updates the LOCAL live viewport (immediate render) and (re)arms a trailing
// debounce; the controller is notified (`onViewBoxChange`) exactly once, when the
// scroll settles. These tests drive native wheel events and use Jest fake timers
// to fire the debounce -- and, for the momentum-interruption case, to also drive
// the momentum rAF loop (Jest 30 fake timers fake performance.now /
// requestAnimationFrame / setTimeout together).

import { act } from '@testing-library/react';

import { dispatchWheel, makeAux, pointerDown, pointerMove, pointerUp, renderCanvas } from './canvas-gesture-harness';

function translate(transform: string | null): { x: number; y: number; zoom: number } {
  const m = /matrix\(([^)]+)\)/.exec(transform ?? '');
  if (!m) {
    throw new Error(`no matrix in transform: ${transform}`);
  }
  const [a, , , , e, f] = m[1].split(/[\s,]+/).map(Number);
  return { x: e / a, y: f / a, zoom: a };
}

describe('Canvas gestures: wheel pan (issue #707)', () => {
  it('updates the live transform per event but commits once on settle', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    jest.useFakeTimers();
    try {
      // wheelPanOffset subtracts delta/zoom from the offset (base 0,0; zoom 1).
      dispatchWheel(h.svg, { deltaX: 30, deltaY: 40 });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      expect(translate(h.getTransform())).toMatchObject({ x: -30, y: -40 });

      dispatchWheel(h.svg, { deltaX: 10, deltaY: 0 });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      expect(translate(h.getTransform())).toMatchObject({ x: -40, y: -40 });

      // Settle: one commit carrying the cumulative offset.
      act(() => {
        jest.advanceTimersByTime(200);
      });
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      const [viewBox, zoom] = h.callbacks.onViewBoxChange.mock.calls[0];
      expect(viewBox).toMatchObject({ x: -40, y: -40 });
      expect(zoom).toBe(1);
    } finally {
      jest.useRealTimers();
    }
  });

  it('re-arms the debounce on each event so a burst commits only once', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    jest.useFakeTimers();
    try {
      dispatchWheel(h.svg, { deltaX: 10, deltaY: 0 });
      act(() => {
        jest.advanceTimersByTime(150); // not yet idle
      });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      dispatchWheel(h.svg, { deltaX: 10, deltaY: 0 });
      act(() => {
        jest.advanceTimersByTime(150); // re-armed: still within the new window
      });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      act(() => {
        jest.advanceTimersByTime(60); // now idle past 200ms since the last event
      });
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(h.callbacks.onViewBoxChange.mock.calls[0][0]).toMatchObject({ x: -20, y: 0 });
    } finally {
      jest.useRealTimers();
    }
  });
});

describe('Canvas gestures: wheel zoom (issue #707)', () => {
  it('zooms around the cursor live and commits once on settle', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    jest.useFakeTimers();
    try {
      // ctrlKey wheel = trackpad pinch-zoom. deltaY -100 -> 2x zoom (clamped ok).
      dispatchWheel(h.svg, { deltaY: -100, ctrlKey: true, clientX: 0, clientY: 0 });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      expect(translate(h.getTransform()).zoom).toBeCloseTo(2, 5);

      act(() => {
        jest.advanceTimersByTime(200);
      });
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(h.callbacks.onViewBoxChange.mock.calls[0][1]).toBeCloseTo(2, 5);
    } finally {
      jest.useRealTimers();
    }
  });
});

describe('Canvas gestures: external view change overrides a live gesture (issue #707)', () => {
  it('clears the live wheel viewport and cancels the pending commit', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    jest.useFakeTimers();
    try {
      // A wheel pan sets the live viewport and arms the debounce (uncommitted).
      dispatchWheel(h.svg, { deltaX: 30, deltaY: 40 });
      expect(translate(h.getTransform())).toMatchObject({ x: -30, y: -40 });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();

      // An external viewport change arrives (e.g. centerVariable / navigation).
      h.setViewport({ x: 500, y: 600, zoom: 1 });

      // The external view wins immediately -- the live wheel offset is dropped.
      expect(translate(h.getTransform())).toMatchObject({ x: 500, y: 600, zoom: 1 });

      // ...and the pending wheel commit was cancelled, so the abandoned gesture
      // never commits a stale offset over the external view.
      act(() => {
        jest.advanceTimersByTime(200);
      });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
    } finally {
      jest.useRealTimers();
    }
  });
});

describe('Canvas gestures: wheel interrupts momentum (issue #707)', () => {
  it('continues from the coasted offset and commits once, without a stray commit', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    jest.useFakeTimers();
    try {
      // Flick to start a momentum coast (fast move, immediate release).
      pointerDown(h.svg, 500, 500, { pointerType: 'touch', isPrimary: true });
      act(() => {
        jest.advanceTimersByTime(10);
      });
      pointerMove(h.svg, 540, 540, { pointerType: 'touch', isPrimary: true, buttons: 1 });
      act(() => {
        jest.advanceTimersByTime(5);
      });
      pointerUp(h.svg, 540, 540, { pointerType: 'touch', isPrimary: true });

      // Let the coast advance a couple of frames (~16ms each under fake timers).
      act(() => {
        jest.advanceTimersByTime(32);
      });
      const coasted = translate(h.getTransform());
      expect(coasted.x).toBeGreaterThan(40);
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();

      // A wheel event interrupts the coast: no commit from the interruption, and
      // the wheel pans from the coasted offset.
      dispatchWheel(h.svg, { deltaX: 10, deltaY: 0 });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      const afterWheel = translate(h.getTransform());
      expect(afterWheel.x).toBeCloseTo(coasted.x - 10, 3);

      // Settle: exactly one commit, at the wheel-adjusted coasted offset.
      act(() => {
        jest.advanceTimersByTime(200);
      });
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(h.callbacks.onViewBoxChange.mock.calls[0][0].x).toBeCloseTo(coasted.x - 10, 3);
    } finally {
      jest.useRealTimers();
    }
  });
});
