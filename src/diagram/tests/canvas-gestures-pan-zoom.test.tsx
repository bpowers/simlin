/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Reconciler-level gesture tests for canvas pan, pinch-zoom, and momentum of the
// React `Canvas` (Piece 1a; see
// docs/design-plans/2026-06-07-canvas-interaction-migration.md). Issue #707 made
// the viewport fully local during a gesture and commits to the controller
// (`onViewBoxChange`) exactly ONCE, on settle. These tests therefore assert two
// things: the committed viewBox at settle, and -- via the rendered `<g>`
// transform -- that the view updates live BEFORE any commit.
//
// Timing is made deterministic with `installFakeClock`: velocity estimation and
// the momentum rAF loop both read the clock, so a "stationary" release is modeled
// by ticking past the 40ms stop window before pointer-up, and a flick by
// releasing immediately after a fast move.

import {
  dispatchWheel,
  installFakeClock,
  makeAux,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
} from './canvas-gesture-harness';

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

// Parse the translate (e, f) out of `matrix(z 0 0 z e f)`. With zoom 1 the
// translate IS the live offset, so this reads the on-screen viewport directly.
function translate(transform: string | null): { x: number; y: number; zoom: number } {
  const m = /matrix\(([^)]+)\)/.exec(transform ?? '');
  if (!m) {
    throw new Error(`no matrix in transform: ${transform}`);
  }
  const [a, , , , e, f] = m[1].split(/[\s,]+/).map(Number);
  // matrix translate is offset * zoom, so divide back out to recover the offset.
  return { x: e / a, y: f / a, zoom: a };
}

describe('Canvas gestures: pan (checklist 3)', () => {
  it('shift-drag with a mouse pans the viewBox and does NOT clear the selection', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      pointerDown(h.svg, 500, 500, { shiftKey: true });
      clock.tick(10);
      pointerMove(h.svg, 530, 540, { shiftKey: true, buttons: 1 });
      // Hold past the 40ms stop window so the release starts no momentum: the
      // pan commits immediately, exactly once.
      clock.tick(50);
      pointerUp(h.svg, 530, 540, { shiftKey: true });

      // newOffset = viewBox(0,0) + (curr - mouseDown) = (30, 40); zoom unchanged.
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(lastViewBox(h.callbacks.onViewBoxChange)).toEqual({ x: 30, y: 40, zoom: 1 });
      // A pan must not clear the selection.
      expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
    } finally {
      clock.restore();
    }
  });

  it('updates the live transform during the drag before committing', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      pointerDown(h.svg, 500, 500, { shiftKey: true });
      clock.tick(10);
      pointerMove(h.svg, 530, 540, { shiftKey: true, buttons: 1 });

      // Mid-gesture: the diagram has visibly moved, but nothing is committed yet.
      expect(translate(h.getTransform())).toMatchObject({ x: 30, y: 40 });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
    } finally {
      clock.restore();
    }
  });

  it('a single-finger touch drag pans the viewBox and preserves the selection', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      pointerDown(h.svg, 500, 500, { pointerType: 'touch', isPrimary: true });
      clock.tick(10);
      pointerMove(h.svg, 540, 560, { pointerType: 'touch', isPrimary: true, buttons: 1 });
      clock.tick(50);
      pointerUp(h.svg, 540, 560, { pointerType: 'touch', isPrimary: true });

      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(lastViewBox(h.callbacks.onViewBoxChange)).toEqual({ x: 40, y: 60, zoom: 1 });
      expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
    } finally {
      clock.restore();
    }
  });
});

describe('Canvas gestures: momentum (issue #707)', () => {
  // A flick: fast move then immediate release (within the 40ms window) starts a
  // momentum coast. The coast must update the view live and commit exactly once.
  function flick(h: ReturnType<typeof renderCanvas>, clock: ReturnType<typeof installFakeClock>): void {
    pointerDown(h.svg, 500, 500, { pointerType: 'touch', isPrimary: true });
    clock.tick(10);
    pointerMove(h.svg, 540, 540, { pointerType: 'touch', isPrimary: true, buttons: 1 });
    clock.tick(5); // released while still moving -> momentum
    pointerUp(h.svg, 540, 540, { pointerType: 'touch', isPrimary: true });
  }

  it('defers the commit until the coast settles, then commits exactly once', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      flick(h, clock);

      // Pointer-up started a coast: no commit yet, but the view has moved to the
      // release offset (40, 40).
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      expect(translate(h.getTransform())).toMatchObject({ x: 40, y: 40 });

      // A few frames in: still coasting, still no commit, but the offset advanced
      // past the release point.
      clock.frame();
      clock.frame();
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      expect(translate(h.getTransform()).x).toBeGreaterThan(40);

      // Run the coast to its natural end: exactly one commit, at the final offset.
      clock.flush();
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(lastViewBox(h.callbacks.onViewBoxChange)!.x).toBeGreaterThan(40);
    } finally {
      clock.restore();
    }
  });

  it('a pan that interrupts a coast starts from the coasted offset, not props.view', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      flick(h, clock);
      clock.frame();
      clock.frame();
      const coasted = translate(h.getTransform());
      expect(coasted.x).toBeGreaterThan(40);

      // Press interrupts the coast (no commit), then pan by (+20, +20) screen px.
      pointerDown(h.svg, 200, 200, { pointerType: 'touch', isPrimary: true });
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      clock.tick(10);
      pointerMove(h.svg, 220, 220, { pointerType: 'touch', isPrimary: true, buttons: 1 });

      // The pan anchors at the coasted offset, so the live view is coasted + 20,
      // NOT props.view(0,0) + 20.
      const panned = translate(h.getTransform());
      expect(panned.x).toBeCloseTo(coasted.x + 20, 3);
      expect(panned.y).toBeCloseTo(coasted.y + 20, 3);

      clock.tick(50);
      pointerUp(h.svg, 220, 220, { pointerType: 'touch', isPrimary: true });
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      expect(lastViewBox(h.callbacks.onViewBoxChange)!.x).toBeCloseTo(coasted.x + 20, 3);
    } finally {
      clock.restore();
    }
  });
});

