/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Issue #529 (rescoped): the Editor handlers that mutate `selection` directly
// (delete, create, flow/link attach, navigation) must honor the same
// empty-selection invariant handleSelection always enforced -- when the
// committed selection empties, the details panel must close (showDetails ->
// undefined) and the variable-details tab must reset. Previously the delete and
// create handlers cleared `selection` but left showDetails at 'variable'. That
// was masked because getDetails() also guards on a named selected element, but
// the stale state was observable: after such a clear, a *plain single-select*
// of another (or the same) element pops the details panel open on mere
// selection -- even though selection alone should never open the panel.
//
// selectionStatePatch is the pure funnel; every selection-assigning setState
// composes it. This test pins both the pure logic and the observable render
// consequence in the real Editor (Canvas + VariableDetails mocked; a real
// ProjectController whose engine-touching methods are stubbed).

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';

import { projectFromJson, type JsonProject } from '@simlin/core/datamodel';
import { ProjectController, type ProjectSnapshot } from '../project-controller';

// A project with a single aux 'x' (uid 1) drawn in the main view, so the
// Editor's details flow resolves to a VariableDetails panel when 'x' is
// selected and showDetails is 'variable'.
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

// Render queryable markers for the two side panels so a test can assert which
// (if either) is showing without depending on their internals. The errors panel
// (ErrorDetails) is model-level and independent of the selection; the variable
// panel (VariableDetails) is selection-tied.
jest.mock('../VariableDetails', () => ({
  __esModule: true,
  VariableDetails: () => React.createElement('div', { 'data-testid': 'var-details' }),
}));
jest.mock('../ErrorDetails', () => ({
  __esModule: true,
  ErrorDetails: () => React.createElement('div', { 'data-testid': 'error-details' }),
}));

// A clickable stand-in for the Status dot so a test can open the errors panel
// (its onClick is the Editor's handleStatusClick, which toggles showDetails to
// 'errors'). The real Status renders an <svg><circle> that is awkward to target.
jest.mock('../Status', () => ({
  __esModule: true,
  Status: ({ onClick }: { onClick: () => void }) =>
    React.createElement('button', { 'data-testid': 'status-toggle', onClick }),
}));

