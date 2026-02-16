// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  linearScale,
  invertLinearScale,
  niceAxisTicks,
  computeYAxisWidth,
  findNearestPointIndex,
  formatTickLabel,
} from '../chart-utils';

describe('linearScale', () => {
  test('maps domain endpoints to range endpoints', () => {
    const scale = linearScale([0, 100], [0, 500]);
    expect(scale(0)).toBe(0);
    expect(scale(100)).toBe(500);
  });

  test('maps interior values proportionally', () => {
    const scale = linearScale([0, 10], [0, 100]);
    expect(scale(5)).toBe(50);
    expect(scale(2.5)).toBe(25);
  });

  test('handles negative domain', () => {
    const scale = linearScale([-10, 10], [0, 200]);
    expect(scale(-10)).toBe(0);
    expect(scale(0)).toBe(100);
    expect(scale(10)).toBe(200);
  });

  test('handles inverted range (y-axis: top is smaller pixel value)', () => {
    const scale = linearScale([0, 100], [300, 0]);
    expect(scale(0)).toBe(300);
    expect(scale(100)).toBe(0);
    expect(scale(50)).toBe(150);
  });

  test('returns range midpoint when domain min equals max', () => {
    const scale = linearScale([5, 5], [0, 200]);
    expect(scale(5)).toBe(100);
    expect(scale(0)).toBe(100);
    expect(scale(999)).toBe(100);
  });
});

describe('invertLinearScale', () => {
  test('inverts pixel to data value', () => {
    const invert = invertLinearScale([0, 100], [0, 500]);
    expect(invert(0)).toBe(0);
    expect(invert(500)).toBe(100);
    expect(invert(250)).toBe(50);
  });

  test('inverts with negative domain', () => {
    const invert = invertLinearScale([-10, 10], [0, 200]);
    expect(invert(0)).toBe(-10);
    expect(invert(100)).toBe(0);
    expect(invert(200)).toBe(10);
  });

  test('inverts inverted range', () => {
    const invert = invertLinearScale([0, 100], [300, 0]);
    expect(invert(300)).toBe(0);
    expect(invert(0)).toBe(100);
    expect(invert(150)).toBe(50);
  });

  test('returns domain midpoint when range min equals max', () => {
    const invert = invertLinearScale([0, 100], [50, 50]);
    expect(invert(50)).toBe(50);
    expect(invert(999)).toBe(50);
  });
});

describe('niceAxisTicks', () => {
  test('generates ticks spanning the range', () => {
    const ticks = niceAxisTicks(0, 100);
    expect(ticks.length).toBeGreaterThanOrEqual(3);
    expect(ticks[0]).toBeLessThanOrEqual(0);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(100);
  });

  test('generates nice round numbers', () => {
    const ticks = niceAxisTicks(0, 1);
    for (const t of ticks) {
      // all ticks should be finite numbers
      expect(Number.isFinite(t)).toBe(true);
    }
    // should include 0 and 1
    expect(ticks).toContain(0);
    expect(ticks).toContain(1);
  });

  test('handles negative ranges', () => {
    const ticks = niceAxisTicks(-50, 50);
    expect(ticks[0]).toBeLessThanOrEqual(-50);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(50);
    expect(ticks).toContain(0);
  });

  test('handles min === max by expanding', () => {
    const ticks = niceAxisTicks(5, 5);
    expect(ticks.length).toBeGreaterThanOrEqual(3);
    expect(ticks[0]).toBeLessThanOrEqual(4);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(6);
  });

  test('handles small fractional ranges', () => {
    const ticks = niceAxisTicks(0, 0.1);
    expect(ticks.length).toBeGreaterThanOrEqual(3);
    expect(ticks[0]).toBeLessThanOrEqual(0);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(0.1);
  });

  test('handles large ranges', () => {
    const ticks = niceAxisTicks(0, 10000);
    expect(ticks.length).toBeGreaterThanOrEqual(3);
    expect(ticks[0]).toBeLessThanOrEqual(0);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(10000);
  });

  test('ticks are evenly spaced', () => {
    const ticks = niceAxisTicks(0, 100);
    if (ticks.length >= 3) {
      const step = ticks[1] - ticks[0];
      for (let i = 2; i < ticks.length; i++) {
        expect(ticks[i] - ticks[i - 1]).toBeCloseTo(step, 10);
      }
    }
  });
});

