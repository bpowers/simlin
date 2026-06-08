// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Unit tests for the pure viewport math (drawing/viewport.ts). No jsdom: every
// function takes already-resolved canvas-space numbers, so the behavior of
// pan/zoom/pinch/momentum is exercised here without the DOM.

import {
  MAX_ZOOM,
  MIN_ZOOM,
  PINCH_ZOOM_DIVISOR,
  VELOCITY_THRESHOLD,
  calculateVelocity,
  clampZoom,
  frictionPosition,
  frictionVelocity,
  isMomentumDone,
  momentumOffsetAt,
  pinchOffset,
  pinchZoom,
  resizeViewBox,
  wheelPanOffset,
  wheelZoom,
  zoomAroundPoint,
} from '../drawing/viewport';

describe('clampZoom', () => {
  it('clamps to the supported range and passes through in-range values', () => {
    expect(clampZoom(MIN_ZOOM - 1)).toBe(MIN_ZOOM);
    expect(clampZoom(MAX_ZOOM + 1)).toBe(MAX_ZOOM);
    expect(clampZoom(1)).toBe(1);
  });
});

describe('wheelPanOffset', () => {
  const base = { x: 100, y: 200 };
  const viewportPx = { width: 800, height: 600 };

  it('subtracts a pixel delta scaled by zoom (zoom 1)', () => {
    expect(wheelPanOffset(base, { x: 30, y: -40, mode: 0 }, 1, viewportPx)).toEqual({ x: 70, y: 240 });
  });

  it('divides the delta by zoom so higher zoom pans less in model units', () => {
    expect(wheelPanOffset(base, { x: 40, y: 0, mode: 0 }, 2, viewportPx)).toEqual({ x: 80, y: 200 });
  });

  it('resolves line deltas (mode 1) at ~16px per line', () => {
    expect(wheelPanOffset(base, { x: 1, y: 2, mode: 1 }, 1, viewportPx)).toEqual({ x: 100 - 16, y: 200 - 32 });
  });

  it('resolves page deltas (mode 2) using the viewport size', () => {
    expect(wheelPanOffset(base, { x: 1, y: -1, mode: 2 }, 1, viewportPx)).toEqual({
      x: base.x - viewportPx.width,
      y: base.y + viewportPx.height,
    });
  });
});

describe('wheelZoom', () => {
  it('halves the zoom for a +divisor deltaY and doubles for -divisor', () => {
    expect(wheelZoom(1, PINCH_ZOOM_DIVISOR).zoom).toBeCloseTo(0.5, 6);
    expect(wheelZoom(1, -PINCH_ZOOM_DIVISOR).zoom).toBeCloseTo(2, 6);
  });

  it('is symmetric: zoom in then out by equal deltas returns to start', () => {
    const inZoom = wheelZoom(1, -50).zoom;
    const out = wheelZoom(inZoom, 50).zoom;
    expect(out).toBeCloseTo(1, 6);
  });

  it('clamps and reports no change at the zoom ceiling', () => {
    const result = wheelZoom(MAX_ZOOM, -PINCH_ZOOM_DIVISOR);
    expect(result.zoom).toBe(MAX_ZOOM);
    expect(result.changed).toBe(false);
  });

  it('reports a change for an in-range step', () => {
    expect(wheelZoom(1, -10).changed).toBe(true);
  });
});

describe('zoomAroundPoint', () => {
  it('keeps the model point under the cursor fixed across a zoom change', () => {
    const oldOffset = { x: 50, y: 50 };
    // At zoom 1 the cursor sits at canvas (200, 150) -> model (150, 100).
    const cursorCanvasOld = { x: 200, y: 150 };
    // At a higher zoom the same screen pixel maps to a different canvas point.
    const cursorCanvasNew = { x: 100, y: 75 };
    const newOffset = zoomAroundPoint(oldOffset, cursorCanvasOld, cursorCanvasNew);
    // The model point under the cursor must be unchanged: cursorNew - newOffset.
    expect(cursorCanvasNew.x - newOffset.x).toBeCloseTo(cursorCanvasOld.x - oldOffset.x, 6);
    expect(cursorCanvasNew.y - newOffset.y).toBeCloseTo(cursorCanvasOld.y - oldOffset.y, 6);
  });
});