// Capture the props the Editor hands the Canvas so we can drive the real
// selection/show-details/delete/create handlers (the documented Canvas ->
// Editor contract) without WASM, jsdom SVG geometry, or a ResizeObserver.
interface CapturedCanvasProps {
  onSetSelection: (sel: ReadonlySet<number>) => void;
  onShowVariableDetails: () => void;
  onDeleteSelection: () => Promise<void> | void;
  onCreateVariable: (element: unknown) => Promise<void> | void;
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

import { Editor, selectionStatePatch, type EditorProps } from '../Editor';

function makeSnapshot(): ProjectSnapshot {
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

describe('selectionStatePatch (pure funnel)', () => {
  it('closes the variable panel and resets the tab when an empty selection had a variable panel open', () => {
    const patch = selectionStatePatch(new Set<number>(), 'variable');
    expect(patch.selection.size).toBe(0);
    // showDetails is present-and-undefined (spread must overwrite the stale
    // 'variable'), not merely absent.
    expect('showDetails' in patch).toBe(true);
    expect(patch.showDetails).toBeUndefined();
    expect(patch.variableDetailsActiveTab).toBe(0);
  });

  it('leaves an open errors panel alone when the selection empties', () => {
    const patch = selectionStatePatch(new Set<number>(), 'errors');
    expect(patch.selection.size).toBe(0);
    // showDetails is absent -> the merge preserves the current 'errors': the
    // model-level error list must survive deleting a selected variable.
    expect('showDetails' in patch).toBe(false);
    // The tab still resets (harmless prep for the next variable panel).
    expect(patch.variableDetailsActiveTab).toBe(0);
  });

  it('does not reopen a panel when an empty selection had none open', () => {
    const patch = selectionStatePatch(new Set<number>(), undefined);
    expect('showDetails' in patch).toBe(false);
    expect(patch.variableDetailsActiveTab).toBe(0);
  });

  it('leaves showDetails and the tab untouched for a non-empty selection', () => {
    const patch = selectionStatePatch(new Set<number>([1, 2]), 'variable');
    expect([...patch.selection].sort()).toEqual([1, 2]);
    // Absent (not present-undefined): a non-empty select must not disturb an
    // intentionally-open panel or the tab the caller chose.
    expect('showDetails' in patch).toBe(false);
    expect('variableDetailsActiveTab' in patch).toBe(false);
  });
});

describe('Editor empty-selection invariant (issue #529)', () => {
  let snapshot: ProjectSnapshot;

  beforeEach(() => {
    capturedCanvasProps = undefined;
    snapshot = makeSnapshot();
    jest.spyOn(ProjectController.prototype, 'getSnapshot').mockImplementation(() => snapshot);
    // The Editor drives everything here through the captured Canvas handlers,
    // so the controller subscription is never fired; return a no-op unsubscribe.
    jest.spyOn(ProjectController.prototype, 'subscribe').mockImplementation(() => () => {});
    jest.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    // The delete/create handlers bail if the engine hasn't opened; the mocked
    // openInitialProject never opens one, so stub getEngine truthy and stub the
    // patch/view methods the handlers await so they proceed to their setState.
    jest.spyOn(ProjectController.prototype, 'getEngine').mockReturnValue({} as never);
    jest.spyOn(ProjectController.prototype, 'applyPatchOrReportError').mockResolvedValue(true);
    jest.spyOn(ProjectController.prototype, 'updateView').mockResolvedValue(undefined);
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  function openVariableDetails(): void {
    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([1]));
      capturedCanvasProps!.onShowVariableDetails();
    });
  }

  it('deleting the selection closes the panel so a later single-select does not reopen it', async () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    openVariableDetails();
    expect(screen.queryByTestId('var-details')).not.toBeNull();

    // Delete the selection: this empties it (and, via selectionStatePatch,
    // resets showDetails). The mocked snapshot is fixed, so 'x' remains
    // selectable afterward -- exactly what makes the stale showDetails
    // observable.
    await act(async () => {
      await capturedCanvasProps!.onDeleteSelection();
    });
    // With the selection empty the panel is gone regardless (guarded by the
    // named-selected-element check).
    expect(screen.queryByTestId('var-details')).toBeNull();

    // A plain single-select must NOT reopen the panel: only the show-details
    // affordance does. Before the fix, showDetails was still 'variable', so
    // this selection popped the panel open.
    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([1]));
    });
    expect(screen.queryByTestId('var-details')).toBeNull();
  });

  it('deleting a variable while the errors panel is open leaves the errors panel open', async () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });

    // Select a variable, then open the model-level errors panel (Status click).
    // The variable panel is NOT shown here (we never invoked show-details), so
    // showDetails is 'errors' with a non-empty selection.
    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([1]));
    });
    act(() => {
      fireEvent.click(screen.getByTestId('status-toggle'));
    });
    expect(screen.queryByTestId('error-details')).not.toBeNull();
    expect(screen.queryByTestId('var-details')).toBeNull();

    // Delete the selected variable mid error-triage. The selection empties, but
    // the errors panel is model-level and must STAY open.
    await act(async () => {
      await capturedCanvasProps!.onDeleteSelection();
    });
    expect(screen.queryByTestId('error-details')).not.toBeNull();
    expect(screen.queryByTestId('var-details')).toBeNull();
  });

  it('deleting a variable while ITS variable panel is open closes that panel (companion)', async () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    openVariableDetails();
    expect(screen.queryByTestId('var-details')).not.toBeNull();

    await act(async () => {
      await capturedCanvasProps!.onDeleteSelection();
    });
    // The selection-tied variable panel closes (contrast with the errors panel
    // above, which survives the same gesture).
    expect(screen.queryByTestId('var-details')).toBeNull();
  });

  it('creating a variable clears the selection so a later single-select does not reopen the panel', async () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    openVariableDetails();
    expect(screen.queryByTestId('var-details')).not.toBeNull();

    // handleCreateVariable clears the selection to empty; the invariant must
    // reset showDetails in that same commit.
    await act(async () => {
      await capturedCanvasProps!.onCreateVariable({ type: 'aux', uid: 0, name: 'y', x: 10, y: 10 });
    });
    expect(screen.queryByTestId('var-details')).toBeNull();

    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([1]));
    });
    expect(screen.queryByTestId('var-details')).toBeNull();
  });
});
