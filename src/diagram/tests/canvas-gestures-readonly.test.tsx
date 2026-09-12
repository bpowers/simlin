// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Reconciler-level gesture tests for the Canvas `readOnly` prop (issue #935).
//
// The Editor hands a read-only Canvas a no-op commit callback and no
// selectedTool. The Canvas owns two things itself: the inline label editor a
// label double-click opens, which must never open read-only (otherwise it LOOKS
// editable while the eventual onRenameVariable commit silently no-ops, the
// deception the issue is about), and the drag preview, which must not show a
// move the release could never commit. Selection, a read capability, still
// works.

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

  it('a drag previews no move and commits nothing (audit M9)', () => {
    // The Editor also hands a read-only Canvas a no-op commit callback; the
    // planner's own read-only arm keeps the preview from showing a move the
    // release could never commit.
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]), readOnly: true });
    h.clearMountCalls();

    const aux = h.query('.simlin-aux')!;
    pointerDown(aux, 100, 100);
    pointerMove(aux, 200, 200, { buttons: 1 });
    expect(h.query('.simlin-aux circle')?.getAttribute('cx')).toBe('100');
    pointerUp(h.svg, 200, 200);

    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });
});
