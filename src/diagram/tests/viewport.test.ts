// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Unit tests for the pure viewport math (drawing/viewport.ts). No jsdom: every
// function takes already-resolved canvas-space numbers, so the behavior of
// pan/zoom/pinch/momentum is exercised here without the DOM.

import { describe, it, expect } from '@rstest/core';

import {
  MAX_ZOOM,
  MIN_ZOOM,
  OFFSCREEN_VISIBLE_FRACTION,
  PINCH_ZOOM_DIVISOR,
  VELOCITY_THRESHOLD,
  calculateVelocity,
  centerOffsetForBounds,
  clampZoom,
  frictionPosition,
  frictionVelocity,
  isDiagramOffscreen,
  isMomentumDone,
  momentumOffsetAt,
  pinchOffset,
  pinchZoom,
  resizeViewBox,
  visibleDiagramFraction,
  wheelPanOffset,
  wheelZoom,
  zoomAroundPoint,
} from '../drawing/viewport';
import type { Rect } from '../drawing/common';

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

describe('visibleDiagramFraction / isDiagramOffscreen (issue #52)', () => {
  // A 100x100 model-space box at the origin, on a 1000x1000 canvas at zoom 1.
  // With those defaults, offset.x = -N shifts the box's left edge to screen -N,
  // so the visible width is trivially (100 - N) for 0 <= N <= 100.
  const box: Rect = { left: 0, top: 0, right: 100, bottom: 100 };
  const svg = { width: 1000, height: 1000 };

  const cases: ReadonlyArray<{ name: string; offsetX: number; fraction: number; offscreen: boolean }> = [
    { name: 'fully visible (centered at origin)', offsetX: 0, fraction: 1, offscreen: false },
    { name: 'mostly visible (50%)', offsetX: -50, fraction: 0.5, offscreen: false },
    { name: 'mostly offscreen (9%)', offsetX: -91, fraction: 0.09, offscreen: true },
    { name: 'exactly at threshold (10%)', offsetX: -90, fraction: 0.1, offscreen: false },
  ];

  for (const c of cases) {
    it(`${c.name}: fraction ${c.fraction}, offscreen=${c.offscreen}`, () => {
      const offset = { x: c.offsetX, y: 0 };
      expect(visibleDiagramFraction(box, offset, 1, svg)).toBeCloseTo(c.fraction, 6);
      expect(isDiagramOffscreen(box, offset, 1, svg)).toBe(c.offscreen);
    });
  }

  it('reports zero visible fraction for a box entirely off the canvas', () => {
    const offset = { x: -5000, y: -5000 };
    expect(visibleDiagramFraction(box, offset, 1, svg)).toBe(0);
    expect(isDiagramOffscreen(box, offset, 1, svg)).toBe(true);
  });

  it('treats an empty (zero-area) box as fully visible and never offscreen', () => {
    const emptyBox: Rect = { left: 0, top: 0, right: 0, bottom: 0 };
    // Even with an absurd offset, an empty model must not trigger a re-center.
    expect(visibleDiagramFraction(emptyBox, { x: -9999, y: -9999 }, 1, svg)).toBe(1);
    expect(isDiagramOffscreen(emptyBox, { x: -9999, y: -9999 }, 1, svg)).toBe(false);
  });

  it('never reports offscreen when the canvas has not been measured (zero size)', () => {
    // A far-offscreen box, but a 0x0 viewport means there is nothing to center
    // against yet -- the shell waits for the first real measurement.
    expect(isDiagramOffscreen(box, { x: -5000, y: -5000 }, 1, { width: 0, height: 0 })).toBe(false);
  });

  it('is computed in screen space: clipping against the fixed viewport depends on zoom', () => {
    // The overlap is a screen-space ratio, so it is zoom-dependent by design: at
    // zoom 2 the same model box covers twice the pixels, so a given model-space
    // shift leaves more of it visible relative to the fixed 1000px viewport.
    // zoom 1, offset -50: screen [-50,50] over box width 100 -> 50% visible.
    expect(visibleDiagramFraction(box, { x: -50, y: 0 }, 1, svg)).toBeCloseTo(0.5, 6);
    // zoom 2, offset -25: screen [-50,150] over box width 200 -> 150/200 visible.
    expect(visibleDiagramFraction(box, { x: -25, y: 0 }, 2, svg)).toBeCloseTo(0.75, 6);
  });

  it('honors a caller-supplied threshold', () => {
    const offset = { x: -70, y: 0 }; // 30% visible
    expect(visibleDiagramFraction(box, offset, 1, svg)).toBeCloseTo(0.3, 6);
    expect(isDiagramOffscreen(box, offset, 1, svg, 0.5)).toBe(true);
    expect(isDiagramOffscreen(box, offset, 1, svg, 0.2)).toBe(false);
  });

  it('exposes a 10% default threshold', () => {
    expect(OFFSCREEN_VISIBLE_FRACTION).toBe(0.1);
  });

  // A model whose on-screen bbox is LARGER than the viewport: normalizing by the
  // box area alone would report a well-framed large model as mostly hidden and
  // yank it to bbox-center on every open, permanently discarding the user's saved
  // framing for exactly the large models where it matters most. The metric
  // normalizes by min(boxArea, viewportArea) so "viewport full of diagram" reads
  // 1.0 regardless of bbox size.
  describe('big-box regime (bbox larger than the viewport)', () => {
    const bigBox: Rect = { left: 0, top: 0, right: 3500, bottom: 3000 };
    const smallViewport = { width: 1200, height: 800 };

    it('a perfectly-centered big model is NOT offscreen', () => {
      // Center the bbox (center 1750,1500) in the 1200x800 viewport at zoom 1.
      const offset = centerOffsetForBounds(bigBox, 1, smallViewport);
      // The viewport is entirely inside the bbox -> the whole viewport is filled
      // with diagram -> fraction 1.0 under min-normalization.
      expect(visibleDiagramFraction(bigBox, offset, 1, smallViewport)).toBeCloseTo(1, 6);
      expect(isDiagramOffscreen(bigBox, offset, 1, smallViewport)).toBe(false);
      // Guard against a box-area-normalized regression: it would read ~9% and
      // (wrongly) trip the 10% threshold.
      const boxAreaFraction =
        (smallViewport.width * smallViewport.height) / ((bigBox.right - bigBox.left) * (bigBox.bottom - bigBox.top));
      expect(boxAreaFraction).toBeLessThan(0.1);
    });

    it('a corner-framed big model (viewport wholly inside the bbox) is NOT offscreen', () => {
      // Offset so the viewport sits near a corner of the model but is still
      // entirely covered by diagram.
      const offset = { x: -100, y: -100 };
      expect(visibleDiagramFraction(bigBox, offset, 1, smallViewport)).toBeCloseTo(1, 6);
      expect(isDiagramOffscreen(bigBox, offset, 1, smallViewport)).toBe(false);
    });

    it('a big model with only a sliver in view IS offscreen', () => {
      // Pan so the bbox's left edge is at screen 1140: only a 60px-wide strip of
      // the 1200px viewport shows diagram. 60*800 / min(bigBoxArea, 1200*800)
      // = 48000 / 960000 = 5% < 10%.
      const offset = { x: 1140, y: -100 };
      expect(visibleDiagramFraction(bigBox, offset, 1, smallViewport)).toBeCloseTo(0.05, 6);
      expect(isDiagramOffscreen(bigBox, offset, 1, smallViewport)).toBe(true);
    });

    it('a big model panned fully away IS offscreen', () => {
      const offset = { x: -10000, y: -10000 };
      expect(visibleDiagramFraction(bigBox, offset, 1, smallViewport)).toBe(0);
      expect(isDiagramOffscreen(bigBox, offset, 1, smallViewport)).toBe(true);
    });

    it('a half-covered viewport reads 0.5 (viewport-area normalized)', () => {
      // bbox left edge at screen 600 (viewport midpoint), full height covered:
      // intersection 600x800 over the 1200x800 viewport = 0.5.
      const offset = { x: 600, y: -100 };
      expect(visibleDiagramFraction(bigBox, offset, 1, smallViewport)).toBeCloseTo(0.5, 6);
      expect(isDiagramOffscreen(bigBox, offset, 1, smallViewport)).toBe(false);
    });
  });
});

