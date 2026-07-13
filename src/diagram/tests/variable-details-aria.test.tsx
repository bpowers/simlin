// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Programmatic error association in VariableDetails: the equation and units
// fields carry aria-invalid / aria-describedby pointing at the rendered
// error/warning rows, so the failure is announced with the field instead of
// being conveyed by the red/orange highlight alone.

import { describe, it, expect, beforeAll, rs } from '@rstest/core';

beforeAll(() => {
  // slate-react gates on element.isContentEditable, which jsdom lacks.
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
});

import * as React from 'react';
import { render } from '@testing-library/react';
import { VariableDetails } from '../VariableDetails';
import { Aux, AuxViewElement, EquationError, ErrorCode, UnitError } from '@simlin/core/datamodel';

function makeAux(ident: string, equation: string, overrides: Partial<Aux> = {}): Aux {
  return {
    type: 'aux',
    ident,
    equation: { type: 'scalar', equation },
    documentation: '',
    units: '',
    gf: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeViewElement(ident: string): AuxViewElement {
  return {
    type: 'aux',
    uid: 1,
    name: ident,
    ident,
    var: undefined,
    x: 0,
    y: 0,
    labelSide: 'right',
    isZeroRadius: false,
  };
}

const equationError: EquationError[] = [{ start: 0, end: 1, code: 0 as unknown as ErrorCode }];
const unitError: UnitError[] = [
  { start: 0, end: 0, code: 0 as unknown as ErrorCode, kind: 'definition', details: "computed units don't match" },
];

const noop = () => {};

function renderDetails(variable: Aux) {
  return render(
    <VariableDetails
      variable={variable}
      viewElement={makeViewElement(variable.ident)}
      onDelete={rs.fn()}
      onEquationChange={rs.fn()}
      onTableChange={rs.fn()}
      activeTab={0}
      onActiveTabChange={noop}
    />,
  );
}

// The equation editor is the first Slate Editable in the panel; the units
// editor is the second. Both render as contenteditable divs.
function editables(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>('[data-slate-editor="true"]'));
}

describe('VariableDetails error association', () => {
  it('an equation error marks the pinned-open editor invalid and points at the error row', () => {
    const { container } = renderDetails(makeAux('x', '+', { errors: equationError }));
    const [eqn] = editables(container);
    expect(eqn.getAttribute('aria-invalid')).toBe('true');

    const describedBy = eqn.getAttribute('aria-describedby');
    expect(describedBy).not.toBeNull();
    // Every referenced id must resolve to a rendered error row (no dangling
    // references), and that row must carry the announced text.
    for (const id of describedBy!.split(' ')) {
      const row = document.getElementById(id);
      expect(row).not.toBeNull();
      expect(row!.textContent).toMatch(/error/i);
    }
  });

  it('unit warnings describe the units field without marking it invalid', () => {
    const { container } = renderDetails(makeAux('x', 'a + b', { units: 'widgets', unitErrors: unitError }));
    const all = editables(container);
    // No equation error, so the preview shows and the units editor is the
    // first (equation) or second Editable depending on edit state; find it by
    // its association instead of position.
    const units = all.find((el) => el.getAttribute('aria-describedby'));
    expect(units).toBeDefined();
    expect(units!.getAttribute('aria-invalid')).toBeNull();

    for (const id of units!.getAttribute('aria-describedby')!.split(' ')) {
      const row = document.getElementById(id);
      expect(row).not.toBeNull();
      expect(row!.textContent).toMatch(/unit error/i);
    }
  });

  it('a healthy variable has no invalid marking and no dangling descriptions', () => {
    const { container } = renderDetails(makeAux('x', 'a + b'));
    for (const el of editables(container)) {
      expect(el.getAttribute('aria-invalid')).toBeNull();
      expect(el.getAttribute('aria-describedby')).toBeNull();
    }
  });
});
