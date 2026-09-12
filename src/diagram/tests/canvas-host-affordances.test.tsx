// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The three Canvas affordances the host's edit queue relies on, driven through
// real pointer and key events on a rendered Canvas:
//
//  - `pressesDisabled`: while the host has an undo or redo queued, a press on
//    empty canvas, on an element, or on a label starts nothing -- a gesture
//    planned on the view the undo replaces could not commit;
//  - `newVariableName`: a creation tool takes its default name from the host,
//    which allocates against pending creates;
//  - a refused name: `onCreateVariable`/`onRenameVariable` returning a message
//    keeps the inline name editor open showing it, and an accepted retry closes
//    it.

import { describe, it, expect } from '@rstest/core';

import { act, fireEvent } from '@testing-library/react';

import {
  makeAux,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
  type CanvasHarness,
} from './canvas-gesture-harness';

function commitName(h: CanvasHarness): void {
  const editable = h.query('[contenteditable]');
  expect(editable).not.toBeNull();
  act(() => {
    fireEvent.keyDown(editable!, { code: 'Enter' });
    fireEvent.keyUp(editable!, { code: 'Enter' });
  });
}

describe('Canvas pressesDisabled', () => {
  it('a press on empty canvas with a creation tool armed stages nothing', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)], selectedTool: 'aux', pressesDisabled: true });
    h.clearMountCalls();
    pointerDown(h.svg, 300, 300);
    pointerMove(h.svg, 320, 320, { buttons: 1 });
    pointerUp(h.svg, 320, 320);
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
    expect(h.callbacks.onCreateVariable).not.toHaveBeenCalled();
    expect(h.query('[contenteditable]')).toBeNull();
  });

  it('a press and drag on an element neither selects nor moves it', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)], pressesDisabled: true });
    h.clearMountCalls();
    const aux = h.query('.simlin-aux circle')!;
    pointerDown(aux, 100, 100);
    pointerMove(h.svg, 160, 160, { buttons: 1 });
    pointerUp(h.svg, 160, 160);
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('control: the same press and drag moves the element when presses are enabled', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)] });
    h.clearMountCalls();
    const aux = h.query('.simlin-aux circle')!;
    pointerDown(aux, 100, 100);
    pointerMove(h.svg, 160, 160, { buttons: 1 });
    pointerUp(h.svg, 160, 160);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  });
});

describe('Canvas newVariableName', () => {
  it("a creation tool takes the host's default name", () => {
    const h = renderCanvas({ elements: [], selectedTool: 'aux', newVariableName: (base) => `${base} 7` });
    h.clearMountCalls();
    pointerDown(h.svg, 200, 200);
    pointerUp(h.svg, 200, 200);
    commitName(h);
    expect(h.callbacks.onCreateVariable).toHaveBeenCalledTimes(1);
    expect(h.callbacks.onCreateVariable.mock.calls[0][0].name).toBe('New Variable 7');
  });
});

describe('Canvas name refusal', () => {
  it('a refused create keeps the name editor open with the message; an accepted retry closes it', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'aux' });
    h.clearMountCalls();
    h.callbacks.onCreateVariable.mockReturnValueOnce("A variable named 'New Variable' already exists");
    pointerDown(h.svg, 200, 200);
    pointerUp(h.svg, 200, 200);

    commitName(h);
    expect(h.callbacks.onCreateVariable).toHaveBeenCalledTimes(1);
    expect(h.query('[contenteditable]')).not.toBeNull();
    expect(h.query('[role="alert"]')?.textContent).toBe("A variable named 'New Variable' already exists");

    commitName(h);
    expect(h.callbacks.onCreateVariable).toHaveBeenCalledTimes(2);
    expect(h.query('[contenteditable]')).toBeNull();
    expect(h.query('[role="alert"]')).toBeNull();
  });

  it('a refused rename keeps the name editor open with the message', () => {
    const h = renderCanvas({ elements: [makeAux(9, 'Existing Variable', 600, 600)] });
    h.clearMountCalls();
    h.callbacks.onRenameVariable.mockReturnValue("A variable named 'a' already exists");
    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 630, clientY: 600 });
    });
    h.setProps({ selection: new Set([9]) });
    const editable = h.query('[contenteditable]');
    expect(editable).not.toBeNull();
    // The Canvas hands every commit to the host (the host decides that an
    // unchanged name is a no-op), so committing the seeded name is enough to
    // exercise the refusal path.
    commitName(h);
    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
    expect(h.query('[contenteditable]')).not.toBeNull();
    expect(h.query('[role="alert"]')?.textContent).toBe("A variable named 'a' already exists");
  });
});
