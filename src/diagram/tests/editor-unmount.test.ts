/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for Editor.componentWillUnmount() resource cleanup.
//
// The Editor mounts/unmounts on every wouter route change in src/app
// (`/` <-> `/<user>/<project>` per src/app/App.tsx) and on every
// EditorHost path swap in src/simlin-serve. Two related leaks were
// observed:
//
//  1. componentWillUnmount removed the keydown listener and cleared the
//     selection-deferral timer but never disposed `this.engineProject`.
//     Each route navigation away from a project leaked one EngineProject
//     handle: ~several MB of WASM linear memory plus the engine's salsa
//     caches. The dispose pattern is the engine's contract -- every
//     `EngineProject.open*()` must be matched with a `dispose()`. It is
//     correctly used elsewhere in Editor.tsx (e.g. `openEngineProject()`
//     and the `disposeOrphanedEngine` helper added in commit 3cfe8e4e).
//
//  2. The constructor and `scheduleSimRun()` / `scheduleSave()` schedule
//     fire-and-forget `setTimeout(..., 0)` callbacks without retaining
//     the handle. On unmount any in-flight load completes against a
//     stale `this`, opens a fresh engine, and immediately leaks (and
//     may also call `setState` on an unmounted component). This
//     compounds with leak #1: even a properly disposing unmount cannot
//     catch an engine that is opened *after* unmount.
//
// We use Object.create(Editor.prototype) for the engine-disposal tests
// (they don't need the constructor's deferred work) and `new Editor()`
// with `jest.useFakeTimers()` for the timer-cancellation tests (we want
// to observe the constructor's pending setTimeout). The document stub
// mirrors the one in editor-selection-changed.test.ts so we can call
// componentWillUnmount under @jest-environment node without jsdom.

import { Project as EngineProject } from '@simlin/engine';

import { Editor } from '../Editor';

type EditorInstance = InstanceType<typeof Editor>;

interface FakeEngine {
  serializeProtobuf: () => Promise<Uint8Array>;
  serializeJson: (format?: unknown, includeStdlib?: boolean) => Promise<string>;
  getErrors: () => Promise<unknown[]>;
  dispose: () => Promise<void>;
}

const validProjectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [{ name: 'main', stocks: [], flows: [], auxiliaries: [] }],
});

function makeFakeEngine(overrides: Partial<FakeEngine> = {}): FakeEngine {
  return {
    serializeProtobuf: async () => new Uint8Array([1, 2, 3]),
    serializeJson: async () => validProjectJson,
    getErrors: async () => [],
    dispose: async () => {},
    ...overrides,
  };
}

// componentWillUnmount touches `document.removeEventListener` (the
// keydown listener installed in componentDidMount). Under
// @jest-environment node there is no DOM, so we install the same minimal
// stub editor-selection-changed.test.ts uses for the same reason.
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

function makeEditor(props: Partial<EditorInstance['props']> = {}): EditorInstance {
  const editor = Object.create(Editor.prototype) as EditorInstance;

  editor.engineProject = undefined;
  editor.newEngineShouldPullView = false;
  editor.newEngineQueuedView = undefined;
  editor.inSave = false;
  editor.saveQueued = false;

  editor.state = {
    modelErrors: [],
    activeProject: undefined,
    projectHistory: [],
    modelName: 'main',
    projectVersion: 1,
    cachedErrors: {
      varErrors: new Map(),
      unitErrors: new Map(),
      simError: undefined,
      modelErrors: [],
    },
  } as unknown as EditorInstance['state'];

  editor.props = {
    inputFormat: 'json',
    initialProjectJson: validProjectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
    ...props,
  } as unknown as EditorInstance['props'];

  editor.setState = (updater: unknown) => {
    const next = typeof updater === 'function' ? updater(editor.state) : updater;
    Object.assign(editor.state as object, next);
  };

  return editor;
}

