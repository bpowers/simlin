/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the create-flow name-edit crash (issue #820 / the
// getElementByUid crash surfaced during the #819 review).
//
// When a just-drawn flow's attach patch fails, the host (Editor) does not
// commit the flow into the view, yet the Canvas has already handed off to the
// just-created-flow name edit. Its `props.selection` then references a flow
// that is not in the view. The render already tolerates that (it resolves the
// editing element through the NON-throwing tryGetElementByUid and skips the
// editor), but `handleEditingNameDone` used the THROWING getElementByUid, so
// any path that fired it against the phantom selection -- notably the deferred
// editing-done scheduled when the active tool changes -- threw
// `expected non-undefined object` and wedged the editor in a repeated-exception
// loop, making the whole editor appear broken.
//
// Two paths reach handleEditingNameDone:
//   - the COMMIT path (a non-empty name, e.g. the deferred tool-change), which
//     resolved the selected element -- this is where the crash lived; and
//   - the CANCEL path (Escape or an empty-name commit -> creatingFlow ->
//     onDeleteSelection), which never dereferences the element.
// The first test pins the commit-path crash fix against a phantom; the second
// pins that the cancel/delete-on-cancel path stays benign (single-fire, no
// throw). A cancel can only be issued against a RENDERED editor, which only
// renders when the element resolves -- so a cancel is never issued against an
// unresolved phantom, and the cancel branch is dereference-free regardless.

import { act, fireEvent } from '@testing-library/react';

import type { StockFlowView } from '@simlin/core/datamodel';

import { makeCloud, makeFlow, pointerDown, pointerMove, pointerUp, renderCanvas } from './canvas-gesture-harness';

// Drain one macrotask so the render-scheduled deferred editing-done runs.
async function flushDeferred(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

// Capture any uncaught error dispatched to window (a throw inside the deferred
// editing-done timer surfaces here rather than at the call site).
function captureWindowErrors(): { errors: unknown[]; stop: () => void } {
  const errors: unknown[] = [];
  const onError = (e: ErrorEvent): void => {
    errors.push(e.error ?? e.message);
  };
  window.addEventListener('error', onError);
  return { errors, stop: () => window.removeEventListener('error', onError) };
}

describe('Canvas name-edit teardown for a just-created flow', () => {
  it('the deferred editing-done does not crash on a phantom selection (commit path)', async () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    // Model the failed-attach path: the host neither commits the drawn flow
    // into the view nor selects a committed element, so the Canvas's selection
    // is left pointing at the (now-cleared) in-creation flow.
    h.callbacks.onMoveFlow.mockImplementation(() => {});
    h.clearMountCalls();

    // Draw the flow. Pointer-up hands off into the just-created-flow name edit
    // (interaction = editingName/creatingFlow). The editor itself does not
    // render because the selected element is unresolvable -- exactly the phantom
    // state a failed attach leaves behind.
    pointerDown(h.svg, 200, 200);
    pointerMove(h.svg, 300, 200, { buttons: 1 });
    pointerUp(h.svg, 300, 200);

    // Changing the active tool schedules the deferred editing-done, which
    // resolves the selection with a non-empty name (the commit path). Before the
    // fix this threw from getElementByUid.
    const capture = captureWindowErrors();
    try {
      h.setProps({ selectedTool: undefined });
      await flushDeferred();
    } finally {
      capture.stop();
    }

    expect(capture.errors).toEqual([]);
    // Nothing to commit against a phantom: no rename, no create -- and, because
    // this is the commit path (non-empty name), no delete either.
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
    expect(h.callbacks.onCreateVariable).not.toHaveBeenCalled();
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
    // The editor is torn down (no lingering contenteditable).
    expect(h.query('[contenteditable]')).toBeNull();
  });

  it('cancelling a just-created flow name edit fires exactly one delete and never throws (cancel path)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    // Success materializer: commit the drawn flow so the name editor renders for
    // a real element. A cancel can only be issued against a rendered editor, so
    // this is the only way to exercise the genuine cancel/delete-on-cancel path.
    h.callbacks.onMoveFlow.mockImplementation(() => {
      const source = makeCloud(51, 50, 200, 200);
      const sink = makeCloud(52, 50, 300, 200);
      const flow = makeFlow(
        50,
        'New Flow',
        [
          { x: 200, y: 200, attachedToUid: 51 },
          { x: 300, y: 200, attachedToUid: 52 },
        ],
        { x: 250, y: 200 },
      );
      const view: StockFlowView = {
        nextUid: 60,
        elements: [source, sink, flow],
        viewBox: { x: 0, y: 0, width: 1000, height: 1000 },
        zoom: 1,
        useLetteredPolarity: false,
      };
      h.setProps({ view, selection: new Set([50]) });
    });
    h.clearMountCalls();

    pointerDown(h.svg, 200, 200);
    pointerMove(h.svg, 300, 200, { buttons: 1 });
    pointerUp(h.svg, 300, 200);
    const editable = h.query('[contenteditable]');
    expect(editable).not.toBeNull();

    const capture = captureWindowErrors();
    try {
      act(() => {
        fireEvent.keyUp(editable!, { code: 'Escape' });
      });
    } finally {
      capture.stop();
    }

    expect(capture.errors).toEqual([]);
    // Cancelling the initial name edit of a just-created flow deletes it EXACTLY
    // once. The cancel branch calls onDeleteSelection without dereferencing the
    // element, so it stays benign for any target and cannot double-fire.
    expect(h.callbacks.onDeleteSelection).toHaveBeenCalledTimes(1);
    // No rename/create on a cancel, and the editor is torn down.
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
    expect(h.callbacks.onCreateVariable).not.toHaveBeenCalled();
    expect(h.query('[contenteditable]')).toBeNull();
  });
});
