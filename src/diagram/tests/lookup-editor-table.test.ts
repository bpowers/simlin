// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { xAtTableIndex } from '../LookupEditor';

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
