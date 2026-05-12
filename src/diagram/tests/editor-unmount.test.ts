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
//  2. componentDidMount and `scheduleSimRun()` / `scheduleSave()` schedule
//     fire-and-forget `setTimeout(..., 0)` callbacks without retaining
//     the handle. On unmount any in-flight load completes against a
//     stale `this`, opens a fresh engine, and immediately leaks (and
//     may also call `setState` on an unmounted component). This
//     compounds with leak #1: even a properly disposing unmount cannot
//     catch an engine that is opened *after* unmount.
//
// (The deferred openInitialProject() lives in componentDidMount, not the
// constructor, so a React 18 StrictMode unmount/remount re-schedules it --
// see Editor.componentDidMount. The "deferred project load" describe block
// at the bottom covers that.)
//
// We use Object.create(Editor.prototype) for the engine-disposal tests
// (they don't exercise the deferred project load) and `new Editor()`
// with `jest.useFakeTimers()` for the timer tests (we want to observe the
// pending setTimeout componentDidMount schedules). The document stub
// mirrors the one in editor-selection-changed.test.ts so we can call
// componentWillUnmount / componentDidMount under @jest-environment node
// without jsdom.

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

  it('cancels the deferred openInitialProject() timer so EngineProject.openJson is not called after unmount', () => {
    // componentDidMount schedules `setTimeout(() => { openInitialProject();
    // scheduleSimRun(); })`. EditorHost (src/simlin-serve) and wouter route
    // changes (src/app) frequently mount/unmount the Editor in the same
    // tick a navigation occurs. If the timer is not tracked, the deferred
    // openInitialProject() races with componentWillUnmount, opens an engine
    // on the unmounted instance, and leaks it.
    const fakeEngine = makeFakeEngine();
    const openSpy = jest
      .spyOn(EngineProject, 'openJson')
      .mockResolvedValue(fakeEngine as unknown as EngineProject);

    // jest.useFakeTimers() holds the deferred openInitialProject() until we
    // drain timers; componentWillUnmount runs first and must cancel it.
    const editor = new Editor({
      inputFormat: 'json',
      initialProjectJson: validProjectJson,
      initialProjectVersion: 1,
      name: 'test',
      onSave: async () => 1,
    });

    withDocumentStub(() => {
      editor.componentDidMount();
      editor.componentWillUnmount();
    });

    jest.runAllTimers();

    // openJson must not be called after unmount; otherwise we have leaked
    // a fresh EngineProject handle on an unmounted instance.
    expect(openSpy).not.toHaveBeenCalled();
    expect(editor.engineProject).toBeUndefined();
  });

  it('cancels the handleUndoRedo timer so EngineProject.openProtobuf is not called after unmount', () => {
    // handleUndoRedo defers `setTimeout(() => { openEngineProject(...);
    // ...; scheduleSimRun(); scheduleSave(); })`. openEngineProject opens a
    // fresh engine and assigns it to this.engineProject; if the Editor has
    // since unmounted (wouter route change / EditorHost path swap), that
    // engine is stranded on a dead instance. The timer must be tracked and
    // cancelled (and the callback short-circuits on `unmounted`).
    const fakeEngine = makeFakeEngine();
    const openProtobufSpy = jest
      .spyOn(EngineProject, 'openProtobuf')
      .mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = new Editor({
      inputFormat: 'json',
      initialProjectJson: validProjectJson,
      initialProjectVersion: 1,
      name: 'test',
      onSave: async () => 1,
    });
    // handleUndoRedo reads projectHistory[projectOffset] via defined() and
    // calls this.setState; new Editor() never mounted, so seed the state
    // fields it touches and make setState a no-op (the state update is
    // irrelevant here -- we only care that the deferred work is cancelled).
    (editor.state as { projectHistory: ReadonlyArray<Uint8Array>; projectOffset: number }).projectHistory = [
      new Uint8Array([1, 2, 3]),
    ];
    (editor.state as { projectOffset: number }).projectOffset = 0;
    editor.setState = (() => {}) as typeof editor.setState;

    editor.handleUndoRedo('undo');

    withDocumentStub(() => editor.componentWillUnmount());

    jest.runAllTimers();

    expect(openProtobufSpy).not.toHaveBeenCalled();
    expect(editor.engineProject).toBeUndefined();
  });
});

describe('Editor.componentDidMount() deferred project load', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  function makeMountedProps(): EditorInstance['props'] {
    return {
      inputFormat: 'json',
      initialProjectJson: validProjectJson,
      initialProjectVersion: 1,
      name: 'test',
      onSave: async () => 1,
    } as unknown as EditorInstance['props'];
  }

  it('does not schedule the deferred load from the constructor alone', () => {
    // The constructor must be side-effect free. React 18 StrictMode (dev)
    // double-invokes the render phase, creating a second Editor instance
    // that is then discarded -- its componentDidMount and componentWillUnmount
    // never run. A timer scheduled in the constructor would still fire on
    // that zombie `this`, opening an EngineProject and then crashing in
    // loadSim() on the state.activeProject that the discarded instance's
    // setState() never committed.
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(makeFakeEngine() as unknown as EngineProject);

    const editor = new Editor(makeMountedProps());
    const openInitialProjectSpy = jest.spyOn(editor, 'openInitialProject').mockResolvedValue(undefined);

    jest.runAllTimers();

    expect(openInitialProjectSpy).not.toHaveBeenCalled();
  });

  it('reschedules openInitialProject() across a StrictMode mount/unmount/mount cycle', () => {
    // React 18 StrictMode drives every committed component through
    // componentDidMount -> componentWillUnmount -> componentDidMount on the
    // *same* instance, without re-running the constructor. If the deferred
    // openInitialProject() were scheduled in the constructor (and cancelled
    // by componentWillUnmount), the second mount would never reschedule it:
    // engineProject and state.activeProject stay undefined and the editor
    // sits on a blank canvas. Scheduling in componentDidMount makes the
    // cycle schedule -> cancel -> schedule, so the load still happens once.
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(makeFakeEngine() as unknown as EngineProject);

    const editor = new Editor(makeMountedProps());
    const openInitialProjectSpy = jest.spyOn(editor, 'openInitialProject').mockResolvedValue(undefined);
    // The deferred callback calls scheduleSimRun() after openInitialProject()
    // resolves; stub it so the test doesn't leave a real timer pending once
    // jest.useRealTimers() restores in afterEach.
    jest.spyOn(editor, 'scheduleSimRun').mockImplementation(() => {});

    withDocumentStub(() => {
      editor.componentDidMount();
      editor.componentWillUnmount();
      editor.componentDidMount();
    });
    // Still deferred -- nothing has run synchronously.
    expect(openInitialProjectSpy).not.toHaveBeenCalled();

    jest.runAllTimers();

    expect(openInitialProjectSpy).toHaveBeenCalledTimes(1);
  });
});
