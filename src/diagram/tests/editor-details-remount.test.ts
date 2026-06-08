/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression test for the details-panel remount key.
//
// The variable/module details panels seed their Slate editors from props in
// their constructors, so React remounts (key changes) are the mechanism that
// refreshes them after the underlying variable changes. The Editor builds the
// panel key from `controllerSnapshot.projectGeneration` (not projectVersion):
// projectGeneration increments exactly when project *content* changes (real
// edits, undo/redo) and stays put for view-only pan/zoom frames and
// save-version bookkeeping, so an open panel is NOT remounted -- discarding
// in-progress edits -- on a pan or an autosave.
//
// The *semantics* of projectGeneration (when it does/doesn't increment) live in
// and are tested against the ProjectController (project-controller.test.ts).
// What remains Editor-specific is that the rendered panel REMOUNTS on a
// projectGeneration change and NOT on a projectVersion-only change. The Editor
// is now a function component, so rather than reading the literal `key` string
// off an internal render-helper, this asserts that observable remount behavior:
// a stub VariableDetails records each mount; a generation bump remounts it, a
// version-only bump does not.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render } from '@testing-library/react';

import { projectFromJson, type JsonProject } from '@simlin/core/datamodel';
import { ProjectController, type ProjectSnapshot } from '../project-controller';

// A project with a single aux 'x' and a view element selecting it, so the
// Editor's details flow resolves to a VariableDetails panel.
const projectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: [{ name: 'x', equation: '1' }],
      views: [{ elements: [{ type: 'aux', uid: 1, name: 'x', x: 0, y: 0 }] }],
    },
  ],
});

// Record a mount each time the stub VariableDetails is (re)mounted. A key change
// (generation bump) unmounts the old instance and mounts a new one, so the
// mount count distinguishes a remount from an in-place prop update.
let variableDetailsMounts = 0;
jest.mock('../VariableDetails', () => ({
  __esModule: true,
  VariableDetails: () => {
    React.useEffect(() => {
      variableDetailsMounts += 1;
    }, []);
    return null;
  },
}));

// Capture the props the Editor hands the Canvas so we can drive the real
// selection/show-details handlers (the documented Canvas -> Editor contract)
// without WASM, jsdom SVG geometry, or a ResizeObserver.
interface CapturedCanvasProps {
  onSetSelection: (sel: ReadonlySet<number>) => void;
  onShowVariableDetails: () => void;
}
let capturedCanvasProps: CapturedCanvasProps | undefined;
jest.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CapturedCanvasProps) => {
    capturedCanvasProps = p;
    return null;
  },
  inCreationUid: -2,
}));

import { Editor, type EditorProps } from '../Editor';

function makeSnapshot(projectGeneration: number, projectVersion: number): ProjectSnapshot {
  const project = projectFromJson(JSON.parse(projectJson) as JsonProject);
  return {
    project,
    projectVersion,
    projectGeneration,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelName: 'main',
    modelStack: [],
    canUndo: false,
    canRedo: false,
    navResetSeq: 0,
  } as unknown as ProjectSnapshot;
}

function makeProps(): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: projectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
  } as EditorProps;
}

describe('Editor details-panel remount on projectGeneration change', () => {
  let snapshot: ProjectSnapshot;
  let listener: (() => void) | undefined;

  beforeEach(() => {
    variableDetailsMounts = 0;
    capturedCanvasProps = undefined;
    listener = undefined;
    snapshot = makeSnapshot(0, 1);
    jest.spyOn(ProjectController.prototype, 'getSnapshot').mockImplementation(() => snapshot);
    jest.spyOn(ProjectController.prototype, 'subscribe').mockImplementation((l: () => void) => {
      listener = l;
      return () => {
        listener = undefined;
      };
    });
    jest.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  // Publish a new snapshot through the controller subscription, the way the real
  // controller notifies the Editor after an edit/undo/pan.
  function publish(next: ProjectSnapshot): void {
    snapshot = next;
    act(() => {
      listener?.();
    });
  }

  function openVariableDetails(): void {
    // Drive the real Editor handlers through the Canvas -> Editor contract.
    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([1]));
      capturedCanvasProps!.onShowVariableDetails();
    });
  }

  it('remounts the VariableDetails panel when projectGeneration changes', () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    openVariableDetails();
    expect(variableDetailsMounts).toBe(1);

    // A content edit bumps generation -> key changes -> remount.
    publish(makeSnapshot(1, 2));
    expect(variableDetailsMounts).toBe(2);
  });

  it('does NOT remount the panel on a projectVersion-only change (pan/autosave)', () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    openVariableDetails();
    expect(variableDetailsMounts).toBe(1);

    // A view-only update bumps projectVersion but NOT projectGeneration -> the
    // key is unchanged -> the open panel must not remount.
    publish(makeSnapshot(0, 1.001));
    expect(variableDetailsMounts).toBe(1);
  });
});
