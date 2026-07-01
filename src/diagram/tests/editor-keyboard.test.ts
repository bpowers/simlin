/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Editor-level keyboard shortcuts beyond undo/redo, and the graceful
// details-panel degradation for a model/view divergence:
//
//  1. Delete/Backspace deletes the current selection -- the ONLY delete
//     affordance for unnamed elements (clouds) and for elements whose details
//     panel cannot open.
//  2. Escape disarms the active creation tool first, then clears selection.
//  3. Keys typed into editable fields never trigger these.
//  4. A selected view element whose variable is MISSING from the model (a
//     corrupted/divergent project) must not crash the editor when the details
//     panel opens; it degrades to no panel so the element can still be
//     selected and keyboard-deleted (the repair path).

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';

import type { StockFlowView, Variable } from '@simlin/core/datamodel';

import { ProjectController, type ProjectSnapshot } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';

// Mock SpeedDial as in editor-tool-deselect.test.ts so tools are queryable.
jest.mock('../components/SpeedDial', () => {
  const react = jest.requireActual('react') as typeof import('react');
  return {
    __esModule: true,
    default: (p: { children?: React.ReactNode; onClick?: (e: unknown) => void }) =>
      react.createElement(
        'div',
        null,
        react.createElement('button', {
          type: 'button',
          'aria-label': 'dial-fab',
          onClick: (e: unknown) => p.onClick?.(e),
        }),
        p.children,
      ),
    SpeedDialAction: (p: { title: string; selected?: boolean; onClick?: (e: unknown) => void }) =>
      react.createElement('button', {
        type: 'button',
        'aria-label': p.title,
        'data-selected': p.selected ? 'true' : 'false',
        onClick: (e: unknown) => p.onClick?.(e),
      }),
    SpeedDialIcon: () => null,
  };
});

// Capture the live Canvas props so tests can drive selection callbacks and
// observe the selection the Editor passes back down.
let canvasProps: CanvasProps | undefined;
jest.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CanvasProps) => {
    canvasProps = p;
    return null;
  },
  inCreationUid: -2,
}));

// The details panel itself is not under test; render a marker so tests can
// assert presence/absence without pulling in the full VariableDetails tree.
jest.mock('../VariableDetails', () => ({
  __esModule: true,
  VariableDetails: () => {
    const react = jest.requireActual('react') as typeof import('react');
    return react.createElement('div', { 'data-testid': 'variable-details' });
  },
}));

import { Editor, type EditorProps } from '../Editor';

function makeView(): StockFlowView {
  return {
    nextUid: 20,
    elements: [
      {
        type: 'aux',
        uid: 9,
        name: 'some var',
        ident: 'some_var',
        var: undefined,
        x: 100,
        y: 100,
        labelSide: 'right',
        isZeroRadius: false,
      },
      // A "ghost": present in the view, but its variable is missing from the
      // model's variables map (the corrupted-project shape).
      {
        type: 'aux',
        uid: 10,
        name: 'ghost',
        ident: 'ghost',
        var: undefined,
        x: 200,
        y: 100,
        labelSide: 'right',
        isZeroRadius: false,
      },
    ],
    viewBox: { x: 0, y: 0, width: 800, height: 600 },
    zoom: 1,
    useLetteredPolarity: false,
  };
}

