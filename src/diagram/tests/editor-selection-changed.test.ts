/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Verifies the onSelectionChanged prop fires after each *committed* selection
// change with the canonical-ident array, but NOT on initial mount.
//
// The callback used to be deferred via setTimeout(0) inside handleSelection so
// React could commit the new selection before getSelectionIdents() read it. It
// now fires from a post-commit effect keyed on the committed selection (React
// only runs effects after the commit), so the read is already against fresh
// state and no deferral (or unmount-time timer cancellation) is needed. This
// also makes the callback fire for selection changes that never went through
// handleSelection (deletion clearing the selection, module drill-in/back, undo
// resets) and NOT fire when the committed selection is unchanged (content-equal
// Sets from undo/navigate-back), nor on the initial mount.
//
// The Editor is now a function component, so this drives the real selection
// flow through the documented Canvas -> Editor contract (the Canvas is mocked
// to capture the onSetSelection handler) and asserts on the host's
// onSelectionChanged callback -- never reaching into instance internals.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render } from '@testing-library/react';

import { projectFromJson, type JsonProject } from '@simlin/core/datamodel';
import { ProjectController, type ProjectSnapshot } from '../project-controller';

// A project with two auxes ('a' uid 1, 'b' uid 2) so getSelectionIdents maps
// the selected uids back to canonical idents.
const projectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: [
        { name: 'a', equation: '1' },
        { name: 'b', equation: '1' },
      ],
      views: [
        {
          elements: [
            { type: 'aux', uid: 1, name: 'a', x: 0, y: 0 },
            { type: 'aux', uid: 2, name: 'b', x: 0, y: 0 },
          ],
        },
      ],
    },
  ],
});

// Capture the Canvas props so the test can drive the real onSetSelection
// handler (the documented Canvas -> Editor contract) without WASM/jsdom SVG.
interface CapturedCanvasProps {
  onSetSelection: (sel: ReadonlySet<number>) => void;
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

function makeSnapshot(navResetSeq = 0): ProjectSnapshot {
  const project = projectFromJson(JSON.parse(projectJson) as JsonProject);
  return {
    project,
    projectVersion: 1,
    projectGeneration: 0,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelName: 'main',
    modelStack: [],
    canUndo: false,
    canRedo: false,
    navResetSeq,
  } as unknown as ProjectSnapshot;
}

function makeProps(onSelectionChanged?: (idents: string[]) => void): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: projectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
    onSelectionChanged,
  } as EditorProps;
}

describe('Editor onSelectionChanged (post-commit effect)', () => {
  let snapshot: ProjectSnapshot;
  let listener: (() => void) | undefined;

  beforeEach(() => {
    capturedCanvasProps = undefined;
    listener = undefined;
    snapshot = makeSnapshot();
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

  function renderEditor(cb?: (idents: string[]) => void): void {
    act(() => {
      render(React.createElement(Editor, makeProps(cb)));
    });
  }

  function setSelection(sel: ReadonlySet<number>): void {
    act(() => {
      capturedCanvasProps!.onSetSelection(sel);
    });
  }

  // Publish a new snapshot through the controller subscription, as the real
  // controller does after an edit/undo. A bumped navResetSeq is the undo-driven
  // navigation reset.
  function publish(next: ProjectSnapshot): void {
    snapshot = next;
    act(() => {
      listener?.();
    });
  }

  it('does not fire on initial mount', () => {
    const callback = jest.fn();
    renderEditor(callback);
    expect(callback).not.toHaveBeenCalled();
  });

  it('invokes onSelectionChanged with the canonical idents after the selection commits', () => {
    const callback = jest.fn();
    renderEditor(callback);

    setSelection(new Set([1, 2]));

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith(['a', 'b']);
  });

  it('passes an empty array when the selection becomes empty', () => {
    const callback = jest.fn();
    renderEditor(callback);

    setSelection(new Set([1]));
    callback.mockClear();

    setSelection(new Set());

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith([]);
  });

  it('does not fire when the committed selection is unchanged (content-equal Set)', () => {
    // A re-render that commits a content-identical but distinct Set (the
    // undo/navigate-back restoredSelection scenario) must NOT re-notify the
    // host: the guard uses setsEqual, not reference equality.
    const callback = jest.fn();
    renderEditor(callback);

    setSelection(new Set([1]));
    callback.mockClear();

    // A fresh Set with identical contents.
    setSelection(new Set([1]));

    expect(callback).not.toHaveBeenCalled();
  });

  it('does not throw when onSelectionChanged is omitted', () => {
    renderEditor(undefined);
    expect(() => setSelection(new Set([1]))).not.toThrow();
  });

  it('fires (with []) when a navResetSeq bump clears the selection (undo-driven reset)', () => {
    // The undo-driven navigation reset clears selection/details/tool via the
    // navReset effect, never routing through handleSelection. The post-commit
    // selection effect still observes the committed clear and notifies the host.
    const callback = jest.fn();
    renderEditor(callback);

    setSelection(new Set([1]));
    callback.mockClear();

    // Restoring a project that no longer contains the viewed model bumps
    // navResetSeq; the Editor's navReset effect clears the selection.
    publish(makeSnapshot(1));

    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith([]);
  });
});
