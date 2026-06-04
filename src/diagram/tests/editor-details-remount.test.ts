/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for when the variable/module details panels remount.
//
// The panels seed their Slate editors from props in their constructors, so
// React remounts (key changes) are the mechanism that refreshes them after
// the underlying variable changes. The key used to include projectVersion,
// which bumps +0.001 on every pan/zoom frame and is reset to the server
// version when an autosave completes -- each of those remounted an open
// panel, discarding in-progress unsaved edits and refiring the async LaTeX
// load. The key now uses `projectGeneration`, which must increment exactly
// when project *content* changes (real edits, undo/redo) and stay put for
// view-only updates and save-version bookkeeping.

import { projectFromJson } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

import { Editor } from '../Editor';

type EditorInstance = InstanceType<typeof Editor>;

const validProjectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [{ name: 'main', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
});

const snap = (n: number): Uint8Array => new Uint8Array([n]);

function makeFakeEngine() {
  return {
    serializeProtobuf: jest.fn(async () => snap(9)),
    serializeJson: jest.fn(async () => validProjectJson),
    getErrors: jest.fn(async () => []),
    applyPatch: jest.fn(async () => {}),
    dispose: jest.fn(async () => {}),
  };
}

function makeEditor(engine: ReturnType<typeof makeFakeEngine>): EditorInstance {
  const editor = Object.create(Editor.prototype) as EditorInstance;

  editor.engineProject = engine as unknown as EditorInstance['engineProject'];
  editor.newEngineShouldPullView = false;
  editor.newEngineQueuedView = undefined;
  editor.inSave = false;
  editor.saveQueued = false;

  editor.state = {
    modelErrors: [],
    activeProject: projectFromJson(JSON.parse(validProjectJson) as JsonProject),
    projectHistory: [snap(1)],
    projectOffset: 0,
    projectGeneration: 0,
    modelName: 'main',
    modelStack: [],
    data: new Map(),
    projectVersion: 1,
    selection: new Set<number>(),
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
  } as unknown as EditorInstance['props'];

  editor.setState = (updater: unknown) => {
    const next = typeof updater === 'function' ? updater(editor.state) : updater;
    Object.assign(editor.state as object, next);
  };

  editor.scheduleSave = jest.fn();
  editor.scheduleSimRun = jest.fn();

  return editor;
}

type GenerationState = { projectGeneration: number };

describe('Editor.projectGeneration (details panel remount key)', () => {
  it('increments on a real edit (history-recording updateProject)', async () => {
    const editor = makeEditor(makeFakeEngine());
    const before = (editor.state as unknown as GenerationState).projectGeneration;

    await editor.updateProject(snap(2));

    expect((editor.state as unknown as GenerationState).projectGeneration).toBe(before + 1);
  });

  it('does NOT increment on a view-only update (pan/zoom)', async () => {
    const editor = makeEditor(makeFakeEngine());
    const before = (editor.state as unknown as GenerationState).projectGeneration;

    const view = editor.getView();
    expect(view).toBeDefined();
    await editor.queueViewUpdate({ ...view!, zoom: 2 });

    expect((editor.state as unknown as GenerationState).projectGeneration).toBe(before);
  });

  it('does NOT change when an autosave completes and resets projectVersion', async () => {
    const engine = makeFakeEngine();
    const editor = makeEditor(engine);
    const before = (editor.state as unknown as GenerationState).projectGeneration;

    await editor.save(1);

    expect((editor.state as unknown as GenerationState).projectGeneration).toBe(before);
  });

  it('increments on undo/redo so the panels pick up restored content', () => {
    // handleUndoRedo is an arrow-function instance field, so this case uses
    // `new Editor(props)` (constructor is side-effect free; the deferred
    // engine reopen is held back by fake timers and a stub).
    jest.useFakeTimers();
    try {
      const editor = new Editor({
        inputFormat: 'json',
        initialProjectJson: validProjectJson,
        initialProjectVersion: 1,
        name: 'test',
        onSave: async () => 1,
      });
      Object.defineProperty(editor, 'state', {
        value: { ...editor.state, projectHistory: [snap(2), snap(1)], projectOffset: 0 },
        writable: true,
        configurable: true,
      });
      editor.setState = ((updater: unknown) => {
        const next = typeof updater === 'function' ? (updater as (s: unknown) => unknown)(editor.state) : updater;
        Object.assign(editor.state as object, next);
      }) as EditorInstance['setState'];
      // The deferred engine reopen isn't under test here.
      editor.openEngineProject = jest.fn(async () => undefined);
      const before = (editor.state as unknown as GenerationState).projectGeneration;

      editor.handleUndoRedo('undo');

      expect((editor.state as unknown as GenerationState).projectGeneration).toBe(before + 1);
    } finally {
      jest.useRealTimers();
    }
  });
});
