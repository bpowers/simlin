// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor wires the model-properties drawer's onSimSpecCommit to a single
// model-only edit per settled field edit (issue #55): each typed character
// used to fire a patch -- an undo-history entry and a scheduled save -- so
// typing "1900" recorded four entries and evicted real edits from the 5-deep
// undo buffer. The drawer debounces to one commit per settle; this test asserts
// the Editor turns each commit into exactly one controller model edit (hence
// one undo entry) whose patch, built against the committed project at dequeue,
// sets the right field and echoes the others from the COMMITTED specs.
//
// Mirrors editor-drawer-delete.test.ts: the drawer is mocked to a prop-recording
// stub, Canvas is stubbed out, and the controller is stubbed so a seeded
// snapshot supplies the project without WASM.

import { describe, test, expect, beforeEach, afterEach, rs } from '@rstest/core';
import type { MockInstance } from '@rstest/core';

import * as React from 'react';
import { act, render } from '@testing-library/react';

import type { Project } from '@simlin/core/datamodel';
import type { JsonProjectPatch } from '@simlin/engine';

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

function makeSnapshot(stop = 100): ProjectSnapshot {
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
        stop,
        dt: { isReciprocal: false, value: 1 },
        timeUnits: 'years',
      },
    },
    modelName: 'main',
    projectVersion: 1,
    serverVersion: 1,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelStack: [],
    canUndo: false,
    canRedo: false,
    undoRedoQueued: false,
    token: 0,
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
  let enqueueModelEdit: MockInstance;

  beforeEach(() => {
    capturedDrawerProps = undefined;
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
    enqueueModelEdit = rs.spyOn(ProjectController.prototype, 'enqueueModelEdit').mockResolvedValue(true);
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function render_(): void {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
  }

  // The patch queued edit `index` builds when it dequeues against `committed`.
  function patchOf(index: number, committed: Project = makeSnapshot().project!): JsonProjectPatch {
    const [edit] = enqueueModelEdit.mock.calls[index] as [{ buildPatch: (p: Project) => JsonProjectPatch }];
    return edit.buildPatch(committed);
  }

  test('a start-time commit enqueues exactly one model edit that sets startTime', async () => {
    render_();
    expect(capturedDrawerProps).toBeDefined();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('startTime', 1900);
    });
    expect(enqueueModelEdit).toHaveBeenCalledTimes(1);
    const patch = patchOf(0);
    expect(patch.projectOps).toHaveLength(1);
    expect(patch.projectOps![0].type).toBe('setSimSpecs');
    const simSpecs = (patch.projectOps![0].payload as { simSpecs: Record<string, unknown> }).simSpecs;
    expect(simSpecs.startTime).toBe(1900);
    // Fields the user did not touch keep the committed values.
    expect(simSpecs.endTime).toBe(100);
  });

  test('untouched fields are echoed from the committed specs at dequeue, not from specs read at commit time', async () => {
    render_();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('startTime', 1900);
    });
    // An earlier edit landed stopTime 250 between the commit and the dequeue.
    const simSpecs = (
      patchOf(0, makeSnapshot(250).project!).projectOps![0].payload as {
        simSpecs: Record<string, unknown>;
      }
    ).simSpecs;
    expect(simSpecs.endTime).toBe(250);
  });

  test('a dt commit routes to the dt field as a string', async () => {
    render_();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('dt', 0.5);
    });
    expect(enqueueModelEdit).toHaveBeenCalledTimes(1);
    expect((patchOf(0).projectOps![0].payload as { simSpecs: Record<string, unknown> }).simSpecs.dt).toBe('0.5');
  });

  test('a time-units commit routes the free string through', async () => {
    render_();
    await act(async () => {
      capturedDrawerProps!.onSimSpecCommit('timeUnits', 'months');
    });
    expect(enqueueModelEdit).toHaveBeenCalledTimes(1);
    expect((patchOf(0).projectOps![0].payload as { simSpecs: Record<string, unknown> }).simSpecs.timeUnits).toBe(
      'months',
    );
  });

  test('three separate commits enqueue three model edits (one undo entry each)', async () => {
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
    expect(enqueueModelEdit).toHaveBeenCalledTimes(3);
  });
});
