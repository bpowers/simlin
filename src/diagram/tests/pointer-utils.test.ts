// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { shouldShowVariableDetails } from '../drawing/pointer-utils';

describe('shouldShowVariableDetails', () => {
  it('returns true for a click on element body', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        undefined, // moveDelta
        false, // isMovingArrow
        false, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(true);
  });

  it('returns true when moveDelta is zero', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        { x: 0, y: 0 }, // moveDelta
        false, // isMovingArrow
        false, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(true);
  });

  it('returns false when dragging an element', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        { x: 1, y: 1 }, // moveDelta
        false, // isMovingArrow
        false, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(false);
  });

  it('returns false when dragging an arrowhead', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        { x: 4, y: 2 }, // moveDelta
        true, // isMovingArrow
        false, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(false);
  });

  it('returns false when dragging a source', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        { x: 2, y: 4 }, // moveDelta
        false, // isMovingArrow
        true, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(false);
  });

  it('returns false when dragging a label', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        undefined, // moveDelta
        false, // isMovingArrow
        false, // isMovingSource
        true, // isMovingLabel
      ),
    ).toBe(false);
  });

  it('returns false when clicking on empty canvas', () => {
    expect(
      shouldShowVariableDetails(
        false, // hadSelection
        undefined, // moveDelta
        false, // isMovingArrow
        false, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(false);
  });

  it('returns false when clicking on arrowhead without movement', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        undefined, // moveDelta
        true, // isMovingArrow
        false, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(false);
  });

  it('returns false when clicking on source without movement', () => {
    expect(
      shouldShowVariableDetails(
        true, // hadSelection
        undefined, // moveDelta
        false, // isMovingArrow
        true, // isMovingSource
        false, // isMovingLabel
      ),
    ).toBe(false);
  });
});
