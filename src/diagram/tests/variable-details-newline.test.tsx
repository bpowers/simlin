/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Cmd+Enter (macOS "Apple enter") and Ctrl+Enter must insert a line break in
// the multi-line equation and documentation editors, matching the Shift+Enter
// gesture Slate already provides. Slate's default key map ignores those chords
// (is-hotkey requires unlisted modifiers to be absent), so VariableDetails
// intercepts them and calls Editor.insertSoftBreak itself. These tests pin that
// wiring: the chord routes through insertSoftBreak, while plain Enter does not
// (Slate splits the block via insertBreak instead).

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

// jsdom does not implement isContentEditable, but slate-react's keyDown
// pipeline gates on ReactEditor.hasEditableTarget -> element.isContentEditable
// before forwarding to our onKeyDown. Without this polyfill the handler is
// unreachable in tests.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
});

import * as React from 'react';
import { render, fireEvent } from '@testing-library/react';
import * as Slate from 'slate';
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

const noop = () => {};

// An equation error forces the raw editor open (showPreview gating), so the
// contenteditable equation field is present to receive key events.
const forceEditorOpen: EquationError[] = [{ start: 0, end: 1, code: 0 as unknown as ErrorCode }];

function renderDetails(variable: Aux) {
  return render(
    <VariableDetails
      variable={variable}
      viewElement={makeViewElement(variable.ident)}
      onDelete={noop}
      onEquationChange={noop}
      onTableChange={noop}
      activeTab={0}
      onActiveTabChange={noop}
    />,
  );
}

function focusEditor(container: HTMLElement, selector: string): Element {
  const editable = container.querySelector(`${selector} [contenteditable]`) ?? container.querySelector(selector);
  expect(editable).not.toBeNull();
  // Slate's composed onKeyDown only forwards to our handler when the editor
  // believes it is focused; jsdom needs an explicit focus event for that.
  fireEvent.focus(editable as Element);
  return editable as Element;
}

describe('VariableDetails newline chord', () => {
  let softBreak: jest.SpyInstance;

  beforeEach(() => {
    // Spy (not mock-through) so no-op splitNodes-without-selection never runs;
    // we only care that the chord routed here.
    softBreak = jest.spyOn(Slate.Editor, 'insertSoftBreak').mockImplementation(() => {});
  });

  afterEach(() => {
    softBreak.mockRestore();
  });

  describe('equation editor', () => {
    it('inserts a soft break on Cmd+Enter', () => {
      const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
      const editor = focusEditor(container, '.eqnEditor');

      const notPrevented = fireEvent.keyDown(editor, { key: 'Enter', metaKey: true });
      expect(notPrevented).toBe(false); // our handler called preventDefault
      expect(softBreak).toHaveBeenCalledTimes(1);
    });

    it('inserts a soft break on Ctrl+Enter', () => {
      const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
      const editor = focusEditor(container, '.eqnEditor');

      fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true });
      expect(softBreak).toHaveBeenCalledTimes(1);
    });

    it('does not route plain Enter through the soft-break path', () => {
      const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
      const editor = focusEditor(container, '.eqnEditor');

      fireEvent.keyDown(editor, { key: 'Enter' });
      expect(softBreak).not.toHaveBeenCalled();
    });

    it('leaves Escape to close the editor, not insert a break', () => {
      const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
      const editor = focusEditor(container, '.eqnEditor');

      fireEvent.keyDown(editor, { key: 'Escape' });
      expect(softBreak).not.toHaveBeenCalled();
    });
  });

  describe('documentation editor', () => {
    it('inserts a soft break on Cmd+Enter', () => {
      const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
      const editor = focusEditor(container, '.notesEditor');

      fireEvent.keyDown(editor, { key: 'Enter', metaKey: true });
      expect(softBreak).toHaveBeenCalledTimes(1);
    });

    it('does not route plain Enter through the soft-break path', () => {
      const { container } = renderDetails(makeAux('x', 'a + b', { errors: forceEditorOpen }));
      const editor = focusEditor(container, '.notesEditor');

      fireEvent.keyDown(editor, { key: 'Enter' });
      expect(softBreak).not.toHaveBeenCalled();
    });
  });
});
