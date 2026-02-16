// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Creates a linear mapping function from data domain to pixel range.
 * When domain[0] === domain[1], returns the range midpoint for all inputs.
 */
export function linearScale(domain: [number, number], range: [number, number]): (value: number) => number {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  const domainSpan = d1 - d0;
  if (domainSpan === 0) {
    const mid = (r0 + r1) / 2;
    return () => mid;
  }
  const scale = (r1 - r0) / domainSpan;
  return (value: number) => r0 + (value - d0) * scale;
}

/**
 * Creates an inverse linear mapping function from pixel range back to data domain.
 * When range[0] === range[1], returns the domain midpoint for all inputs.
 */
export function invertLinearScale(domain: [number, number], range: [number, number]): (pixel: number) => number {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  const rangeSpan = r1 - r0;
  if (rangeSpan === 0) {
    const mid = (d0 + d1) / 2;
    return () => mid;
  }
  const scale = (d1 - d0) / rangeSpan;
  return (pixel: number) => d0 + (pixel - r0) * scale;
}

/**
 * Picks a "nice" step size (1, 2, 2.5, or 5 times a power of 10) that yields
 * approximately `approxCount` ticks for the given range.
 */
function niceStep(range: number, approxCount: number): number {
  const rawStep = range / approxCount;
  const magnitude = Math.pow(10, Math.floor(Math.log10(rawStep)));
  const normalized = rawStep / magnitude;
  let niceNormalized: number;
  if (normalized <= 1) {
    niceNormalized = 1;
  } else if (normalized <= 2) {
    niceNormalized = 2;
  } else if (normalized <= 2.5) {
    niceNormalized = 2.5;
  } else if (normalized <= 5) {
    niceNormalized = 5;
  } else {
    niceNormalized = 10;
  }
  return niceNormalized * magnitude;
}

/**
 * Generates evenly-spaced "nice" tick values spanning [min, max].
 * When min === max, expands to [min-1, min, min+1].
 */
export function niceAxisTicks(min: number, max: number, approxCount = 5): number[] {
  if (min === max) {
    min = min - 1;
    max = max + 1;
  }

  const step = niceStep(max - min, approxCount);
  const start = Math.floor(min / step) * step;
  const end = Math.ceil(max / step) * step;

  const ticks: number[] = [];
  // Use a count-based loop to avoid floating point accumulation issues
  const count = Math.round((end - start) / step);
  for (let i = 0; i <= count; i++) {
    const tick = start + i * step;
    // Round to eliminate floating point noise
    ticks.push(parseFloat(tick.toPrecision(12)));
  }
  return ticks;
}

/**
 * Estimates pixel width needed for Y-axis labels based on the formatted
 * length of the longest tick label.
 */
export function computeYAxisWidth(ticks: number[]): number {
  let maxLen = 0;
  for (const t of ticks) {
    const len = formatTickLabel(t).length;
    if (len > maxLen) {
      maxLen = len;
    }
  }
  return Math.max(40, 12 + maxLen * 7);
}

/**
 * Binary search for the index of the nearest point by x value.
 * Precondition: points are sorted by x ascending.
 * Returns -1 for empty arrays.
 */
export function findNearestPointIndex(points: ReadonlyArray<{ x: number }>, xValue: number): number {
  const n = points.length;
  if (n === 0) return -1;
  if (n === 1) return 0;

  // edge cases: before first or after last
  if (xValue <= points[0].x) return 0;
  if (xValue >= points[n - 1].x) return n - 1;

  // binary search for the insertion point
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (points[mid].x < xValue) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }

  // lo is the first index with points[lo].x >= xValue
  // compare with lo-1 to find which is closer
  if (lo === 0) return 0;
  const dLo = Math.abs(points[lo].x - xValue);
  const dPrev = Math.abs(points[lo - 1].x - xValue);
  return dPrev <= dLo ? lo - 1 : lo;
}

/**
 * Formats a numeric tick label: integers as integers, decimals with
 * minimal trailing zeros.
 */
export function formatTickLabel(value: number): string {
  if (Number.isInteger(value)) {
    return value.toString();
  }
  // Use toPrecision to get significant digits, then strip trailing zeros
  // For most axis ticks, toString() already does minimal formatting
  return parseFloat(value.toPrecision(12)).toString();
}