function makeSnapshot(): ProjectSnapshot {
  const someVar: Variable = {
    type: 'aux',
    ident: 'some_var',
    equation: { type: 'scalar', equation: '1' },
    documentation: '',
    units: '',
    gf: undefined,
    canBeModuleInput: false,
    isPublic: false,
    aiState: undefined,
    data: undefined,
    errors: [],
    unitErrors: [],
    uid: 9,
  } as unknown as Variable;
  return {
    project: {
      name: 'test-project',
      models: new Map([
        [
          'main',
          {
            name: 'main',
            variables: new Map([['some_var', someVar]]),
            views: [makeView()],
            loopMetadata: [],
            groups: [],
          },
        ],
      ]),
      simSpecs: { start: 0, stop: 100, dt: { isReciprocal: false, value: 1 }, timeUnits: 'years' },
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

function renderEditor(props: EditorProps = makeProps()): void {
  act(() => {
    render(React.createElement(Editor, props));
  });
}

const toolSelected = (title: string): boolean => screen.getByLabelText(title).getAttribute('data-selected') === 'true';

describe('Editor keyboard shortcuts', () => {
  let applyPatchCalls: unknown[];
  let updateViewCalls: StockFlowView[];

  beforeEach(() => {
    canvasProps = undefined;
    applyPatchCalls = [];
    updateViewCalls = [];
    jest.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    jest.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    jest.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
    jest.spyOn(ProjectController.prototype, 'getEngine').mockReturnValue({} as never);
    jest.spyOn(ProjectController.prototype, 'applyPatchOrReportError').mockImplementation(async (patch) => {
      applyPatchCalls.push(patch);
      return true;
    });
    jest.spyOn(ProjectController.prototype, 'updateView').mockImplementation(async (view) => {
      updateViewCalls.push(view);
    });
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  function selectUid(uid: number): void {
    act(() => {
      canvasProps?.onSetSelection(new Set([uid]));
    });
  }

  it('Delete removes the selected element (deleteVariable op + view update)', async () => {
    renderEditor();
    selectUid(9);

    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
    });

    expect(applyPatchCalls).toHaveLength(1);
    expect(JSON.stringify(applyPatchCalls[0])).toContain('"deleteVariable"');
    expect(JSON.stringify(applyPatchCalls[0])).toContain('some_var');
    expect(updateViewCalls).toHaveLength(1);
    expect(updateViewCalls[0].elements.some((el) => el.uid === 9)).toBe(false);
    // The selection was cleared alongside.
    expect(canvasProps?.selection.size).toBe(0);
  });

  it('Backspace deletes too', async () => {
    renderEditor();
    selectUid(9);

    await act(async () => {
      fireEvent.keyDown(document, { key: 'Backspace' });
    });

    expect(updateViewCalls).toHaveLength(1);
  });

  it('Delete with no selection is a no-op', async () => {
    renderEditor();

    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
    });

    expect(applyPatchCalls).toHaveLength(0);
    expect(updateViewCalls).toHaveLength(0);
  });

  it('Delete in readOnlyMode is a no-op', async () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    selectUid(9);

    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
    });

    expect(applyPatchCalls).toHaveLength(0);
    expect(updateViewCalls).toHaveLength(0);
  });

  it('Delete typed in an editable field does not delete the selection', async () => {
    renderEditor();
    selectUid(9);

    const input = document.createElement('input');
    document.body.appendChild(input);
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Delete' });
    });

    expect(updateViewCalls).toHaveLength(0);
    input.remove();
  });

  it('Escape disarms the active tool before touching the selection', () => {
    renderEditor();
    fireEvent.click(screen.getByLabelText('dial-fab'));
    fireEvent.click(screen.getByLabelText('Flow'));
    expect(toolSelected('Flow')).toBe(true);
    selectUid(9);

    act(() => {
      fireEvent.keyDown(document, { key: 'Escape' });
    });
    expect(toolSelected('Flow')).toBe(false);
    // Selection untouched on the first Escape.
    expect(canvasProps?.selection.has(9)).toBe(true);

    act(() => {
      fireEvent.keyDown(document, { key: 'Escape' });
    });
    expect(canvasProps?.selection.size).toBe(0);
  });

  it('the ghost element (variable missing from the model) can be selected and keyboard-deleted', async () => {
    // Before the hardening, selecting the ghost with the details panel open
    // crashed the whole editor in render (getOrThrow on the missing variable),
    // which ALSO made the element undeletable (the panel is the only other
    // delete affordance).
    renderEditor();
    selectUid(10);

    // Open the details panel the way Canvas does after a clean click.
    act(() => {
      canvasProps?.onShowVariableDetails();
    });

    // No crash: the editor is still rendering (the canvas is alive) and no
    // details panel appears for the ghost.
    expect(canvasProps).toBeDefined();
    expect(screen.queryByTestId('variable-details')).toBeNull();

    // The repair path: keyboard-delete the ghost. Its variable is missing, so
    // no deleteVariable op is emitted -- but the view element must go.
    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
    });
    expect(updateViewCalls).toHaveLength(1);
    expect(updateViewCalls[0].elements.some((el) => el.uid === 10)).toBe(false);
  });

  it('the details panel still renders for a healthy variable', () => {
    renderEditor();
    selectUid(9);
    act(() => {
      canvasProps?.onShowVariableDetails();
    });
    expect(screen.queryByTestId('variable-details')).not.toBeNull();
  });
});
