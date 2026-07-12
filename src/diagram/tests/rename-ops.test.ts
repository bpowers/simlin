// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import type { AuxViewElement, LinkViewElement, StockFlowView } from '@simlin/core/datamodel';
import type { JsonViewElement } from '@simlin/engine';

import { buildVariableRenameOps } from '../rename-ops';

function makeAuxElement(overrides: Partial<AuxViewElement> = {}): AuxViewElement {
  return {
    type: 'aux',
    uid: 1,
    name: 'Total Students',
    ident: 'total_students',
    var: undefined,
    x: 10,
    y: 20,
    labelSide: 'right',
    isZeroRadius: false,
    ...overrides,
  };
}

function makeView(elements: StockFlowView['elements']): StockFlowView {
  return {
    nextUid: 10,
    elements,
    viewBox: { x: 0, y: 0, width: 100, height: 100 },
    zoom: 1,
    useLetteredPolarity: false,
  };
}

describe('buildVariableRenameOps', () => {
  it('sends the typed name RAW as the rename `to` and the canonical ident as `from`', () => {
    // The engine preserves display spellings verbatim and matches canonically
    // (issue #890); canonicalizing `to` here downgraded the stored spelling
    // ("New Students" -> `new_students`) on every rename (issue #906).
    const view = makeView([makeAuxElement()]);
    const { ops } = buildVariableRenameOps(view, 'Total Students', 'New Students');

    expect(ops[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'total_students', to: 'New Students' },
    });
  });

  it('preserves the display spelling on a case-only rename', () => {
    // A case-only rename is exactly the "restamp a preserved spelling" bug:
    // from and to canonicalize identically, so a canonicalized `to` would be a
    // no-op rename that still destroyed the display name.
    const view = makeView([makeAuxElement({ name: 'students', ident: 'students' })]);
    const { ops } = buildVariableRenameOps(view, 'students', 'Students');

    expect(ops[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'students', to: 'Students' },
    });
  });

  it('encodes line breaks in the new name (stored backslash-n form)', () => {
    const view = makeView([makeAuxElement()]);
    const { ops, updatedView } = buildVariableRenameOps(view, 'Total Students', 'testing\nassymptomatic');

    expect(ops[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'total_students', to: 'testing\\nassymptomatic' },
    });
    expect((updatedView.elements[0] as AuxViewElement).name).toBe('testing\\nassymptomatic');
  });

  it('updates only the matching named view element and pairs an upsertView op', () => {
    const other = makeAuxElement({ uid: 2, name: 'Other Var', ident: 'other_var' });
    const link: LinkViewElement = {
      type: 'link',
      uid: 3,
      fromUid: 1,
      toUid: 2,
      arc: undefined,
      isStraight: true,
      multiPoint: undefined,
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };
    const view = makeView([makeAuxElement(), other, link]);

    const { ops, updatedView } = buildVariableRenameOps(view, 'Total Students', 'Enrolled Students');

    expect((updatedView.elements[0] as AuxViewElement).name).toBe('Enrolled Students');
    // Unmatched elements pass through by reference.
    expect(updatedView.elements[1]).toBe(other);
    expect(updatedView.elements[2]).toBe(link);

    expect(ops).toHaveLength(2);
    expect(ops[1].type).toBe('upsertView');
    if (ops[1].type !== 'upsertView') {
      throw new Error('expected upsertView');
    }
    const serialized = ops[1].payload.view.elements as JsonViewElement[];
    const renamed = serialized.find((e) => e.uid === 1);
    expect(renamed && 'name' in renamed ? renamed.name : undefined).toBe('Enrolled Students');
  });
});
