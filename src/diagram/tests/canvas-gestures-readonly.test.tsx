// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Reconciler-level gesture tests for the Canvas `readOnly` prop (issue #935).
//
// The mutation gating itself lives in the Editor (it hands a read-only Canvas
// no-op callbacks and no selectedTool); what Canvas owns is the one editing
// entry point it opens ITSELF -- the inline label editor on a label
// double-click. Without the prop that editor still opened while the eventual
// onRenameVariable commit silently no-op'd: the exact "editable but unsavable"
// deception the issue is about. These tests pin that the editor never opens
// read-only, while selection (a read capability) still works and gestures
// still raise their callbacks (the host decides what they do).

import { describe, it, expect } from '@rstest/core';

import { act, fireEvent } from '@testing-library/react';

import { makeAux, pointerDown, pointerMove, pointerUp, renderCanvas } from './canvas-gesture-harness';

describe('Canvas gestures: readOnly', () => {
  it('double-clicking a variable name does NOT open the inline label editor', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], readOnly: true });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 130, clientY: 100 });
    });
    h.setProps({ selection: new Set([10]) });

    expect(h.query('[contenteditable]')).toBeNull();
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
  });

  it('double-clicking an ALREADY-SELECTED variable name does not open the editor either', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]), readOnly: true });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 130, clientY: 100 });
    });
    h.setProps({ selection: new Set([10]) });

    expect(h.query('[contenteditable]')).toBeNull();
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
  });

  it('clicking an element still selects it (selection is a read capability)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], readOnly: true });
    h.clearMountCalls();

    const aux = h.query('.simlin-aux')!;
    pointerDown(aux, 100, 100);
    pointerUp(aux, 100, 100);

    expect(h.callbacks.onSetSelection).toHaveBeenCalled();
    const lastCall = h.callbacks.onSetSelection.mock.calls.at(-1)![0] as ReadonlySet<number>;
    expect([...lastCall]).toEqual([10]);
  });

  it('a drag still raises onMoveSelection -- the host decides it is a no-op', () => {
    // Deliberate layering: Canvas raises the gesture callback; the Editor
    // substitutes a no-op when read-only. Pinning this keeps the gate's
    // location honest (Editor-side, not silently duplicated in Canvas).
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]), readOnly: true });
    h.clearMountCalls();

    const aux = h.query('.simlin-aux')!;
    pointerDown(aux, 100, 100);
    pointerMove(aux, 200, 200, { buttons: 1 });
    pointerUp(h.svg, 200, 200);

    expect(h.callbacks.onMoveSelection).toHaveBeenCalled();
  });
});
