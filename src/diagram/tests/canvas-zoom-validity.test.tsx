// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A stored view zoom is a FACTOR (1.0 = 100%). Gestures keep it inside
// [MIN_ZOOM, MAX_ZOOM], but a value loaded from data can be anything -- an
// unset 0, or a percentage recorded where a factor belongs (an XMILE
// `zoom="200"` passed through unconverted renders labels tens of thousands of
// pixels wide and a blank canvas). These tests drive a real <Canvas> through
// the gesture harness and assert on the committed viewBox/zoom and the
// rendered <g> transform, never on Canvas internals; the predicate itself is
// covered in viewport.test.ts.

import { describe, it, expect } from '@rstest/core';
import type { Mock } from '@rstest/core';

import { MAX_ZOOM } from '../drawing/viewport';
import { renderCanvas, makeAux, makeStock } from './canvas-gesture-harness';

function lastCommittedZoom(onViewBoxChange: Mock): number {
  const calls = onViewBoxChange.mock.calls;
  expect(calls.length).toBeGreaterThan(0);
  return calls[calls.length - 1][1] as number;
}

// Parse the uniform scale out of `matrix(z 0 0 z e f)`.
function renderedZoom(transform: string | null): number {
  const m = /matrix\(([^)]+)\)/.exec(transform ?? '');
  if (!m) {
    throw new Error(`no matrix in transform: ${transform}`);
  }
  return Number(m[1].split(/[\s,]+/)[0]);
}

describe('Canvas: stored zoom validity', () => {
  it('a stored zoom of 2 (a real 200% factor) is honored on mount and drawn at 2x', () => {
    const elements = [makeAux(1, 'a', 100, 100), makeStock(2, 'b', 220, 160)];
    const h = renderCanvas({ elements, zoom: 2 });
    h.resize(1000, 1000);

    expect(lastCommittedZoom(h.callbacks.onViewBoxChange)).toBe(2);
    expect(renderedZoom(h.getTransform())).toBeCloseTo(2, 5);

    // Elements land at plausible screen coordinates: inside the 1000x1000
    // canvas rather than tens of thousands of pixels away.
    const [viewBox, zoom] = h.callbacks.onViewBoxChange.mock.calls[h.callbacks.onViewBoxChange.mock.calls.length - 1];
    for (const el of elements) {
      const screenX = (el.x + viewBox.x) * zoom;
      const screenY = (el.y + viewBox.y) * zoom;
      expect(screenX).toBeGreaterThanOrEqual(0);
      expect(screenX).toBeLessThanOrEqual(1000);
      expect(screenY).toBeGreaterThanOrEqual(0);
      expect(screenY).toBeLessThanOrEqual(1000);
    }
  });

  it('a stored zoom of 200 (an XMILE percentage in the wrong unit) is reset to 1 on mount, not drawn at 200x', () => {
    const elements = [makeAux(1, 'a', 100, 100), makeStock(2, 'b', 220, 160)];
    const h = renderCanvas({ elements, zoom: 200 });
    h.resize(1000, 1000);

    // The mount-time fit persists the healed zoom through onViewBoxChange...
    expect(lastCommittedZoom(h.callbacks.onViewBoxChange)).toBe(1);
    // ...and nothing was ever drawn at 200x.
    expect(renderedZoom(h.getTransform())).toBeCloseTo(1, 5);
  });

  it('every out-of-range stored zoom (0, negative, NaN, just above MAX_ZOOM) resets to 1', () => {
    for (const zoom of [0, -1, Number.NaN, MAX_ZOOM + 0.01]) {
      const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)], zoom });
      h.resize(1000, 1000);
      expect(lastCommittedZoom(h.callbacks.onViewBoxChange)).toBe(1);
      expect(renderedZoom(h.getTransform())).toBeCloseTo(1, 5);
      h.unmount();
    }
  });

  it('an out-of-range zoom pushed EXTERNALLY after mount is drawn at 1 (the render rule matches the mount rule)', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)] });
    h.resize(1000, 1000);
    h.clearMountCalls();

    h.setViewport({ zoom: 200 });
    expect(renderedZoom(h.getTransform())).toBeCloseTo(1, 5);

    // A valid external zoom is drawn as given.
    h.setViewport({ zoom: 2.5 });
    expect(renderedZoom(h.getTransform())).toBeCloseTo(2.5, 5);
  });
});
