/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// EditableLabel key handling: plain Enter commits (and its keyDown is
// prevented so Slate never inserts a line break for it), shift+Enter is the
// line-break gesture and must NOT commit, ctrl+Enter still commits (legacy
// chord), Escape cancels. This pins the standard-editor convention adopted
// after the audit found the inverted mapping (commit only on modifier+Enter)
// left every "type name, press Enter" edit staging a trailing newline.

import * as React from 'react';
import { render, fireEvent } from '@testing-library/react';

// jsdom does not implement isContentEditable, but slate-react's keyDown
// pipeline gates on ReactEditor.hasEditableTarget -> element.isContentEditable
// before forwarding to our onKeyDown. Without this polyfill the keyDown
// handler (which prevents Slate's default line-break on plain Enter) is
// unreachable in tests. (keyUp is passed through unwrapped, so it needs no
// polyfill.)
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
});

import { EditableLabel } from '../drawing/EditableLabel';
import { plainDeserialize } from '../drawing/common';

function renderLabel(): { editable: Element; onDone: jest.Mock; onChange: jest.Mock } {
  const onDone = jest.fn();
  const onChange = jest.fn();
  const value = plainDeserialize('label', 'some name');
  const { container } = render(
    <EditableLabel
      uid={1}
      cx={100}
      cy={100}
      side="bottom"
      rw={9}
      rh={9}
      zoom={1}
      value={value}
      onChange={onChange}
      onDone={onDone}
    />,
  );
  const editable = container.querySelector('[contenteditable]');
  expect(editable).not.toBeNull();
  // Slate's composed onKeyDown only forwards to our handler when the editor
  // believes it is focused; jsdom needs an explicit focus event for that.
  fireEvent.focus(editable as Element);
  return { editable: editable as Element, onDone, onChange };
}

describe('EditableLabel key handling', () => {
  it('commits on plain Enter and prevents the default line-break insertion', () => {
    const { editable, onDone } = renderLabel();

    // fireEvent returns false when preventDefault was called.
    const keyDownNotPrevented = fireEvent.keyDown(editable, { code: 'Enter' });
    expect(keyDownNotPrevented).toBe(false);

    fireEvent.keyUp(editable, { code: 'Enter' });
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onDone).toHaveBeenCalledWith(false);
  });

  it('commits on NumpadEnter too', () => {
    const { editable, onDone } = renderLabel();
    fireEvent.keyDown(editable, { code: 'NumpadEnter' });
    fireEvent.keyUp(editable, { code: 'NumpadEnter' });
    expect(onDone).toHaveBeenCalledWith(false);
  });

  it('does not commit on shift+Enter (the line-break gesture)', () => {
    const { editable, onDone } = renderLabel();

    // keyDown must NOT be prevented: Slate's default insertBreak adds the line.
    const keyDownNotPrevented = fireEvent.keyDown(editable, { code: 'Enter', shiftKey: true });
    expect(keyDownNotPrevented).toBe(true);

    fireEvent.keyUp(editable, { code: 'Enter', shiftKey: true });
    expect(onDone).not.toHaveBeenCalled();
  });

  it('still commits on ctrl+Enter (legacy chord)', () => {
    const { editable, onDone } = renderLabel();
    fireEvent.keyDown(editable, { code: 'Enter', ctrlKey: true });
    fireEvent.keyUp(editable, { code: 'Enter', ctrlKey: true });
    expect(onDone).toHaveBeenCalledWith(false);
  });

  it('cancels on Escape', () => {
    const { editable, onDone } = renderLabel();
    fireEvent.keyUp(editable, { code: 'Escape' });
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onDone).toHaveBeenCalledWith(true);
  });
});