describe('Canvas gestures: embedded mode is viewport-inert (issue #707)', () => {
  it('ignores wheel and resize, never setting a live viewport or committing', () => {
    const h = renderCanvas({ embedded: true, elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    // A wheel in embedded mode is a no-op (handleNativeWheel early-returns), so
    // nothing commits and there is no content transform (embedded draws to a
    // tight viewBox attribute, leaving the content <g> with no transform).
    dispatchWheel(h.svg, { deltaX: 30, deltaY: 40 });
    expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
    expect(h.getTransform()).toBeNull();

    // Resize in embedded mode only measures; it never commits a viewBox.
    h.resize(1000, 1000);
    h.resize(800, 800);
    expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
    expect(h.getTransform()).toBeNull();
  });
});

describe('Canvas gestures: resize (issue #707)', () => {
  it('commits the re-centered viewBox immediately when no gesture is in flight', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    // First resize establishes the known size; clear so we assert on the second.
    h.resize(1000, 1000);
    h.clearMountCalls();

    h.resize(800, 800);

    // resizeViewBox((0,0), dW=-200, dH=-200, 800, 800) shifts by dW/4 = -50.
    expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
    expect(lastViewBox(h.callbacks.onViewBoxChange)).toEqual({ x: -50, y: -50, zoom: 1 });
  });

  it('leaves the offset to an active pan and commits the new size on settle', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.resize(1000, 1000); // establish svgSize before the gesture
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      // Begin a pan (not released): live viewport = (30, 40).
      pointerDown(h.svg, 500, 500, { shiftKey: true });
      clock.tick(10);
      pointerMove(h.svg, 530, 540, { shiftKey: true, buttons: 1 });
      expect(translate(h.getTransform())).toMatchObject({ x: 30, y: 40 });

      // Resize mid-pan: the pan owns the offset, so the offset does NOT shift
      // (no re-centering jump) and nothing commits. Only the measured size changes.
      h.resize(1200, 900);
      expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
      expect(translate(h.getTransform())).toMatchObject({ x: 30, y: 40 });

      // The pan continues correctly from its press-time anchor after the resize
      // (it must not be discarded or jump): another move yields (60, 80).
      clock.tick(10);
      pointerMove(h.svg, 560, 580, { shiftKey: true, buttons: 1 });
      expect(translate(h.getTransform())).toMatchObject({ x: 60, y: 80 });

      // Releasing stationary commits once, carrying the final offset AND the new
      // measured size (so view.viewBox.width/height are not left stale).
      clock.tick(50);
      pointerUp(h.svg, 560, 580, { shiftKey: true });
      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
      const committed = h.callbacks.onViewBoxChange.mock.calls[0][0];
      expect(committed).toMatchObject({ x: 60, y: 80, width: 1200, height: 900 });
    } finally {
      clock.restore();
    }
  });
});

describe('Canvas gestures: pinch (checklist 14)', () => {
  it('a pinch-apart zooms the live view in and commits once on exit', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    // First finger starts a single-finger pan; the second finger must clear that
    // and switch to pinch (handlePointerDown's activePointers.size === 2 branch).
    pointerDown(h.svg, 100, 100, { pointerId: 1, pointerType: 'touch', isPrimary: true });
    pointerDown(h.svg, 200, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });

    // Spread the fingers: distance 100 -> 200, scale 2 -> zoom 1 -> 2.
    pointerMove(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false, buttons: 1 });

    // The move zooms the live transform but does NOT commit yet.
    expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
    expect(translate(h.getTransform()).zoom).toBeCloseTo(2, 5);

    // Lifting the second finger exits pinch and commits the zoom exactly once.
    pointerUp(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });
    expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
    expect(lastViewBox(h.callbacks.onViewBoxChange)!.zoom).toBeCloseTo(2, 5);
  });

  it('pointer-up exits pinch cleanly so a subsequent single-finger pan works', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    const clock = installFakeClock();
    try {
      pointerDown(h.svg, 100, 100, { pointerId: 1, pointerType: 'touch', isPrimary: true });
      pointerDown(h.svg, 200, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });
      pointerMove(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false, buttons: 1 });
      pointerUp(h.svg, 300, 100, { pointerId: 2, pointerType: 'touch', isPrimary: false });

      // A fresh single-finger gesture after the pinch must pan (no stuck pinch).
      h.callbacks.onViewBoxChange.mockClear();
      pointerDown(h.svg, 100, 100, { pointerId: 3, pointerType: 'touch', isPrimary: true });
      clock.tick(10);
      pointerMove(h.svg, 130, 130, { pointerId: 3, pointerType: 'touch', isPrimary: true, buttons: 1 });
      clock.tick(50);
      pointerUp(h.svg, 130, 130, { pointerId: 3, pointerType: 'touch', isPrimary: true });

      expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
    } finally {
      clock.restore();
    }
  });
});
