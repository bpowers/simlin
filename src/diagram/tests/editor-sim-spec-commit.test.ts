// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor wires the model-properties drawer's onSimSpecCommit to a single
// engine patch per settled field edit (issue #55): before the fix each typed
// character fired applyPatch -- an undo-history entry and a scheduled save --
// so typing "1900" recorded four entries and evicted real edits from the
// 5-deep undo buffer. The drawer now debounces to one commit per settle; this
// test asserts the Editor side turns each commit into exactly one applyPatch
// (hence one undo entry) targeting the right sim-spec field.
//
// Mirrors editor-drawer-delete.test.ts: the drawer is mocked to a prop-recording
// stub, Canvas is stubbed out, and the controller is stubbed so a seeded
// snapshot supplies the project without WASM.

import { describe, test, expect, beforeEach, afterEach, rs } from '@rstest/core';
import type { MockInstance } from '@rstest/core';

import * as React from 'react';
import { act, render } from '@testing-library/react';

import type { ModelPropertiesDrawer as ModelPropertiesDrawerType } from '../ModelPropertiesDrawer';
import { ProjectController, type ProjectSnapshot } from '../project-controller';

type DrawerProps = React.ComponentProps<typeof ModelPropertiesDrawerType>;
let capturedDrawerProps: DrawerProps | undefined;

rs.mock('../ModelPropertiesDrawer', () => ({
  __esModule: true,
  ModelPropertiesDrawer: (p: DrawerProps) => {
    capturedDrawerProps = p;
    return null;
  },
}));

rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: () => null,
  inCreationUid: -2,
}));

import { Editor, type EditorProps } from '../Editor';

function makeSnapshot(): ProjectSnapshot {
  const view = {
    nextUid: 1,
    elements: [],
    viewBox: { x: 0, y: 0, width: 800, height: 600 },
    zoom: 1,
    useLetteredPolarity: false,
  };
  return {
    project: {
      name: 'test-project',
      models: new Map([['main', { name: 'main', variables: new Map(), views: [view], loopMetadata: [], groups: [] }]]),
      simSpecs: {
        start: 0,
        stop: 100,
        dt: { isReciprocal: false, value: 1 },
        timeUnits: 'years',
      },
    },
    modelName: 'main',
    projectVersion: 1,
    projectGeneration: 0,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelStack: [],
    canUndo: false,
    canRedo: false,
    navResetSeq: 0,
  } as unknown as ProjectSnapshot;
}

function makeProps(overrides: Partial<EditorProps> = {}): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: '{}',
    initialProjectVersion: 1,
    name: 'test-project',
    embedded: false,
    readOnlyMode: false,
    onSave: async () => 1,
    ...overrides,
  } as EditorProps;
}

describe('Editor sim-spec commit wiring (issue #55)', () => {
  let applyPatch: MockInstance;

  beforeEach(() => {
    capturedDrawerProps = undefined;
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
    applyPatch = rs.spyOn(ProjectController.prototype, 'applyPatch').mockResolvedValue(true);
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function render_(): void {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
  }

  test('a start-time commit fires exactly one applyPatch with setSimSpecs', async () => {
    render_();
    expect(capturedDrawerProps).toBeDefined();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('startTime', 1900);
    });
    expect(applyPatch).toHaveBeenCalledTimes(1);
    const [patch] = applyPatch.mock.calls[0];
    expect(patch.projectOps).toHaveLength(1);
    expect(patch.projectOps[0].type).toBe('setSimSpecs');
    expect(patch.projectOps[0].payload.simSpecs.startTime).toBe(1900);
    // Fields the user did not touch keep the model values.
    expect(patch.projectOps[0].payload.simSpecs.endTime).toBe(100);
  });

  test('a dt commit routes to the dt field as a string', async () => {
    render_();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('dt', 0.5);
    });
    expect(applyPatch).toHaveBeenCalledTimes(1);
    const [patch] = applyPatch.mock.calls[0];
    expect(patch.projectOps[0].payload.simSpecs.dt).toBe('0.5');
  });

  test('a time-units commit routes the free string through', async () => {
    render_();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('timeUnits', 'months');
    });
    expect(applyPatch).toHaveBeenCalledTimes(1);
    const [patch] = applyPatch.mock.calls[0];
    expect(patch.projectOps[0].payload.simSpecs.timeUnits).toBe('months');
  });

  test('three separate commits fire three applyPatches (one undo entry each)', async () => {
    render_();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('startTime', 10);
    });
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('stopTime', 200);
    });
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('dt', 2);
    });
    expect(applyPatch).toHaveBeenCalledTimes(3);
  });
});