describe('centerOffsetForBounds (issue #52)', () => {
  const box: Rect = { left: 0, top: 0, right: 100, bottom: 100 };
  const svg = { width: 1000, height: 1000 };

  it('places the box center at the middle of the viewport at zoom 1', () => {
    const offset = centerOffsetForBounds(box, 1, svg);
    expect(offset).toEqual({ x: 450, y: 450 });
    // Verify: box center (50,50) maps to the screen center (500,500).
    expect((50 + offset.x) * 1).toBeCloseTo(500, 6);
    expect((50 + offset.y) * 1).toBeCloseTo(500, 6);
  });

  it('accounts for zoom without changing it', () => {
    const offset = centerOffsetForBounds(box, 2, svg);
    expect((50 + offset.x) * 2).toBeCloseTo(500, 6);
    expect((50 + offset.y) * 2).toBeCloseTo(500, 6);
  });

  it('centers an off-origin box', () => {
    const shifted: Rect = { left: 200, top: 400, right: 300, bottom: 500 };
    const offset = centerOffsetForBounds(shifted, 1, svg);
    // Center (250, 450) must land at the viewport center (500, 500).
    expect((250 + offset.x) * 1).toBeCloseTo(500, 6);
    expect((450 + offset.y) * 1).toBeCloseTo(500, 6);
  });
});
