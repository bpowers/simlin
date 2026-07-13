// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// VariableDetails in read-only mode (issue #935): the panel opens for
// INSPECTION -- equation preview, chart, units, docs, lookup shape -- but every
// editing affordance is inert and visibly absent, so it cannot present the
// "editable but unsavable" trap the Editor-level gate closes.

import { describe, it, expect, beforeAll, rs } from '@rstest/core';

beforeAll(() => {
  // slate-react gates on element.isContentEditable, which jsdom lacks.
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
  // jsdom does not implement Range.getBoundingClientRect, which the preview
  // click-to-caret mapping calls when a click DOES open the editor.
  if (!('getBoundingClientRect' in Range.prototype)) {
    const zero = () =>
      ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} }) as DOMRect;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Range.prototype as any).getBoundingClientRect = zero;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Range.prototype as any).getClientRects = () =>
      ({ length: 0, item: () => null, [Symbol.iterator]: function* () {} }) as unknown as DOMRectList;
  }
});

import * as React from 'react';
import { render, act, fireEvent } from '@testing-library/react';
import { VariableDetails } from '../VariableDetails';
import { Aux, AuxViewElement, EquationError, ErrorCode, GraphicalFunction } from '@simlin/core/datamodel';

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

const someGf: GraphicalFunction = {
  kind: 'continuous',
  xScale: { min: 0, max: 1 },
  yScale: { min: 0, max: 1 },
  xPoints: undefined,
  yPoints: [0, 0.5, 1.0],
};

const forceEditorOpen: EquationError[] = [{ start: 0, end: 1, code: 0 as unknown as ErrorCode }];

const noop = () => {};

interface RenderOpts {
  readOnly?: boolean;
  activeTab?: number;
}

function renderDetails(variable: Aux, opts: RenderOpts = {}) {
  const onEquationChange = rs.fn();
  const onTableChange = rs.fn();
  const onDelete = rs.fn();
  const result = render(
    <VariableDetails
      variable={variable}
      viewElement={makeViewElement(variable.ident)}
      onDelete={onDelete}
      onEquationChange={onEquationChange}
      onTableChange={onTableChange}
      activeTab={opts.activeTab ?? 0}
      onActiveTabChange={noop}
      readOnly={opts.readOnly}
    />,
  );
  return { container: result.container, onEquationChange, onTableChange, onDelete };
}

function buttonTexts(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('button')).map((b) => b.textContent ?? '');
}

describe('VariableDetails read-only mode', () => {
  it('editable mode still offers Delete/Cancel/Save (control)', () => {
    const { container } = renderDetails(makeAux('x', 'a + b'));
    const texts = buttonTexts(container);
    expect(texts).toContain('Delete');
    expect(texts).toContain('Cancel');
    expect(texts).toContain('Save');
  });

  it('hides the Delete/Cancel/Save action row', () => {
    const { container } = renderDetails(makeAux('x', 'a + b'), { readOnly: true });
    const texts = buttonTexts(container);
    expect(texts).not.toContain('Delete');
    expect(texts).not.toContain('Cancel');
    expect(texts).not.toContain('Save');
  });

  it('clicking the equation preview does not open the raw editor', async () => {
    const { container } = renderDetails(makeAux('x', 'a + b'), { readOnly: true });
    const preview = container.querySelector('.eqnPreview');
    expect(preview).not.toBeNull();
    await act(async () => {
      fireEvent.click(preview as Element);
    });
    await act(async () => {
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    });
    expect(container.querySelector('.eqnEditor')).toBeNull();
    // The preview stays up for inspection.
    expect(container.querySelector('.eqnPreview')).not.toBeNull();
  });

  it('the error-pinned raw equation editor renders non-editable', () => {
    // An equation error pins the raw editor open so the highlight is visible;
    // that inspection value is preserved read-only, not blanked.
    const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }), { readOnly: true });
    const editor = container.querySelector('.eqnEditor');
    expect(editor).not.toBeNull();
    expect(editor!.getAttribute('contenteditable')).toBe('false');
  });

  it('units and documentation editors are non-editable', () => {
    const { container } = renderDetails(makeAux('x', 'a + b'), { readOnly: true });
    const units = container.querySelector('.unitsEditor');
    const notes = container.querySelector('.notesEditor');
    expect(units).not.toBeNull();
    expect(notes).not.toBeNull();
    expect(units!.getAttribute('contenteditable')).toBe('false');
    expect(notes!.getAttribute('contenteditable')).toBe('false');
  });

  it('hides the "Add lookup table" affordance on the lookup tab', () => {
    const { container, onTableChange } = renderDetails(makeAux('x', 'a + b'), { readOnly: true, activeTab: 1 });
    const add = Array.from(container.querySelectorAll('button')).find((b) =>
      (b.textContent ?? '').includes('Add lookup table'),
    );
    expect(add).toBeUndefined();
    expect(onTableChange).not.toHaveBeenCalled();
  });

  it('shows the lookup read-only: no Remove/Cancel/Save, inputs disabled', () => {
    const { container } = renderDetails(makeAux('x', 'a + b', { gf: someGf }), { readOnly: true, activeTab: 1 });
    const texts = buttonTexts(container);
    expect(texts).not.toContain('Remove');
    expect(texts).not.toContain('Cancel');
    expect(texts).not.toContain('Save');
    const inputs = Array.from(container.querySelectorAll('input'));
    expect(inputs.length).toBeGreaterThan(0);
    for (const input of inputs) {
      expect(input.disabled).toBe(true);
    }
  });

  it('lookup stays editable without the flag (control)', () => {
    const { container } = renderDetails(makeAux('x', 'a + b', { gf: someGf }), { activeTab: 1 });
    const texts = buttonTexts(container);
    expect(texts).toContain('Remove');
    expect(texts).toContain('Save');
  });
});
