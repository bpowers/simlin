/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Verifies the onSelectionChanged prop fires from componentDidUpdate
// whenever the committed selection changes, with the canonical-ident array.
//
// The callback used to be deferred via setTimeout(0) inside handleSelection
// so React could commit the new selection before getSelectionIdents() read
// `this.state.selection`. It now fires from componentDidUpdate, which React
// only invokes *after* the commit -- so the read is already against fresh
// state and no deferral (or unmount-time timer cancellation) is needed.
// This also makes the callback fire for selection changes that never went
// through handleSelection (deletion clearing the selection, module
// drill-in/back resetting it, undo/redo resets).
//
// Uses `new Editor(props)` to install the arrow-function instance fields,
// then shims state/setState so we can drive componentDidUpdate without the
// React reconciler.

import { Editor } from '../Editor';

type EditorInstance = InstanceType<typeof Editor>;

function makeEditor(args: {
  onSelectionChanged?: (idents: string[]) => void;
  selectionIdents: string[];
}): EditorInstance {
  const onSave = jest.fn().mockResolvedValue(1);
  const props = {
    inputFormat: 'json' as const,
    initialProjectJson: '{}',
    initialProjectVersion: 0,
    name: 'test-project',
    onSave,
    onSelectionChanged: args.onSelectionChanged,
  };

  const editor = new Editor(props);

  // Replace the React-attached state/setState with a mutable shim so we can
  // observe state assignments without the React reconciler.
  // Object.defineProperty bypasses the readonly typing on `state`.
  Object.defineProperty(editor, 'state', {
    value: {
      ...editor.state,
      selection: new Set<number>(),
      flowStillBeingCreated: false,
      variableDetailsActiveTab: 0,
      showDetails: undefined,
      modelName: 'main',
    },
    writable: true,
    configurable: true,
  });

  editor.setState = ((updater: unknown) => {
    const next = typeof updater === 'function' ? (updater as (s: unknown) => unknown)(editor.state) : updater;
    Object.assign(editor.state, next);
  }) as EditorInstance['setState'];

  // Stub getSelectionIdents so the test doesn't need a full StockFlowView.
  // The real method's wiring (state.selection -> view.elements) is exercised
  // by editor-applyPatch.test.ts and the Canvas integration tests.
  editor.getSelectionIdents = () => args.selectionIdents;
  // refreshCachedErrors is invoked from componentDidUpdate on a modelName
  // change; stub it so the selection-focused tests don't need an engine.
  editor.refreshCachedErrors = jest.fn().mockResolvedValue(undefined) as EditorInstance['refreshCachedErrors'];

  return editor;
}

// Drive a committed selection change the way React would: mutate state, then
// invoke componentDidUpdate with the prior state.
function commitSelection(editor: EditorInstance, selection: ReadonlySet<number>): void {
  const prevState = { ...editor.state };
  editor.handleSelection(selection);
  editor.componentDidUpdate(editor.props, prevState);
}

describe('Editor componentDidUpdate -> onSelectionChanged', () => {
  it('invokes onSelectionChanged with the canonical idents after the selection commits', () => {
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['teacup_temperature', 'ambient_temperature'],
    });

    commitSelection(editor, new Set<number>([1, 2]));

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith(['teacup_temperature', 'ambient_temperature']);
  });

  it('passes an empty array when the selection becomes empty', () => {
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['x'],
    });

    // First commit a non-empty selection so clearing it is a real change.
    commitSelection(editor, new Set<number>([1]));
    callback.mockClear();
    editor.getSelectionIdents = () => [];

    commitSelection(editor, new Set<number>());

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith([]);
  });

  it('does not fire when the committed selection is unchanged', () => {
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['x'],
    });

    // componentDidUpdate with prevState.selection identical to the current
    // selection (e.g. an unrelated state change re-rendered the component)
    // must not re-notify the host.
    const sameSelection = new Set<number>([1]);
    Object.assign(editor.state, { selection: sameSelection });
    editor.componentDidUpdate(editor.props, { ...editor.state, selection: sameSelection });

    expect(callback).not.toHaveBeenCalled();
  });

  it('does not fire when prev and current selections have equal content but distinct Set identities', () => {
    // undo/navigate-back rebuild a fresh Set with the same contents (the
    // restoredSelection scenario). The guard uses setsEqual, not reference
    // equality, so a content-identical but distinct Set must not re-notify
    // the host -- otherwise every undo with a non-empty selection would emit
    // a spurious callback.
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['x'],
    });

    const prevSelection = new Set<number>([1, 2]);
    Object.assign(editor.state, { selection: prevSelection });
    const prevState = { ...editor.state };
    // A new Set instance with identical contents.
    Object.assign(editor.state, { selection: new Set<number>([1, 2]) });
    editor.componentDidUpdate(editor.props, prevState);

    expect(callback).not.toHaveBeenCalled();
  });

  it('does not throw when onSelectionChanged is omitted', () => {
    const editor = makeEditor({
      onSelectionChanged: undefined,
      selectionIdents: ['x'],
    });

    expect(() => {
      commitSelection(editor, new Set<number>([1]));
    }).not.toThrow();
  });

  it('notifies when the selection is cleared by a non-handleSelection path (e.g. deletion)', () => {
    // The deliberate semantic improvement: handleSelectionDelete clears the
    // selection directly via setState, never routing through handleSelection.
    // componentDidUpdate still observes the committed change and notifies the
    // host -- previously this case sent no callback at all.
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: [],
    });

    // Simulate a prior non-empty selection, then a delete that clears it.
    Object.assign(editor.state, { selection: new Set<number>([1, 2]) });
    const prevState = { ...editor.state };
    Object.assign(editor.state, { selection: new Set<number>() });
    editor.componentDidUpdate(editor.props, prevState);

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith([]);
  });

  it('reads idents at fire time so the host sees the committed state', () => {
    // getSelectionIdents reads the committed state; componentDidUpdate runs
    // after the commit, so whatever the method returns at that point is what
    // the host observes.
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['initial'],
    });

    editor.getSelectionIdents = () => ['committed'];
    commitSelection(editor, new Set<number>([1]));

    expect(callback).toHaveBeenCalledWith(['committed']);
  });
});
