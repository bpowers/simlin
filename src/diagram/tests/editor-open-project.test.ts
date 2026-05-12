/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for Editor.openInitialProject() / openEngineProject().
//
// Both methods used to wrap only the EngineProject.open*() call in a
// try/catch. The steps that run *after* a successful open --
// serializeProtobuf(), JSON.parse(serializeJson(...)), projectFromJson()
// (which throws on an unknown view element type), updateVariableErrors(),
// and the final setState() -- were unguarded. openInitialProject() is
// invoked from a fire-and-forget setTimeout in componentDidMount and there
// is no React error boundary in src/app or src/diagram, so a panic in the
// engine's serializeJson or a rejection from projectFromJson became an
// unhandled rejection: state.activeProject stayed undefined and the user
// saw editor chrome with a blank canvas and no error message.
//
// These tests drive both methods into "open succeeds, a later step fails"
// and assert that the method resolves (does not reject) and that the
// failure is surfaced via state.modelErrors. They also keep a regression
// guard for the happy path. We use Object.create(Editor.prototype) to
// bypass the constructor (which would spin up async WASM initialisation
// and React internals); only the fields these methods touch are seeded,
// and EngineProject.open* is replaced with a jest spy returning a fake
// engine handle.

import { Project as EngineProject } from '@simlin/engine';

import { Editor } from '../Editor';

type EditorInstance = InstanceType<typeof Editor>;

interface FakeEngine {
  serializeProtobuf: () => Promise<Uint8Array>;
  serializeJson: (format?: unknown, includeStdlib?: boolean) => Promise<string>;
  getErrors: () => Promise<unknown[]>;
  dispose: () => Promise<void>;
}

// A minimal but valid JSON project that projectFromJson() accepts.
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

function makeEditor(props: Partial<EditorInstance['props']> = {}): EditorInstance {
  const editor = Object.create(Editor.prototype) as EditorInstance;

  editor.engineProject = undefined;
  editor.newEngineShouldPullView = false;
  editor.newEngineQueuedView = undefined;

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

describe('Editor.openInitialProject() error handling', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('surfaces a model error and resolves when serializeJson rejects after a successful open', async () => {
    const disposeSpy = jest.fn(async () => {});
    const fakeEngine = makeFakeEngine({
      serializeJson: async () => {
        throw new Error('engine panic in serializeJson');
      },
      dispose: disposeSpy,
    });
    const openSpy = jest.spyOn(EngineProject, 'openJson').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    await expect(editor.openInitialProject()).resolves.toBeUndefined();

    expect(openSpy).toHaveBeenCalledTimes(1);
    expect(editor.state.modelErrors.length).toBeGreaterThan(0);
    expect(editor.state.modelErrors.some((e) => e.message.includes('engine panic in serializeJson'))).toBe(true);
    expect(editor.state.modelErrors.some((e) => e.message.includes('opening the project'))).toBe(true);
    expect(editor.state.activeProject).toBeUndefined();
    // The WASM handle that was opened must be released so we don't leak it.
    expect(disposeSpy).toHaveBeenCalledTimes(1);
    expect(editor.engineProject).toBeUndefined();
  });

  it('surfaces a model error and resolves when projectFromJson rejects the engine JSON', async () => {
    // serializeJson succeeds but the JSON contains an unknown view element
    // type, which makes projectFromJson throw.
    const badJson = JSON.stringify({
      name: 'test',
      simSpecs: { startTime: 0, endTime: 10, dt: '1' },
      models: [
        {
          name: 'main',
          stocks: [],
          flows: [],
          auxiliaries: [],
          views: [{ kind: 'stock_flow', elements: [{ type: 'not_a_real_element_type', uid: 1 }] }],
        },
      ],
    });
    const fakeEngine = makeFakeEngine({ serializeJson: async () => badJson });
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    await expect(editor.openInitialProject()).resolves.toBeUndefined();

    expect(editor.state.modelErrors.length).toBeGreaterThan(0);
    expect(editor.state.modelErrors.some((e) => e.message.includes('unknown view element type'))).toBe(true);
    expect(editor.state.activeProject).toBeUndefined();
  });

  it('does not surface an error and populates activeProject on a successful open', async () => {
    const fakeEngine = makeFakeEngine();
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    await editor.openInitialProject();

    expect(editor.state.modelErrors).toHaveLength(0);
    expect(editor.state.activeProject).toBeDefined();
    expect(editor.state.activeProject?.name).toBe('test');
    expect(editor.state.projectHistory).toHaveLength(1);
    expect(editor.engineProject).toBe(fakeEngine as unknown as EngineProject);
  });

  it('surfaces a model error and resolves when EngineProject.openJson itself fails', async () => {
    jest.spyOn(EngineProject, 'openJson').mockRejectedValue(new Error('bad bytes'));

    const editor = makeEditor();

    await expect(editor.openInitialProject()).resolves.toBeUndefined();

    expect(editor.state.modelErrors.some((e) => e.message.includes('bad bytes'))).toBe(true);
    expect(editor.state.activeProject).toBeUndefined();
  });
});

describe('Editor.openEngineProject() error handling', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('surfaces a model error and resolves when serializeJson rejects after a successful open', async () => {
    const disposeSpy = jest.fn(async () => {});
    const fakeEngine = makeFakeEngine({
      serializeJson: async () => {
        throw new Error('engine panic in serializeJson');
      },
      dispose: disposeSpy,
    });
    jest.spyOn(EngineProject, 'openProtobuf').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    const result = await editor.openEngineProject(new Uint8Array([1, 2, 3]));

    expect(result).toBeUndefined();
    expect(editor.state.modelErrors.length).toBeGreaterThan(0);
    expect(editor.state.modelErrors.some((e) => e.message.includes('engine panic in serializeJson'))).toBe(true);
    expect(editor.state.modelErrors.some((e) => e.message.includes('opening the project'))).toBe(true);
    expect(editor.state.activeProject).toBeUndefined();
    expect(disposeSpy).toHaveBeenCalledTimes(1);
    expect(editor.engineProject).toBeUndefined();
  });

  it('surfaces a model error and resolves when projectFromJson rejects the engine JSON', async () => {
    const badJson = JSON.stringify({
      name: 'test',
      simSpecs: { startTime: 0, endTime: 10, dt: '1' },
      models: [
        {
          name: 'main',
          stocks: [],
          flows: [],
          auxiliaries: [],
          views: [{ kind: 'stock_flow', elements: [{ type: 'not_a_real_element_type', uid: 1 }] }],
        },
      ],
    });
    const fakeEngine = makeFakeEngine({ serializeJson: async () => badJson });
    jest.spyOn(EngineProject, 'openProtobuf').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    const result = await editor.openEngineProject(new Uint8Array([1, 2, 3]));

    expect(result).toBeUndefined();
    expect(editor.state.modelErrors.some((e) => e.message.includes('unknown view element type'))).toBe(true);
    expect(editor.state.activeProject).toBeUndefined();
  });

  it('returns the engine and populates activeProject on a successful open', async () => {
    const fakeEngine = makeFakeEngine();
    jest.spyOn(EngineProject, 'openProtobuf').mockResolvedValue(fakeEngine as unknown as EngineProject);

    const editor = makeEditor();

    const result = await editor.openEngineProject(new Uint8Array([1, 2, 3]));

    expect(result).toBe(fakeEngine as unknown as EngineProject);
    expect(editor.state.modelErrors).toHaveLength(0);
    expect(editor.state.activeProject).toBeDefined();
    expect(editor.state.activeProject?.name).toBe('test');
    expect(editor.engineProject).toBe(fakeEngine as unknown as EngineProject);
  });
});
