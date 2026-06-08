/**
 * @jest-environment jsdom
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
//   - construct a controller and kick off openInitialProject() on mount (the
//     controller guards its own dispose-races),
//   - subscribe to controller snapshots,
//   - dispose the controller (which releases the WASM EngineProject handle)
//     and unsubscribe on unmount.
//
// A mount -> unmount -> mount cycle (React 18 StrictMode) must create a *fresh*
// controller on the second mount because the first was disposed. The Editor is
// now a function component, so these assert against OBSERVABLE behavior by
// rendering a real <Editor> through @testing-library/react and spying on the
// controller's openInitialProject/dispose -- never reaching into instance
// internals. openInitialProject/dispose are stubbed so the tests stay off WASM.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render } from '@testing-library/react';

import { Editor, type EditorProps } from '../Editor';
import { ProjectController } from '../project-controller';

const validProjectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [{ name: 'main', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
});

function makeProps(): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: validProjectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
  } as EditorProps;
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

  it('opens the project on mount and disposes the controller on unmount', () => {
    let result!: ReturnType<typeof render>;
    act(() => {
      result = render(React.createElement(Editor, makeProps()));
    });
    expect(openSpy).toHaveBeenCalledTimes(1);

    act(() => {
      result.unmount();
    });
    expect(disposeSpy).toHaveBeenCalledTimes(1);
  });

  it('creates a fresh controller across a StrictMode mount/unmount/mount cycle', () => {
    // React 18 StrictMode drives the component through mount -> unmount -> mount
    // on the same fiber. The mount effect builds controller #1, the unmount
    // cleanup disposes it, and the remount must build a fresh controller #2 (and
    // open it). Two opens (one per mount); one dispose (the controller torn down
    // between them). A final unmount disposes the live controller.
    let result!: ReturnType<typeof render>;
    act(() => {
      result = render(React.createElement(React.StrictMode, null, React.createElement(Editor, makeProps())));
    });

    expect(openSpy).toHaveBeenCalledTimes(2);
    expect(disposeSpy).toHaveBeenCalledTimes(1);

    act(() => {
      result.unmount();
    });
    expect(disposeSpy).toHaveBeenCalledTimes(2);
  });

  it('does not throw on unmount when the controller dispose rejects (best-effort)', () => {
    disposeSpy.mockRejectedValue(new Error('engine dispose failed'));
    let result!: ReturnType<typeof render>;
    act(() => {
      result = render(React.createElement(Editor, makeProps()));
    });

    // dispose is fire-and-forget (the cleanup attaches a .catch); a rejected
    // promise must not throw synchronously out of the unmount cleanup.
    expect(() =>
      act(() => {
        result.unmount();
      }),
    ).not.toThrow();
  });
});