describe('Editor.componentWillUnmount() engine disposal', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('disposes the engine and clears engineProject when unmounted with an open project', async () => {
    const disposeSpy = jest.fn(async () => {});
    const fakeEngine = makeFakeEngine({ dispose: disposeSpy });
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    await editor.openInitialProject();
    expect(editor.engineProject).toBe(fakeEngine as unknown as EngineProject);

    withDocumentStub(() => editor.componentWillUnmount());

    expect(disposeSpy).toHaveBeenCalledTimes(1);
    expect(editor.engineProject).toBeUndefined();
  });

  it('is a no-op when no engine has been opened (no throw)', () => {
    const editor = makeEditor();
    expect(editor.engineProject).toBeUndefined();
    withDocumentStub(() => {
      expect(() => editor.componentWillUnmount()).not.toThrow();
    });
    expect(editor.engineProject).toBeUndefined();
  });

  it('survives a throwing dispose() (best-effort cleanup)', async () => {
    const disposeSpy = jest.fn(async () => {
      throw new Error('engine dispose failed');
    });
    const fakeEngine = makeFakeEngine({ dispose: disposeSpy });
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();
    await editor.openInitialProject();

    // The unmount path swallows dispose errors so a buggy WASM teardown
    // cannot crash the host. The engine handle is still cleared.
    withDocumentStub(() => {
      expect(() => editor.componentWillUnmount()).not.toThrow();
    });
    expect(disposeSpy).toHaveBeenCalledTimes(1);
    expect(editor.engineProject).toBeUndefined();
  });
});

describe('Editor.componentWillUnmount() orphan-timer cancellation', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  it('cancels the scheduleSimRun() timer so a late callback does not run after unmount', () => {
    const editor = makeEditor();

    // Spy on loadSim so we can detect whether the scheduled callback fired.
    // (loadSim is the only side effect of the scheduleSimRun() callback.)
    const loadSimSpy = jest.spyOn(editor, 'loadSim').mockImplementation(async () => {});
    // Seed an engine so the scheduled callback's `if (!engine) return;`
    // guard does not short-circuit before we get to the leak point.
    editor.engineProject = makeFakeEngine() as unknown as EngineProject;

    editor.scheduleSimRun();

    // Now unmount before the timer fires. A correct implementation tracks
    // the timer handle and clears it (or guards the callback with an
    // unmounted flag); a leaky implementation lets the callback fire
    // against a freshly-disposed instance and call loadSim against a
    // dangling engine handle.
    withDocumentStub(() => editor.componentWillUnmount());

    jest.runAllTimers();

    expect(loadSimSpy).not.toHaveBeenCalled();
  });

  it('cancels the scheduleSave() timer so a late callback does not run after unmount', async () => {
    const onSaveSpy = jest.fn(async () => 2);
    const editor = makeEditor({ onSave: onSaveSpy } as Partial<EditorInstance['props']>);
    // Synchronous serializeJson keeps the promise chain on the microtask
    // queue so jest.advanceTimersByTime can drive the callback to its
    // onSave call without real-time waits.
    editor.engineProject = {
      serializeJson: () => '{}',
      serializeProtobuf: () => new Uint8Array([1, 2, 3]),
    } as unknown as EngineProject;

    editor.scheduleSave();

    withDocumentStub(() => editor.componentWillUnmount());

    jest.runAllTimers();
    // Drain any microtasks the (cancelled) callback might have queued.
    await Promise.resolve();
    await Promise.resolve();

    expect(onSaveSpy).not.toHaveBeenCalled();
  });

  it('cancels the constructor timer so EngineProject.openJson is not called after unmount', () => {
    // The constructor schedules `setTimeout(() => { openInitialProject();
    // scheduleSimRun(); })`. EditorHost (src/simlin-serve) and wouter route
    // changes (src/app) frequently mount/unmount the Editor in the same
    // tick a navigation occurs. If the constructor timer is not tracked,
    // the deferred openInitialProject() races with componentWillUnmount,
    // opens an engine on the unmounted instance, and leaks it.
    const fakeEngine = makeFakeEngine();
    const openSpy = jest
      .spyOn(EngineProject, 'openJson')
      .mockResolvedValue(fakeEngine as unknown as EngineProject);

    // `new Editor(props)` triggers the real constructor; jest.useFakeTimers()
    // holds the deferred openInitialProject() until we drain timers.
    const editor = new Editor({
      inputFormat: 'json',
      initialProjectJson: validProjectJson,
      initialProjectVersion: 1,
      name: 'test',
      onSave: async () => 1,
    });

    withDocumentStub(() => editor.componentWillUnmount());

    jest.runAllTimers();

    // openJson must not be called after unmount; otherwise we have leaked
    // a fresh EngineProject handle on an unmounted instance.
    expect(openSpy).not.toHaveBeenCalled();
    expect(editor.engineProject).toBeUndefined();
  });
});
