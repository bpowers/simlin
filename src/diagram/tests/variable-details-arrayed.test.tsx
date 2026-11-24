// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// What the details panel shows for an arrayed variable, and what it submits.
// The equation field holds an apply-to-all variable's equation and nothing
// else: whatever text it holds is what a save sends, so the dimensions are
// named beside it. A per-element variable has no such field -- its equations
// are listed read-only -- so nothing the user does in the panel submits one.
//
// The wording (dimension lists, the default, lookups) is tabled in
// variable-details-display.test.ts; what a submission does to the model is
// editor-panel-edits-engine.test.ts.

import { describe, it, expect, beforeAll, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
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
import { act, fireEvent, render, waitFor } from '@testing-library/react';
import { Editor, Transforms } from 'slate';
import { ELEMENT_TO_NODE } from 'slate-dom';

import { auxFromJson, ErrorCode, type Aux, type AuxViewElement, type EquationError } from '@simlin/core/datamodel';
import type { JsonAuxiliary } from '@simlin/engine';

import { VariableDetails, type PendingSubmission } from '../VariableDetails';

// Variables come through the production deserializer, so their shapes are the
// ones the engine's JSON gives the panel.
function variable(json: Partial<JsonAuxiliary>, overrides: Partial<Aux> = {}): Aux {
  return { ...auxFromJson({ name: 'price', ...json }), ...overrides };
}

const applyToAll = (equation: string, overrides: Partial<Aux> = {}): Aux =>
  variable({ arrayedEquation: { dimensions: ['Region'], equation } }, overrides);

const perElement = (overrides: Partial<Aux> = {}): Aux =>
  variable(
    {
      arrayedEquation: {
        dimensions: ['Region'],
        equation: '7',
        hasExceptDefault: true,
        elements: [
          { subscript: 'north', equation: '1' },
          { subscript: 'south', equation: 'cost[south] *\n  2' },
        ],
      },
    },
    overrides,
  );

const viewElement: AuxViewElement = {
  type: 'aux',
  uid: 1,
  name: 'price',
  ident: 'price',
  var: undefined,
  x: 0,
  y: 0,
  labelSide: 'right',
  isZeroRadius: false,
};

interface Options {
  readonly readOnly?: boolean;
  readonly getLatexEquation?: (ident: string) => Promise<string | undefined>;
  readonly pendingSubmission?: PendingSubmission;
}

interface Panel {
  readonly container: HTMLElement;
  readonly onEquationChange: Mock;
  flush(): boolean;
}

function renderPanel(v: Aux, options: Options = {}): Panel {
  const onEquationChange = rs.fn();
  let flush: () => boolean = () => false;
  const { container } = render(
    <VariableDetails
      variable={v}
      viewElement={viewElement}
      getLatexEquation={options.getLatexEquation}
      onDelete={rs.fn()}
      onEquationChange={onEquationChange}
      onTableChange={rs.fn()}
      activeTab={0}
      onActiveTabChange={rs.fn()}
      readOnly={options.readOnly}
      pendingSubmission={options.pendingSubmission}
      registerDraftFlush={(f) => {
        flush = f;
        return () => {};
      }}
    />,
  );
  return { container, onEquationChange, flush: () => flush() };
}

function editorFor(container: HTMLElement, selector: string): Editor {
  const el = container.querySelector(selector);
  expect(el).not.toBeNull();
  return ELEMENT_TO_NODE.get(el as HTMLElement) as unknown as Editor;
}

async function setField(container: HTMLElement, selector: string, text: string): Promise<void> {
  const editor = editorFor(container, selector);
  await act(async () => {
    Transforms.delete(editor, { at: { anchor: Editor.start(editor, []), focus: Editor.end(editor, []) } });
    Transforms.insertText(editor, text, { at: Editor.end(editor, []) });
    editor.onChange();
    await Promise.resolve();
  });
  await waitFor(() => expect(button(container, 'Save').disabled).toBe(false));
}

function button(container: HTMLElement, text: string): HTMLButtonElement {
  const found = Array.from(container.querySelectorAll('button')).find((b) => b.textContent === text);
  expect(found).toBeDefined();
  return found!;
}

async function openEquationEditor(container: HTMLElement): Promise<void> {
  await act(async () => {
    fireEvent.click(container.querySelector('.eqnPreview')!);
  });
  await act(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
  });
}

