/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Discarding in-progress equation edits (GitHub issue #86). All three fields
// (equation, units, notes) save on blur, which is load-bearing: a canvas-driven
// edit blurs the panel editor to commit its text. The bug was that clicking
// Cancel -- or pressing Escape -- blurred the focused editor FIRST, so the edit
// committed before the discard ran. These tests pin that Cancel/Escape now win
// over the blur-save while a genuine blur-away still commits.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

beforeAll(() => {
  // slate-react's keyDown pipeline gates on ReactEditor.hasEditableTarget ->
  // element.isContentEditable before forwarding to our onKeyDown; jsdom lacks it.
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
  // jsdom does not implement Range.getBoundingClientRect, which the preview
  // click-to-caret mapping calls. Stub it (and getClientRects) so clicking the
  // equation preview opens the raw editor instead of throwing.
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
import { Editor, Transforms } from 'slate';
import { HistoryEditor } from 'slate-history';
import { ELEMENT_TO_NODE } from 'slate-dom';
import { VariableDetails } from '../VariableDetails';
import { Aux, AuxViewElement, EquationError, ErrorCode } from '@simlin/core/datamodel';

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

// An equation error forces the raw equation editor open on mount (showPreview
// gating), so the contenteditable is present to type into and stays present
// after a discard (letting us assert the reverted content).
const forceEditorOpen: EquationError[] = [{ start: 0, end: 1, code: 0 as unknown as ErrorCode }];

const noop = () => {};

interface Harness {
  container: HTMLElement;
  onEquationChange: jest.Mock;
}

function renderDetails(variable: Aux): Harness {
  const onEquationChange = jest.fn();
  const { container } = render(
    <VariableDetails
      variable={variable}
      viewElement={makeViewElement(variable.ident)}
      onDelete={noop}
      onEquationChange={onEquationChange}
      onTableChange={noop}
      activeTab={0}
      onActiveTabChange={noop}
    />,
  );
  return { container, onEquationChange };
}

// The Slate editor instance backing a rendered Editable, reachable through
// slate-dom's element->node map. Driving edits through it (rather than fake
// DOM input events, which jsdom does not support for Slate) is the highest
// fidelity typing available.
function editorFor(container: HTMLElement, selector: string): Editor {
  const el = container.querySelector(selector);
  expect(el).not.toBeNull();
  return ELEMENT_TO_NODE.get(el as HTMLElement) as unknown as Editor;
}

async function appendText(editor: Editor, text: string): Promise<void> {
  await act(async () => {
    Transforms.insertText(editor, text, { at: Editor.end(editor, []) });
    // Slate defers its onChange to a microtask; nudge it and flush so the
    // component's React state (and Save/Cancel enabled state) updates.
    editor.onChange();
    await Promise.resolve();
  });
}

function stripZeroWidth(s: string | null): string {
  return Array.from(s ?? '')
    .filter((c) => c.charCodeAt(0) !== 0xfeff && c.charCodeAt(0) !== 0x200b)
    .join('');
}

function buttonByText(container: HTMLElement, text: string): HTMLButtonElement {
  const btn = Array.from(container.querySelectorAll('button')).find((b) => b.textContent === text);
  expect(btn).toBeTruthy();
  return btn as HTMLButtonElement;
}

async function openEquationEditorViaPreview(container: HTMLElement): Promise<void> {
  const preview = container.querySelector('.eqnPreview');
  expect(preview).not.toBeNull();
  await act(async () => {
    fireEvent.click(preview as Element);
  });
  await act(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
  });
}

describe('VariableDetails discard (Cancel / Escape)', () => {
  it('click Cancel discards the equation edit and reverts the field', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');

    const editable = container.querySelector('.eqnEditor') as HTMLElement;
    expect(editable.textContent).toBe('a + bZ');
    const cancel = buttonByText(container, 'Cancel');
    expect(cancel.disabled).toBe(false);

    // The browser sequence for a mouse click on Cancel: pointer-down (our
    // preventDefault keeps focus, so relatedTarget is the button), then click.
    const notPrevented = fireEvent.mouseDown(cancel);
    expect(notPrevented).toBe(false); // onMouseDown preventDefault is wired
    await act(async () => {
      fireEvent.blur(editable, { relatedTarget: cancel });
      fireEvent.click(cancel);
      await Promise.resolve();
    });

    expect(onEquationChange).not.toHaveBeenCalled();
    // The visible editor content reverted to the original equation.
    expect((container.querySelector('.eqnEditor') as HTMLElement).textContent).toBe('a + b');
    expect(buttonByText(container, 'Cancel').disabled).toBe(true);
  });

  it('keyboard-activated Cancel (no pointer-down) also discards', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');

    const editable = container.querySelector('.eqnEditor') as HTMLElement;
    const cancel = buttonByText(container, 'Cancel');
    // Tabbing to the button blurs the editor (relatedTarget = the button); then
    // Enter/Space fires the click. No pointer-down occurs, so this exercises the
    // focusLeftPanel guard, not the button's preventDefault.
    await act(async () => {
      fireEvent.blur(editable, { relatedTarget: cancel });
      fireEvent.click(cancel);
      await Promise.resolve();
    });

    expect(onEquationChange).not.toHaveBeenCalled();
    expect((container.querySelector('.eqnEditor') as HTMLElement).textContent).toBe('a + b');
  });

  it('Escape discards the equation edit and reverts to the preview', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b'));
    await openEquationEditorViaPreview(container);
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');
    expect((container.querySelector('.eqnEditor') as HTMLElement).textContent).toBe('a + bZ');

    await act(async () => {
      fireEvent.keyDown(container.querySelector('.eqnEditor') as Element, { key: 'Escape' });
      await Promise.resolve();
    });

    expect(onEquationChange).not.toHaveBeenCalled();
    // Escape exits the raw editor back to the preview, showing the original.
    expect(container.querySelector('.eqnEditor')).toBeNull();
    const preview = container.querySelector('.eqnPreview');
    expect(preview).not.toBeNull();
    expect(preview!.textContent).toContain('a + b');
    expect(preview!.textContent).not.toContain('a + bZ');
  });

  it('Escape discards when an equation error pins the raw editor open', async () => {
    // With a fatal error the editor cannot fall back to the preview, so it stays
    // mounted after the discard -- a distinct branch from the no-error path
    // above. The revert must land in the still-mounted editor.
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');
    expect((container.querySelector('.eqnEditor') as HTMLElement).textContent).toBe('a + bZ');

    await act(async () => {
      fireEvent.keyDown(container.querySelector('.eqnEditor') as Element, { key: 'Escape' });
      await Promise.resolve();
    });

    expect(onEquationChange).not.toHaveBeenCalled();
    const editable = container.querySelector('.eqnEditor');
    expect(editable).not.toBeNull();
    expect((editable as HTMLElement).textContent).toBe('a + b');
  });

  it('switching to the Lookup Function tab commits a pending edit', async () => {
    // The tab strip sits inside the card, so the blur toward it is an
    // intra-panel blur the focusLeftPanel gate deliberately skips -- but the
    // tab switch hides the equation editors (and the Lookup tab renders no
    // Save/Cancel), so a pending edit would be stranded invisibly and dropped
    // by the next keyed remount. Changing tabs must commit, like blur-away.
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');

    const editable = container.querySelector('.eqnEditor') as HTMLElement;
    const lookupTab = buttonByText(container, 'Lookup Function');
    await act(async () => {
      fireEvent.mouseDown(lookupTab);
      fireEvent.blur(editable, { relatedTarget: lookupTab });
      fireEvent.click(lookupTab);
      await Promise.resolve();
    });

    expect(onEquationChange).toHaveBeenCalledTimes(1);
    expect(onEquationChange).toHaveBeenCalledWith('x', 'a + bZ', undefined, undefined);
  });

  it('blur to the canvas (focus leaves the panel) still commits the edit', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');

    const editable = container.querySelector('.eqnEditor') as HTMLElement;
    // relatedTarget null models focus moving to a non-focusable canvas / nowhere.
    await act(async () => {
      fireEvent.blur(editable, { relatedTarget: null });
      await Promise.resolve();
    });

    expect(onEquationChange).toHaveBeenCalledTimes(1);
    expect(onEquationChange).toHaveBeenCalledWith('x', 'a + bZ', undefined, undefined);
  });

  it('Save commits the edited equation exactly once', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');

    const editable = container.querySelector('.eqnEditor') as HTMLElement;
    const save = buttonByText(container, 'Save');
    await act(async () => {
      // Focus moves to Save (inside the panel) then the click fires. The blur
      // must not double-commit; only the click's onClick should.
      fireEvent.blur(editable, { relatedTarget: save });
      fireEvent.click(save);
      await Promise.resolve();
    });

    expect(onEquationChange).toHaveBeenCalledTimes(1);
    expect(onEquationChange).toHaveBeenCalledWith('x', 'a + bZ', undefined, undefined);
  });

  it('Cancel discards an in-progress units edit', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b'));
    const unitsEditor = editorFor(container, '.unitsEditor');
    await appendText(unitsEditor, 'kg');

    const unitsEditable = container.querySelector('.unitsEditor') as HTMLElement;
    expect(unitsEditable.textContent).toBe('kg');
    const cancel = buttonByText(container, 'Cancel');
    await act(async () => {
      fireEvent.blur(unitsEditable, { relatedTarget: cancel });
      fireEvent.click(cancel);
      await Promise.resolve();
    });

    expect(onEquationChange).not.toHaveBeenCalled();
    // The typed units are gone (the emptied field falls back to its
    // placeholder), and with every field back to its original the actions
    // disable again.
    expect(stripZeroWidth((container.querySelector('.unitsEditor') as HTMLElement).textContent)).not.toContain('kg');
    expect(buttonByText(container, 'Cancel').disabled).toBe(true);
  });

  it('Escape in the notes field discards its edit', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b'));
    const notesEditor = editorFor(container, '.notesEditor');
    await appendText(notesEditor, 'hello');

    const notesEditable = container.querySelector('.notesEditor') as HTMLElement;
    expect(notesEditable.textContent).toBe('hello');
    await act(async () => {
      fireEvent.keyDown(notesEditable, { key: 'Escape' });
      await Promise.resolve();
    });

    expect(onEquationChange).not.toHaveBeenCalled();
    expect(stripZeroWidth((container.querySelector('.notesEditor') as HTMLElement).textContent)).not.toContain('hello');
    expect(buttonByText(container, 'Cancel').disabled).toBe(true);
  });

  it('undo (Cmd+Z) after Cancel does not resurrect the discarded text', async () => {
    // The editors are withHistory: without clearing history, the discard's own
    // transforms are recorded, so one undo would invert the revert and bring the
    // abandoned text back -- and a later blur would then commit it.
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b'));
    const unitsEditor = editorFor(container, '.unitsEditor');
    await appendText(unitsEditor, 'kg');

    const cancel = buttonByText(container, 'Cancel');
    await act(async () => {
      fireEvent.blur(container.querySelector('.unitsEditor') as Element, { relatedTarget: cancel });
      fireEvent.click(cancel);
      await Promise.resolve();
    });

    // Drive the Slate history undo directly (jsdom cannot deliver the Cmd+Z
    // chord through slate-react's input pipeline).
    await act(async () => {
      HistoryEditor.undo(unitsEditor as unknown as HistoryEditor);
      unitsEditor.onChange();
      await Promise.resolve();
    });

    expect(stripZeroWidth((container.querySelector('.unitsEditor') as HTMLElement).textContent)).not.toContain('kg');

    // A blur-away after the undo must not commit resurrected text.
    await act(async () => {
      fireEvent.blur(container.querySelector('.unitsEditor') as Element, { relatedTarget: null });
      await Promise.resolve();
    });
    expect(onEquationChange).not.toHaveBeenCalled();
  });

  it('intra-panel blur keeps the raw editor open; blur away collapses to preview and commits', async () => {
    const { container, onEquationChange } = renderDetails(makeAux('x', 'a + b'));
    await openEquationEditorViaPreview(container);
    const editor = editorFor(container, '.eqnEditor');
    await appendText(editor, 'Z');

    // Blur toward another field inside the panel: no commit, and the raw editor
    // stays open showing the pending edit rather than snapping to the preview.
    const units = container.querySelector('.unitsEditor') as HTMLElement;
    await act(async () => {
      fireEvent.blur(container.querySelector('.eqnEditor') as Element, { relatedTarget: units });
      await Promise.resolve();
    });
    expect(container.querySelector('.eqnEditor')).not.toBeNull();
    expect(container.querySelector('.eqnPreview')).toBeNull();
    expect((container.querySelector('.eqnEditor') as HTMLElement).textContent).toBe('a + bZ');
    expect(onEquationChange).not.toHaveBeenCalled();

    // Blur out of the panel: commits and collapses back to the preview.
    await act(async () => {
      fireEvent.blur(container.querySelector('.eqnEditor') as Element, { relatedTarget: null });
      await Promise.resolve();
    });
    expect(container.querySelector('.eqnEditor')).toBeNull();
    expect(container.querySelector('.eqnPreview')).not.toBeNull();
    expect(onEquationChange).toHaveBeenCalledTimes(1);
    expect(onEquationChange).toHaveBeenCalledWith('x', 'a + bZ', undefined, undefined);
  });
});
