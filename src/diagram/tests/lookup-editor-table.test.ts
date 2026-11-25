// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import type { GraphicalFunction } from '@simlin/core/datamodel';

import { resample, rescaleX, xAtTableIndex, type GFTable } from '../LookupEditor';

describe('xAtTableIndex', () => {
  it('spans [xMin, xMax] inclusively for multi-point tables', () => {
    expect(xAtTableIndex(0, 11, 0, 1)).toBeCloseTo(0);
    expect(xAtTableIndex(5, 11, 0, 1)).toBeCloseTo(0.5);
    expect(xAtTableIndex(10, 11, 0, 1)).toBeCloseTo(1);
  });

  it('maps a single-point table to xMin instead of NaN', () => {
    // i / (size - 1) is 0/0 === NaN for a 1-point table; the NaN x poisons
    // lookup()-based resampling and can be saved into the table.
    expect(xAtTableIndex(0, 1, 2, 8)).toBe(2);
    expect(Number.isNaN(xAtTableIndex(0, 1, 0, 1))).toBe(false);
  });

  it('handles offset scales', () => {
    expect(xAtTableIndex(1, 3, 10, 20)).toBeCloseTo(15);
  });
});

const spread: GraphicalFunction = {
  kind: 'continuous',
  xPoints: undefined,
  yPoints: [0, 5, 9],
  xScale: { min: 0, max: 2 },
  yScale: { min: 0, max: 10 },
};
// Drawn over more than it covers.
const ownPoints: GraphicalFunction = {
  ...spread,
  xPoints: [1, 2, 9],
  yPoints: [0, 5, 12],
  xScale: { min: 0, max: 20 },
};

function table(x: number[], y: number[]): GFTable {
  return { type: 'continuous', size: x.length, x: Float64Array.from(x), y: Float64Array.from(y) };
}

function points(t: GFTable | undefined): number[][] | undefined {
  return t && Array.from(t.x, (x, i) => [x, t.y[i]]);
}

describe('rescaleX', () => {
  it('spreads a table without x points of its own over the new scale', () => {
    const rescaled = rescaleX({ ...spread, xScale: { min: -2, max: 2 } }, table([0, 1, 2], [0, 5, 9]));
    expect(points(rescaled)).toEqual([
      [-2, 0],
      [0, 5],
      [2, 9],
    ]);
  });

  it('leaves a table with x points of its own where they are', () => {
    const given = table([1, 2, 9], [0, 5, 12]);
    expect(points(rescaleX({ ...ownPoints, xScale: { min: -2, max: 2 } }, given))).toEqual(points(given));
  });

  it('leaves the points alone while the scale is empty or reversed', () => {
    const given = table([0, 1, 2], [0, 5, 9]);
    expect(points(rescaleX({ ...spread, xScale: { min: 2, max: 2 } }, given))).toEqual(points(given));
  });
});

describe('resample', () => {
  it('is the table itself at its own point count', () => {
    const given = table([1, 2, 9], [0, 5, 12]);
    expect(resample(ownPoints, given, 3)).toBe(given);
  });

  it('reads a table without x points of its own over its x scale', () => {
    expect(points(resample(spread, table([0, 1, 2], [0, 5, 9]), 5))).toEqual([
      [0, 0],
      [0.5, 2.5],
      [1, 5],
      [1.5, 7],
      [2, 9],
    ]);
  });

  it('reads a table with x points of its own over the x they cover, not the range it is drawn over', () => {
    expect(points(resample(ownPoints, table([1, 2, 9], [0, 5, 12]), 5))).toEqual([
      [1, 0],
      [3, 6],
      [5, 8],
      [7, 10],
      [9, 12],
    ]);
  });

  it('spreads a lone point over the x scale, at its value', () => {
    const lone = { ...ownPoints, xPoints: [4], yPoints: [3] };
    expect(points(resample(lone, table([4], [3]), 3))).toEqual([
      [0, 3],
      [10, 3],
      [20, 3],
    ]);
  });

  it('has no answer over an empty range', () => {
    expect(resample({ ...spread, xScale: { min: 2, max: 2 } }, table([0, 1, 2], [0, 5, 9]), 5)).toBeUndefined();
  });
});