function note(container: HTMLElement): string | undefined {
  return container.querySelector('.eqnNote')?.textContent ?? undefined;
}

function listed(container: HTMLElement): string[][] {
  const names = Array.from(container.querySelectorAll('dt'), (el) => el.textContent ?? '');
  const equations = Array.from(container.querySelectorAll('dd'), (el) => el.textContent ?? '');
  expect(equations).toHaveLength(names.length);
  return names.map((name, i) => [name, equations[i]]);
}

const equationErrorAt = (start: number, end: number): EquationError[] => [
  { start, end, code: ErrorCode.UnknownDependency, details: 'bad_ref is not a variable' },
];

describe('VariableDetails for an apply-to-all variable', () => {
  it('shows the equation itself, with its dimension named outside the field', () => {
    const { container } = renderPanel(applyToAll('10'));
    expect(container.querySelector('.eqnPreview')!.textContent).toBe('10');
    expect(note(container)).toBe('Each element of Region');
    expect(container.querySelector('[contenteditable] .eqnNote')).toBeNull();
  });

  it('shows no such line for a scalar (control)', () => {
    const { container } = renderPanel(variable({ equation: '10' }));
    expect(container.querySelector('.eqnPreview')!.textContent).toBe('10');
    expect(note(container)).toBeUndefined();
  });

  it('opens the equation text alone in the editor, and submits exactly what is typed', async () => {
    const { container, onEquationChange } = renderPanel(applyToAll('10'));
    await openEquationEditor(container);
    expect(container.querySelector('.eqnEditor')!.textContent).toBe('10');
    await setField(container, '.eqnEditor', '12');
    act(() => {
      fireEvent.click(button(container, 'Save'));
    });
    expect(onEquationChange.mock.calls).toEqual([['price', '12', undefined, undefined]]);
  });

  it('Cancel puts the equation back', async () => {
    const { container, onEquationChange } = renderPanel(applyToAll('10'));
    await openEquationEditor(container);
    await setField(container, '.eqnEditor', '12');
    act(() => {
      fireEvent.click(button(container, 'Cancel'));
    });
    expect(container.querySelector('.eqnPreview')!.textContent).toBe('10');
    expect(onEquationChange).not.toHaveBeenCalled();
  });

  it('shows its pending text, and keeps it as a draft when the edit does not land', async () => {
    let settle!: (landed: boolean) => void;
    const landed = new Promise<boolean>((resolve) => {
      settle = resolve;
    });
    const { container, onEquationChange } = renderPanel(applyToAll('10'), {
      pendingSubmission: { equation: { text: '12', landed } },
    });
    expect(container.querySelector('.eqnPreview')!.textContent).toBe('12');
    expect(button(container, 'Save').disabled).toBe(true);
    await act(async () => {
      settle(false);
      await landed;
    });
    expect(button(container, 'Save').disabled).toBe(false);
    act(() => {
      fireEvent.click(button(container, 'Save'));
    });
    expect(onEquationChange.mock.calls).toEqual([['price', '12', undefined, undefined]]);
  });

  it("underlines an error at the engine's offsets into the equation", () => {
    const { container } = renderPanel(applyToAll('a +\nbad_ref', { errors: equationErrorAt(4, 11) }));
    expect(Array.from(container.querySelectorAll('.eqnEditor .eqnError'), (el) => el.textContent)).toEqual(['bad_ref']);
  });

  it('describes the editor with the dimension line and the error rows', () => {
    const { container } = renderPanel(applyToAll('a + bad_ref', { errors: equationErrorAt(4, 11) }));
    const ids = container.querySelector('.eqnEditor')!.getAttribute('aria-describedby')!.split(' ');
    expect(ids.map((id) => document.getElementById(id)?.textContent)).toEqual([
      'Each element of Region',
      'error: bad_ref is not a variable',
    ]);
  });

  // The engine's LaTeX for an arrayed variable is a placeholder, not an equation.
  it('previews the equation as text, without asking for LaTeX', async () => {
    const getLatexEquation = rs.fn(async () => 'TODO(array)');
    const { container } = renderPanel(applyToAll('10'), { getLatexEquation });
    await act(async () => {});
    expect(getLatexEquation).not.toHaveBeenCalled();
    expect(container.querySelector('.katex')).toBeNull();
    expect(container.querySelector('.eqnPreview')!.textContent).toBe('10');
  });

  it("asks for a scalar's LaTeX (control)", async () => {
    const getLatexEquation = rs.fn(async () => '10');
    const { container } = renderPanel(variable({ equation: '10' }), { getLatexEquation });
    await act(async () => {});
    expect(getLatexEquation).toHaveBeenCalledWith('price');
    expect(container.querySelector('.katex')).not.toBeNull();
  });

  it('is not editable read-only', async () => {
    const { container } = renderPanel(applyToAll('10'), { readOnly: true });
    await openEquationEditor(container);
    expect(container.querySelector('.eqnEditor')).toBeNull();
    expect(container.querySelector('.eqnPreview')!.textContent).toBe('10');
    expect(note(container)).toBe('Each element of Region');
  });
});

