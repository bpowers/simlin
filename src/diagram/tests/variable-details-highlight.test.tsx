// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The error underline in VariableDetails is decorated from props, not seeded
// into the Slate document: the Editor does not remount the panel when only the
// variable's errors change (so a draft survives an unrelated edit landing), and
// the underline must still follow the props. The engine's offsets describe the
// committed text, so a dirty draft is not underlined. Driven on the units
// field, which is always an editor (the equation field collapses to a preview
// when it has no errors).

import { describe, it, expect, beforeAll, rs } from '@rstest/core';

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
});

import * as React from 'react';
import { render, act, type RenderResult } from '@testing-library/react';
import { Editor, Transforms } from 'slate';
import { ELEMENT_TO_NODE } from 'slate-dom';

import type { Aux, AuxViewElement, ErrorCode, UnitError } from '@simlin/core/datamodel';

import { VariableDetails } from '../VariableDetails';

function makeAux(units: string, unitErrors: UnitError[] | undefined): Aux {
  return {
    type: 'aux',
    ident: 'x',
    equation: { type: 'scalar', equation: '1' },
    documentation: '',
    units,
    gf: undefined,
    data: undefined,
    errors: undefined,
    unitErrors,
    uid: undefined,
  } as unknown as Aux;
}

const viewElement: AuxViewElement = {
  type: 'aux',
  uid: 1,
  name: 'x',
  ident: 'x',
  var: undefined,
  x: 0,
  y: 0,
  labelSide: 'right',
  isZeroRadius: false,
};

const definitionError: UnitError[] = [{ start: 0, end: 3, code: 0 as unknown as ErrorCode, kind: 'definition' }];

function panel(variable: Aux): React.ReactElement {
  return (
    <VariableDetails
      variable={variable}
      viewElement={viewElement}
      onDelete={rs.fn()}
      onEquationChange={rs.fn()}
      onTableChange={rs.fn()}
      activeTab={0}
      onActiveTabChange={rs.fn()}
    />
  );
}

function underlined(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('.unitsEditor .eqnError')).map((el) => el.textContent ?? '');
}

function unitsEditor(container: HTMLElement): Editor {
  return ELEMENT_TO_NODE.get(container.querySelector('.unitsEditor') as HTMLElement) as unknown as Editor;
}

describe('VariableDetails error underline', () => {
  it('underlines the committed text from props and follows prop changes without re-seeding the editor', () => {
    let result!: RenderResult;
    act(() => {
      result = render(panel(makeAux('bad(units)', definitionError)));
    });
    expect(underlined(result.container)).toEqual(['bad']);
    const editor = unitsEditor(result.container);

    act(() => {
      result.rerender(panel(makeAux('bad(units)', undefined)));
    });
    expect(underlined(result.container)).toEqual([]);
    expect(unitsEditor(result.container)).toBe(editor);

    act(() => {
      result.rerender(panel(makeAux('bad(units)', [{ ...definitionError[0], start: 4, end: 9 }])));
    });
    expect(underlined(result.container)).toEqual(['units']);
    expect(unitsEditor(result.container)).toBe(editor);
  });

  it('does not underline a dirty draft, and a prop change keeps the draft text', async () => {
    let result!: RenderResult;
    act(() => {
      result = render(panel(makeAux('bad(units)', definitionError)));
    });
    const editor = unitsEditor(result.container);
    await act(async () => {
      Transforms.insertText(editor, '!', { at: Editor.end(editor, []) });
      editor.onChange();
      await Promise.resolve();
    });
    expect(underlined(result.container)).toEqual([]);

    act(() => {
      result.rerender(panel(makeAux('bad(units)', [{ ...definitionError[0] }])));
    });
    expect(result.container.querySelector('.unitsEditor')?.textContent).toBe('bad(units)!');
    expect(underlined(result.container)).toEqual([]);
  });
});
