/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Reconciler-level gesture tests for the just-created-flow name-edit lifecycle
// of the React `Canvas`. These replace the old internals-poking
// canvas-editing-name-done.test.ts: instead of constructing the class and
// driving `handleEditingNameDone` directly, they drive real flow-tool creation
// gestures through the harness and assert only on prop-callback payloads and
// rendered DOM. They survive the class->hooks conversion unchanged and pin the
// `creatingFlow` (formerly `flowStillBeingCreated`) state machine:
//
//   1. Cancelling the initial name edit of a just-created flow deletes it.
//   2. Committing the initial flow name does NOT delete it and clears the
//      creatingFlow latch.
//   3. After committing a created flow's name, a LATER rename-cancel of an
//      unrelated variable must NOT fire onDeleteSelection -- i.e. the
//      creatingFlow latch did not leak across editing sessions.
//
// The third scenario is the load-bearing regression: a stale `true` latch would
// make an ordinary Escape-cancel of an unrelated rename delete that variable.

import { act, fireEvent } from '@testing-library/react';

import type { StockFlowView, ViewElement } from '@simlin/core/datamodel';

import {
  makeAux,
  makeCloud,
  makeFlow,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
  type CanvasHarness,
} from './canvas-gesture-harness';

// Materialize a concrete flow + source/sink clouds when the flow-tool drag
// releases, mirroring Editor.handleFlowAttach: it commits the real flow (uid 50)
// into the view and selects it. `extraElements` lets a test keep an unrelated
// pre-existing variable in the view alongside the new flow.
function installFlowMaterializer(h: CanvasHarness, extraElements: readonly ViewElement[] = []): void {
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
    const elements = [...extraElements, source, sink, flow];
    const view: StockFlowView = {
      nextUid: 60,
      elements,
      viewBox: { x: 0, y: 0, width: 1000, height: 1000 },
      zoom: 1,
      useLetteredPolarity: false,
    };
    h.setProps({ view, selection: new Set([50]) });
  });
}

// Drive the flow tool from empty canvas through to the on-screen name editor.
// Returns the live contenteditable node for the just-created flow's name edit.
function createFlowAndEnterNameEdit(h: CanvasHarness): Element {
  pointerDown(h.svg, 200, 200);
  pointerMove(h.svg, 300, 200, { buttons: 1 });
  pointerUp(h.svg, 300, 200);

  const editable = h.query('[contenteditable]');
  expect(editable).not.toBeNull();
  return editable!;
}

describe('Canvas name-edit lifecycle: just-created flow', () => {
  it('cancelling the initial name edit of a just-created flow deletes it (checklist 12)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    installFlowMaterializer(h);
    h.clearMountCalls();

    const editable = createFlowAndEnterNameEdit(h);

    act(() => {
      fireEvent.keyUp(editable, { code: 'Escape' });
    });

    // The flow-creation cancel deletes the just-created flow exactly once.
    expect(h.callbacks.onDeleteSelection).toHaveBeenCalledTimes(1);
    // The inline editor is gone afterward (clearPointerState ended editing).
    expect(h.query('[contenteditable]')).toBeNull();
  });

  it('committing the initial flow name does NOT delete it and clears the latch', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    installFlowMaterializer(h);
    h.clearMountCalls();

    const editable = createFlowAndEnterNameEdit(h);

    act(() => {
      // EditableLabel commits on plain Enter (keyDown is prevented so Slate
      // doesn't insert a line break; keyUp performs the commit).
      fireEvent.keyDown(editable, { code: 'Enter' });
      fireEvent.keyUp(editable, { code: 'Enter' });
    });

    // Committing renames the flow (uid 50 != inCreationUid, so it is an existing
    // element by now) and must NOT delete it.
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
    // The editor closes on commit.
    expect(h.query('[contenteditable]')).toBeNull();
  });

  it('a later rename-cancel of an unrelated variable does NOT delete it (no latch leak)', () => {
    // An unrelated variable that survives the flow creation and is renamed later.
    const existing = makeAux(9, 'Existing Variable', 600, 600);
    const h = renderCanvas({ elements: [existing], selectedTool: 'flow' });
    installFlowMaterializer(h, [existing]);
    h.clearMountCalls();

    // 1. Create a flow and COMMIT its name. This clears the creatingFlow latch.
    const flowEditable = createFlowAndEnterNameEdit(h);
    act(() => {
      fireEvent.keyDown(flowEditable, { code: 'Enter' });
      fireEvent.keyUp(flowEditable, { code: 'Enter' });
    });
    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();

    // Drop the flow tool now that creation finished, mirroring the host clearing
    // the active tool after a create commit.
    h.setProps({ selectedTool: undefined });

    // 2. Double-click the unrelated variable to enter a plain rename edit.
    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 630, clientY: 600 });
    });
    h.setProps({ selection: new Set([9]) });
    const renameEditable = h.query('[contenteditable]');
    expect(renameEditable).not.toBeNull();

    // 3. Escape-cancel that rename. A leaked creatingFlow latch would delete the
    // variable here; it must not.
    act(() => {
      fireEvent.keyUp(renameEditable!, { code: 'Escape' });
    });

    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1); // only the flow commit
  });

  it('shift+Enter does NOT commit (it inserts a line break instead)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    installFlowMaterializer(h);
    h.clearMountCalls();

    const editable = createFlowAndEnterNameEdit(h);

    act(() => {
      fireEvent.keyDown(editable, { code: 'Enter', shiftKey: true });
      fireEvent.keyUp(editable, { code: 'Enter', shiftKey: true });
    });

    // No commit, no delete: the editor stays open for the second line.
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
    expect(h.query('[contenteditable]')).not.toBeNull();
  });
});