describe('VariableDetails for a per-element variable', () => {
  const rows = [
    ['north', '1'],
    ['south', 'cost[south] *\n  2'],
    ['all others', '7'],
  ];
  const sentence = "The equations for each element of Region can't be edited here yet.";

  for (const readOnly of [false, true]) {
    it(`lists every equation, and has no equation field (${readOnly ? 'read-only' : 'editable'})`, () => {
      const { container } = renderPanel(perElement(), { readOnly });
      expect(listed(container)).toEqual(rows);
      expect(note(container)).toBe(sentence);
      expect(container.querySelector('.eqnPreview')).toBeNull();
      // The only editors are units and documentation.
      expect(
        Array.from(container.querySelectorAll('[data-slate-editor]'), (el) => el.classList.contains('eqnEditor')),
      ).toEqual([false, false]);
    });
  }

  it('lists a per-element variable with no default without one', () => {
    const { container } = renderPanel(
      variable({
        arrayedEquation: { dimensions: ['Region'], elements: [{ subscript: 'north', equation: '1' }] },
      }),
    );
    expect(listed(container)).toEqual([['north', '1']]);
  });

  it('opens no editor when its equations are clicked', async () => {
    const { container, flush, onEquationChange } = renderPanel(perElement());
    await act(async () => {
      fireEvent.click(container.querySelector('dd')!);
    });
    expect(container.querySelector('.eqnEditor')).toBeNull();
    expect(button(container, 'Save').disabled).toBe(true);
    expect(flush()).toBe(false);
    expect(onEquationChange).not.toHaveBeenCalled();
  });

  it('opens no editor for an equation error, and describes the list with it', () => {
    const { container } = renderPanel(perElement({ errors: equationErrorAt(0, 4) }));
    expect(container.querySelector('.eqnEditor')).toBeNull();
    expect(listed(container)).toEqual(rows);
    const ids = container.querySelector('.elementEquations')!.getAttribute('aria-describedby')!.split(' ');
    expect(ids.map((id) => document.getElementById(id)?.textContent)).toEqual([
      sentence,
      'error: bad_ref is not a variable',
    ]);
  });

  it('submits units and notes, and no equation', async () => {
    const { container, flush, onEquationChange } = renderPanel(perElement());
    await setField(container, '.unitsEditor', 'widgets');
    await setField(container, '.notesEditor', 'why');
    let submitted = false;
    act(() => {
      submitted = flush();
    });
    expect(submitted).toBe(true);
    expect(onEquationChange.mock.calls).toEqual([['price', undefined, 'widgets', 'why']]);
  });

  // A submission recorded for the element before its variable became
  // per-element: nothing shows its text, so it must not come back as a draft.
  it('submits no equation left over from a pending submission that did not land', async () => {
    const landed = Promise.resolve(false);
    const { container, flush, onEquationChange } = renderPanel(perElement(), {
      pendingSubmission: { equation: { text: '12', landed } },
    });
    await act(async () => {
      await landed;
    });
    expect(button(container, 'Save').disabled).toBe(true);
    act(() => {
      expect(flush()).toBe(false);
    });
    expect(onEquationChange).not.toHaveBeenCalled();
  });

  it('asks for no LaTeX', async () => {
    const getLatexEquation = rs.fn(async () => 'TODO(array)');
    const { container } = renderPanel(perElement(), { getLatexEquation });
    await act(async () => {});
    expect(getLatexEquation).not.toHaveBeenCalled();
    expect(container.querySelector('.katex')).toBeNull();
  });
});
