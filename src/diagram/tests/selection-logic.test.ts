// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Set } from 'immutable';

import {
  computeMouseDownSelection,
  computeMouseUpSelection,
  pointerStateReset,
  resolveSelectionForReattachment,
} from '../selection-logic';

describe('computeMouseDownSelection', () => {
  it('click unselected element without modifier replaces selection', () => {
    const result = computeMouseDownSelection(Set([1, 2, 3]), 5, false);
    expect(result.newSelection).toEqual(Set([5]));
    expect(result.deferSingleSelect).toBeUndefined();
  });

  it('click unselected element with modifier adds to selection', () => {
    const result = computeMouseDownSelection(Set([1, 2]), 3, true);
    expect(result.newSelection).toEqual(Set([1, 2, 3]));
    expect(result.deferSingleSelect).toBeUndefined();
  });

  it('click selected element with modifier removes from selection', () => {
    const result = computeMouseDownSelection(Set([1, 2, 3]), 2, true);
    expect(result.newSelection).toEqual(Set([1, 3]));
    expect(result.deferSingleSelect).toBeUndefined();
  });

  it('click element already in multi-selection without modifier defers', () => {
    const result = computeMouseDownSelection(Set([1, 2, 3]), 2, false);
    expect(result.newSelection).toBeUndefined();
    expect(result.deferSingleSelect).toBe(2);
  });

  it('click sole selected element without modifier defers', () => {
    const result = computeMouseDownSelection(Set([5]), 5, false);
    expect(result.newSelection).toBeUndefined();
    expect(result.deferSingleSelect).toBe(5);
  });

  it('click with empty selection without modifier selects it', () => {
    const result = computeMouseDownSelection(Set(), 1, false);
    expect(result.newSelection).toEqual(Set([1]));
    expect(result.deferSingleSelect).toBeUndefined();
  });

  it('modifier click on only element in selection removes it', () => {
    const result = computeMouseDownSelection(Set([1]), 1, true);
    expect(result.newSelection).toEqual(Set());
    expect(result.deferSingleSelect).toBeUndefined();
  });
});

describe('resolveSelectionForReattachment', () => {
  it('overrides selection with flow UID when re-attachment is activated', () => {
    // When clicking a cloud triggers flow re-attachment, the selection must
    // contain the flow UID -- mouseUp reads only(selection) and expects a
    // FlowViewElement for attachment handling.
    const cloudUid = 10;
    const flowUid = 20;
    const result = resolveSelectionForReattachment(Set([cloudUid]), true, flowUid);
    expect(result).toEqual(Set([flowUid]));
  });

  it('preserves original selection when re-attachment is not activated', () => {
    const result = resolveSelectionForReattachment(Set([10]), false, 20);
    expect(result).toEqual(Set([10]));
  });

  it('preserves multi-element selection when re-attachment is not activated', () => {
    const result = resolveSelectionForReattachment(Set([1, 2, 3]), false, 20);
    expect(result).toEqual(Set([1, 2, 3]));
  });
});

describe('computeMouseUpSelection', () => {
  it('deferred + no drag collapses to single element', () => {
    const result = computeMouseUpSelection(2, false);
    expect(result).toEqual(Set([2]));
  });

  it('deferred + drag occurred returns undefined', () => {
    const result = computeMouseUpSelection(2, true);
    expect(result).toBeUndefined();
  });

  it('no deferred UID + no drag returns undefined', () => {
    const result = computeMouseUpSelection(undefined, false);
    expect(result).toBeUndefined();
  });

  it('no deferred UID + drag returns undefined', () => {
    const result = computeMouseUpSelection(undefined, true);
    expect(result).toBeUndefined();
  });
});

describe('pointerStateReset', () => {
  it('clears moveDelta', () => {
    const reset = pointerStateReset();
    expect(reset.moveDelta).toBeUndefined();
  });

  it('clears all movement flags', () => {
    const reset = pointerStateReset();
    expect(reset.isMovingArrow).toBe(false);
    expect(reset.isMovingSource).toBe(false);
    expect(reset.isMovingLabel).toBe(false);
    expect(reset.isMovingCanvas).toBe(false);
  });

  it('clears all interaction state', () => {
    const reset = pointerStateReset();
    expect(reset.isDragSelecting).toBe(false);
    expect(reset.isEditingName).toBe(false);
    expect(reset.labelSide).toBeUndefined();
    expect(reset.dragSelectionPoint).toBeUndefined();
    expect(reset.inCreation).toBeUndefined();
    expect(reset.inCreationCloud).toBeUndefined();
    expect(reset.draggingSegmentIndex).toBeUndefined();
  });

  it('returns consistent values across calls', () => {
    const a = pointerStateReset();
    const b = pointerStateReset();
    expect(a).toEqual(b);
  });
});
