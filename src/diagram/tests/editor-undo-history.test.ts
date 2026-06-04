/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the Editor's undo history bookkeeping.
//
// Two invariants:
//
// 1. Editing after an undo discards the redo branch. projectHistory is
//    newest-first and projectOffset points at the displayed snapshot;
//    entries before the offset were created after the snapshot being
//    edited, so a new edit must drop them or a later undo would jump to
//    abandoned sibling states instead of the edit's true parent.
//
// 2. View-only updates (pan/zoom/momentum frames, panel resizes -- anything
//    flowing through queueViewUpdate) must NOT record undo snapshots.
//    viewBox/zoom are serialized into the protobuf, so each frame is a
//    distinct snapshot; with MaxUndoSize = 5 a single momentum flick would
//    otherwise evict every real edit from the undo buffer.
//
// Same Object.create(Editor.prototype) harness as editor-open-project.test.ts.

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

interface FakeEngine {
  serializeProtobuf: jest.Mock;
  serializeJson: jest.Mock;
  getErrors: jest.Mock;
  applyPatch: jest.Mock;
  dispose: jest.Mock;
}

function makeFakeEngine(nextProtobuf: Uint8Array): FakeEngine {
  return {
    serializeProtobuf: jest.fn(async () => nextProtobuf),
    serializeJson: jest.fn(async () => validProjectJson),
    getErrors: jest.fn(async () => []),
    applyPatch: jest.fn(async () => {}),
    dispose: jest.fn(async () => {}),
  };
}

function makeEditor(history: Uint8Array[], offset: number, engine: FakeEngine): EditorInstance {
  const editor = Object.create(Editor.prototype) as EditorInstance;

  editor.engineProject = engine as unknown as EditorInstance['engineProject'];
  editor.newEngineShouldPullView = false;
  editor.newEngineQueuedView = undefined;
  editor.inSave = false;
  editor.saveQueued = false;

  editor.state = {
    modelErrors: [],
    activeProject: undefined,
    projectHistory: history,
    projectOffset: offset,
    modelName: 'main',
    modelStack: [],
    data: new Map(),
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
  } as unknown as EditorInstance['props'];

  editor.setState = (updater: unknown) => {
    const next = typeof updater === 'function' ? updater(editor.state) : updater;
    Object.assign(editor.state as object, next);
  };

  // The save path is exercised by editor-save-insave.test.ts; here we only
  // care about history bookkeeping, so neuter the deferred save dispatch.
  editor.scheduleSave = jest.fn();

  return editor;
}

describe('Editor undo history bookkeeping', () => {
  it('editing after an undo discards the redo branch', async () => {
    // Viewing C (offset 2) of [E, D, C, B, A]; an edit producing F must
    // yield [F, C, B, A] so undo from F lands on C, not E.
    const engine = makeFakeEngine(snap(6));
    const editor = makeEditor([snap(5), snap(4), snap(3), snap(2), snap(1)], 2, engine);

    await editor.updateProject(snap(6));

    expect(editor.state.projectHistory).toEqual([snap(6), snap(3), snap(2), snap(1)]);
    expect(editor.state.projectOffset).toBe(0);
    expect(editor.scheduleSave).toHaveBeenCalled();
  });

  it('an ordinary edit prepends and caps at MaxUndoSize', async () => {
    const engine = makeFakeEngine(snap(6));
    const editor = makeEditor([snap(5), snap(4), snap(3), snap(2), snap(1)], 0, engine);

    await editor.updateProject(snap(6));

    expect(editor.state.projectHistory).toEqual([snap(6), snap(5), snap(4), snap(3), snap(2)]);
    expect(editor.state.projectOffset).toBe(0);
  });

  it('view-only updates do not record undo snapshots or move the offset', async () => {
    const engine = makeFakeEngine(snap(9));
    const editor = makeEditor([snap(2), snap(1)], 1, engine);

    // Seed an active project so setView/getView work, then drive the
    // view-only path (pan/zoom) the way Canvas's onViewBoxChange does.
    const activeProject = projectFromJson(JSON.parse(validProjectJson) as JsonProject);
    editor.setState({ activeProject });
    const versionBefore = editor.state.projectVersion;

    const view = editor.getView();
    expect(view).toBeDefined();
    await editor.queueViewUpdate({ ...view!, zoom: 2 });

    // The view change reaches the engine and bumps the render version, but
    // the undo history is untouched.
    expect(engine.applyPatch).toHaveBeenCalled();
    expect(editor.state.projectVersion).toBeGreaterThan(versionBefore);
    expect(editor.state.projectHistory).toEqual([snap(2), snap(1)]);
    expect(editor.state.projectOffset).toBe(1);
    expect(editor.scheduleSave).not.toHaveBeenCalled();
  });
});
