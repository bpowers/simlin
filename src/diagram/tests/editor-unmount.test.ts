/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the Editor's controller lifecycle.
//
// The engine lifecycle, the deferred sim/save/undo dispatch, and the
// orphan-disposal-on-race machinery all moved into ProjectController (see
// project-controller.ts and project-controller.test.ts). The Editor's only
// remaining lifecycle responsibility is to:
//
//   - create a controller in componentDidMount and kick off
//     openInitialProject() (the controller guards its own dispose-races),
//   - subscribe to controller snapshots,
//   - dispose the controller (which releases the WASM EngineProject handle)
//     and unsubscribe in componentWillUnmount.
//
// A mount -> unmount -> mount cycle on the same instance (React 18 StrictMode)
// must create a *fresh* controller on the second mount because the first was
// disposed. These tests pin that contract by spying on the controller's
// openInitialProject/dispose, without spinning up real WASM. We use
// `new Editor(props)` so the arrow-function fields and constructor-built
// controller exist, and the document stub lets componentDidMount/
// componentWillUnmount run under @jest-environment node without jsdom.

import { Editor } from '../Editor';
import { ProjectController } from '../project-controller';

type EditorInstance = InstanceType<typeof Editor>;

const validProjectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [{ name: 'main', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
});

function makeProps(): EditorInstance['props'] {
  return {
    inputFormat: 'json',
    initialProjectJson: validProjectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
  } as unknown as EditorInstance['props'];
}

// componentDidMount/componentWillUnmount touch document.addEventListener and
// document.removeEventListener. Under @jest-environment node there is no DOM,
// so install a minimal stub.
function withDocumentStub<T>(fn: () => T): T {
  const documentStub = {
    addEventListener: () => {},
    removeEventListener: () => {},
  } as unknown as Document;
  const previous = (globalThis as { document?: Document }).document;
  (globalThis as { document?: Document }).document = documentStub;
  try {
    return fn();
  } finally {
    if (previous === undefined) {
      delete (globalThis as { document?: Document }).document;
    } else {
      (globalThis as { document?: Document }).document = previous;
    }
  }
}

describe('Editor controller lifecycle', () => {
  let openSpy: jest.SpyInstance;
  let disposeSpy: jest.SpyInstance;

  beforeEach(() => {
    // The controller's openInitialProject opens a real engine; stub it so the
    // tests stay off WASM. dispose is the contract we assert on unmount.
    openSpy = jest.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    disposeSpy = jest.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('opens the project on mount and disposes the controller on unmount', async () => {
    const editor = new Editor(makeProps());

    withDocumentStub(() => editor.componentDidMount());
    expect(openSpy).toHaveBeenCalledTimes(1);

    withDocumentStub(() => editor.componentWillUnmount());
    expect(disposeSpy).toHaveBeenCalledTimes(1);
  });

  it('creates a fresh controller across a StrictMode mount/unmount/mount cycle', () => {
    // The constructor builds controller #1. componentWillUnmount disposes it
    // and clears the reference, so the second componentDidMount must build a
    // fresh controller #2 (and open it). Two opens, one per mount; one dispose,
    // for the controller torn down between them.
    const editor = new Editor(makeProps());

    withDocumentStub(() => {
      editor.componentDidMount();
      editor.componentWillUnmount();
      editor.componentDidMount();
    });

    expect(openSpy).toHaveBeenCalledTimes(2);
    expect(disposeSpy).toHaveBeenCalledTimes(1);

    // Clean up the second controller.
    withDocumentStub(() => editor.componentWillUnmount());
    expect(disposeSpy).toHaveBeenCalledTimes(2);
  });

  it('does not throw on unmount when the controller dispose rejects (best-effort)', () => {
    disposeSpy.mockRejectedValue(new Error('engine dispose failed'));
    const editor = new Editor(makeProps());

    withDocumentStub(() => editor.componentDidMount());
    // dispose is fire-and-forget (void); a rejected promise must not throw
    // synchronously out of componentWillUnmount.
    withDocumentStub(() => {
      expect(() => editor.componentWillUnmount()).not.toThrow();
    });
  });
});