describe('pinchZoom / pinchOffset', () => {
  it('scales the initial zoom by the finger-distance ratio, clamped', () => {
    expect(pinchZoom(1, 2)).toBe(2);
    expect(pinchZoom(1, 100)).toBe(MAX_ZOOM);
    expect(pinchZoom(1, 0.01)).toBe(MIN_ZOOM);
  });

  it('is symmetric: spreading then pinching back returns to the start zoom', () => {
    expect(pinchZoom(pinchZoom(1, 2), 0.5)).toBeCloseTo(1, 6);
  });

  it('places the offset so the model point sits under the pinch center', () => {
    const center = { x: 300, y: 200 };
    const modelPoint = { x: 120, y: 80 };
    const offset = pinchOffset(center, modelPoint);
    expect(center.x - offset.x).toBeCloseTo(modelPoint.x, 6);
    expect(center.y - offset.y).toBeCloseTo(modelPoint.y, 6);
  });
});

describe('momentum friction', () => {
  it('decays velocity monotonically toward zero', () => {
    const v0 = 1000;
    const v1 = frictionVelocity(v0, 0.1);
    const v2 = frictionVelocity(v0, 0.5);
    expect(v1).toBeLessThan(v0);
    expect(v2).toBeLessThan(v1);
    expect(frictionVelocity(v0, 0)).toBe(v0);
  });

  it('accumulates displacement monotonically in the direction of travel', () => {
    const d1 = frictionPosition(1000, 0.1);
    const d2 = frictionPosition(1000, 0.5);
    expect(d1).toBeGreaterThan(0);
    expect(d2).toBeGreaterThan(d1);
    expect(frictionPosition(1000, 0)).toBeCloseTo(0, 10);
  });

  it('offsets from the start position by the decayed displacement', () => {
    const start = { x: 10, y: 20 };
    const v0 = { x: 500, y: -300 };
    const at = momentumOffsetAt(start, v0, 0.2);
    expect(at.x).toBeCloseTo(start.x + frictionPosition(v0.x, 0.2), 9);
    expect(at.y).toBeCloseTo(start.y + frictionPosition(v0.y, 0.2), 9);
  });

  it('reports done once the decayed speed drops below the threshold', () => {
    const v0 = { x: VELOCITY_THRESHOLD * 4, y: 0 };
    expect(isMomentumDone(v0, 0)).toBe(false);
    // Friction retains 5%/s, so after enough time the speed is below threshold.
    expect(isMomentumDone(v0, 2)).toBe(true);
  });
});

describe('calculateVelocity', () => {
  it('returns zero with fewer than two samples', () => {
    expect(calculateVelocity([], 100)).toEqual({ x: 0, y: 0 });
    expect(calculateVelocity([{ x: 0, y: 0, timestamp: 0 }], 100)).toEqual({ x: 0, y: 0 });
  });

  it('returns zero when the pointer was stationary (>40ms) before release', () => {
    const positions = [
      { x: 0, y: 0, timestamp: 0 },
      { x: 100, y: 0, timestamp: 50 },
    ];
    // now is 60ms after the last sample -> intentional stop.
    expect(calculateVelocity(positions, 110)).toEqual({ x: 0, y: 0 });
  });

  it('averages px/s over the recent (<100ms) samples', () => {
    const positions = [
      { x: 0, y: 0, timestamp: 0 },
      { x: 50, y: 25, timestamp: 50 },
      { x: 100, y: 50, timestamp: 100 },
    ];
    // now == 100: all samples within 100ms; 100px over 0.1s = 1000 px/s.
    expect(calculateVelocity(positions, 100)).toEqual({ x: 1000, y: 500 });
  });

  it('falls back to the last two samples when only one is recent', () => {
    const positions = [
      { x: 0, y: 0, timestamp: 0 },
      { x: 20, y: 10, timestamp: 130 },
    ];
    // now == 140: only the last sample is <100ms old, so recentPositions has 1.
    // Fallback uses the final two: 20px over 0.13s.
    const v = calculateVelocity(positions, 140);
    expect(v.x).toBeCloseTo(20 / 0.13, 6);
    expect(v.y).toBeCloseTo(10 / 0.13, 6);
  });
});

describe('resizeViewBox', () => {
  it('shifts the offset by a quarter of the size delta and adopts the new size', () => {
    expect(resizeViewBox({ x: 100, y: 200 }, 40, -20, 840, 580)).toEqual({
      x: 110,
      y: 195,
      width: 840,
      height: 580,
    });
  });
});
