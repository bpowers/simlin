/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Verifies the onSelectionChanged prop fires after handleSelection commits
// state, with the canonical-ident array. Uses `new Editor(props)` to install
// the arrow-function instance fields (handleSelection lives on the instance,
// not the prototype, so Object.create wouldn't expose it). The constructor's
// deferred `openInitialProject()` and other async work is held back by
// jest.useFakeTimers — none of that machinery is required for the
// handleSelection -> onSelectionChanged path under test.

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

  return editor;
}

describe('Editor.handleSelection -> onSelectionChanged', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('invokes onSelectionChanged with the canonical idents after the selection commits', () => {
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['teacup_temperature', 'ambient_temperature'],
    });

    editor.handleSelection(new Set<number>([1, 2]));

    // The callback is deferred via setTimeout(0) so the state commit
    // happens before the host receives the callback.
    expect(callback).not.toHaveBeenCalled();

    jest.advanceTimersByTime(0);

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith(['teacup_temperature', 'ambient_temperature']);
  });

  it('passes an empty array when the selection becomes empty', () => {
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: [],
    });

    editor.handleSelection(new Set<number>());

    jest.advanceTimersByTime(0);

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith([]);
  });

  it('does not throw when onSelectionChanged is omitted', () => {
    const editor = makeEditor({
      onSelectionChanged: undefined,
      selectionIdents: ['x'],
    });

    expect(() => {
      editor.handleSelection(new Set<number>([1]));
    }).not.toThrow();

    // Advancing timers must also not throw — i.e. no scheduled callback
    // attempted to run on `undefined`.
    expect(() => {
      jest.advanceTimersByTime(0);
    }).not.toThrow();
  });

  it('cancels the deferred callback on unmount so a remount does not see stale idents', () => {
    // EditorHost keys the Editor by `${path}#${loadGeneration}`, so a path
    // swap unmounts the current Editor and mounts a fresh one. The
    // setTimeout(0) deferral must not fire after componentWillUnmount,
    // or the unmounted Editor's onSelectionChanged callback runs against
    // a host that has since switched paths — re-introducing the
    // stale-idents-on-new-path bug the EditorHost-side fix already
    // closed for the debounce window.
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['stale_ident'],
    });

    editor.handleSelection(new Set<number>([1]));

    // componentWillUnmount also clears the document keydown listener,
    // which doesn't exist under @jest-environment node — stub it so we
    // can call the lifecycle hook without bringing in jsdom.
    const documentStub = {
      removeEventListener: () => {},
      addEventListener: () => {},
    } as unknown as Document;
    (globalThis as { document?: Document }).document = documentStub;
    try {
      editor.componentWillUnmount();
    } finally {
      delete (globalThis as { document?: Document }).document;
    }

    jest.advanceTimersByTime(0);

    expect(callback).not.toHaveBeenCalled();
  });

  it('reads idents at fire time so the host sees the committed state', () => {
    // React 19's setState is asynchronous — `this.state.selection` is not
    // updated synchronously when handleSelection's setState call returns.
    // The deferred setTimeout ensures the callback fires AFTER the state
    // commit, and getSelectionIdents() inside the deferred closure reads
    // the fresh state. We verify this contract by mutating the
    // getSelectionIdents stub between scheduling and firing: the callback
    // observes the value at fire time.
    const callback = jest.fn();
    const editor = makeEditor({
      onSelectionChanged: callback,
      selectionIdents: ['initial'],
    });

    editor.handleSelection(new Set<number>([1]));

    editor.getSelectionIdents = () => ['committed'];

    jest.advanceTimersByTime(0);

    expect(callback).toHaveBeenCalledWith(['committed']);
  });
});