describe('computeYAxisWidth', () => {
  test('returns minimum width for small labels', () => {
    const width = computeYAxisWidth([0, 1]);
    expect(width).toBeGreaterThanOrEqual(40);
  });

  test('returns wider width for large labels', () => {
    const widthSmall = computeYAxisWidth([0, 1]);
    const widthLarge = computeYAxisWidth([0, 100000]);
    expect(widthLarge).toBeGreaterThan(widthSmall);
  });

  test('handles negative numbers', () => {
    const width = computeYAxisWidth([-1000, 0, 1000]);
    // negative sign takes space
    expect(width).toBeGreaterThanOrEqual(40);
  });

  test('handles empty array', () => {
    const width = computeYAxisWidth([]);
    expect(width).toBeGreaterThanOrEqual(40);
  });
});

describe('findNearestPointIndex', () => {
  test('finds exact match', () => {
    const points = [{ x: 0 }, { x: 1 }, { x: 2 }, { x: 3 }];
    expect(findNearestPointIndex(points, 2)).toBe(2);
  });

  test('finds nearest point when between values', () => {
    const points = [{ x: 0 }, { x: 1 }, { x: 2 }, { x: 3 }];
    expect(findNearestPointIndex(points, 1.3)).toBe(1);
    expect(findNearestPointIndex(points, 1.7)).toBe(2);
  });

  test('returns 0 for value before first point', () => {
    const points = [{ x: 1 }, { x: 2 }, { x: 3 }];
    expect(findNearestPointIndex(points, -5)).toBe(0);
  });

  test('returns last index for value after last point', () => {
    const points = [{ x: 1 }, { x: 2 }, { x: 3 }];
    expect(findNearestPointIndex(points, 100)).toBe(2);
  });

  test('handles single element array', () => {
    const points = [{ x: 5 }];
    expect(findNearestPointIndex(points, 5)).toBe(0);
    expect(findNearestPointIndex(points, 0)).toBe(0);
    expect(findNearestPointIndex(points, 100)).toBe(0);
  });

  test('returns -1 for empty array', () => {
    expect(findNearestPointIndex([], 5)).toBe(-1);
  });

  test('handles midpoint between two points', () => {
    const points = [{ x: 0 }, { x: 10 }];
    // at exact midpoint, either 0 or 1 is acceptable
    const idx = findNearestPointIndex(points, 5);
    expect(idx === 0 || idx === 1).toBe(true);
  });

  test('works with floating point values', () => {
    const points = [{ x: 0.1 }, { x: 0.2 }, { x: 0.3 }, { x: 0.4 }];
    expect(findNearestPointIndex(points, 0.25)).toBe(1); // closer to 0.2
    expect(findNearestPointIndex(points, 0.35)).toBe(2); // closer to 0.3 (actually 0.35 is equidistant but we accept either)
  });
});

describe('formatTickLabel', () => {
  test('formats integers without decimal point', () => {
    expect(formatTickLabel(0)).toBe('0');
    expect(formatTickLabel(1)).toBe('1');
    expect(formatTickLabel(100)).toBe('100');
    expect(formatTickLabel(-5)).toBe('-5');
  });

  test('formats decimals with minimal trailing zeros', () => {
    expect(formatTickLabel(0.5)).toBe('0.5');
    expect(formatTickLabel(1.25)).toBe('1.25');
  });

  test('strips trailing zeros', () => {
    expect(formatTickLabel(1.1)).toBe('1.1');
    expect(formatTickLabel(2.0)).toBe('2');
  });

  test('handles large numbers', () => {
    expect(formatTickLabel(10000)).toBe('10000');
  });

  test('handles small decimals', () => {
    expect(formatTickLabel(0.001)).toBe('0.001');
  });
});
