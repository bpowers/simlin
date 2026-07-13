// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// readOnlyMode is the app's "viewing someone else's public project" state
// (issue #935). Persistence is already a host-side no-op there, so every LOCAL
// mutation affordance must be inert too -- otherwise the editor is "editable
// but unsavable" and edits silently evaporate. These tests pin the Editor-level
// capability gate:
//
//  - every Canvas mutation callback is a no-op (no controller mutation calls);
//  - the creation toolbar (SpeedDial) and UndoRedoBar are hidden, and keyboard
//    undo/redo is inert even when history exists;
//  - what stays ALIVE: selection, pan/zoom (viewbox updates), and opening the
//    details panel for inspection (rendered read-only);
//  - the persistent "View only" pill reflects the CURRENT readOnlyMode: it
//    appears/disappears as the prop flips (replacing the once-latched mount
//    toast, which could go stale for an owner whose identity resolved late);
//  - flips in BOTH directions are deterministic: editable -> read-only disarms
//    the creation tool and remounts an open details panel as read-only
//    (discarding in-flight panel edits); read-only -> editable restores the
//    affordances without resurrecting a previously-armed tool.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, fireEvent, render, screen, type RenderResult } from '@testing-library/react';

// Mock factories are hoisted above the imports and so cannot close over `React`
// above. This attribute is rstest's synchronous stand-in for jest.requireActual:
// the binding resolves to the real module and is hoisted alongside the factory.
import * as react from 'react' with { rstest: 'importActual' };

import type {
  FlowViewElement,
  GraphicalFunction,
  LinkViewElement,
  StockFlowView,
  Variable,
} from '@simlin/core/datamodel';

import { ProjectController, type ProjectSnapshot } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';
import type { VariableDetails as VariableDetailsType } from '../VariableDetails';
import type { ModuleDetails as ModuleDetailsType } from '../ModuleDetails';

// Mock SpeedDial as in editor-keyboard.test.ts so tools are queryable.
rs.mock('../components/SpeedDial', () => {
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

// Capture the live Canvas props so tests can drive the Editor's handlers
// through the documented Canvas -> Editor contract.
let canvasProps: CanvasProps | undefined;
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CanvasProps) => {
    canvasProps = p;
    return null;
  },
  inCreationUid: -2,
}));

// Record the props the Editor hands VariableDetails plus a mount counter: a
// readOnly flip must REMOUNT the panel (key change) so its Slate editors
// re-seed from the committed model, discarding in-flight text.
type VariableDetailsProps = React.ComponentProps<typeof VariableDetailsType>;
let variableDetailsProps: VariableDetailsProps | undefined;
let variableDetailsMounts = 0;
rs.mock('../VariableDetails', () => ({
  __esModule: true,
  VariableDetails: (p: VariableDetailsProps) => {
    variableDetailsProps = p;
    react.useEffect(() => {
      variableDetailsMounts += 1;
    }, []);
    return react.createElement('div', { 'data-testid': 'variable-details' });
  },
}));

// Same for ModuleDetails, so the module-side handler guards are exercisable:
// the Editor passes its REAL handlers to the panel even in read-only mode (the
// panel renders them inert), so invoking these props directly probes the
// Editor-internal isReadOnly() guards, not the noop swap at the Canvas edge.
type ModuleDetailsProps = React.ComponentProps<typeof ModuleDetailsType>;
let moduleDetailsProps: ModuleDetailsProps | undefined;
rs.mock('../ModuleDetails', () => ({
  __esModule: true,
  ModuleDetails: (p: ModuleDetailsProps) => {
    moduleDetailsProps = p;
    return react.createElement('div', { 'data-testid': 'module-details' });
  },
}));

// Capture the drawer props so the sim-specs read-only wiring is observable.
interface DrawerPropsLike {
  readOnly?: boolean;
  onDelete?: () => Promise<void>;
  onDownloadXmile: () => void;
  onSimSpecCommit: (field: 'startTime' | 'stopTime' | 'dt' | 'timeUnits', value: number | string) => void;
}
let drawerProps: DrawerPropsLike | undefined;
rs.mock('../ModelPropertiesDrawer', () => ({
  __esModule: true,
  ModelPropertiesDrawer: (p: DrawerPropsLike) => {
    drawerProps = p;
    return null;
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
      {
        type: 'module',
        uid: 11,
        name: 'mod',
        ident: 'mod',
        var: undefined,
        x: 300,
        y: 100,
        labelSide: 'bottom',
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
  const modVar: Variable = {
    type: 'module',
    ident: 'mod',
    modelName: 'sub',
    references: [],
    documentation: '',
    units: '',
    canBeModuleInput: false,
    isPublic: false,
    dataSource: undefined,
    data: undefined,
    errors: [],
    unitErrors: [],
    uid: 11,
  } as unknown as Variable;
  return {
    project: {
      name: 'test-project',
      models: new Map([
        [
          'main',
          {
            name: 'main',
            variables: new Map([
              ['some_var', someVar],
              ['mod', modVar],
            ]),
            views: [makeView()],
            loopMetadata: [],
            groups: [],
          },
        ],
        ['sub', { name: 'sub', variables: new Map(), views: [], loopMetadata: [], groups: [] }],
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
    // History exists, so undo/redo gating is observable (not vacuously off).
    canUndo: true,
    canRedo: true,
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

const viewOnlyLabel = 'View only';

describe('Editor readOnlyMode capability gate', () => {
  let mutationSpies: {
    applyPatch: ReturnType<typeof rs.spyOn>;
    applyPatchOrReportError: ReturnType<typeof rs.spyOn>;
    updateView: ReturnType<typeof rs.spyOn>;
    undoRedo: ReturnType<typeof rs.spyOn>;
  };
  let queueViewUpdateSpy: ReturnType<typeof rs.spyOn>;

  beforeEach(() => {
    canvasProps = undefined;
    variableDetailsProps = undefined;
    moduleDetailsProps = undefined;
    variableDetailsMounts = 0;
    drawerProps = undefined;
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
    rs.spyOn(ProjectController.prototype, 'getEngine').mockReturnValue({} as never);
    mutationSpies = {
      applyPatch: rs.spyOn(ProjectController.prototype, 'applyPatch').mockResolvedValue(true),
      applyPatchOrReportError: rs.spyOn(ProjectController.prototype, 'applyPatchOrReportError').mockResolvedValue(true),
      updateView: rs.spyOn(ProjectController.prototype, 'updateView').mockResolvedValue(undefined),
      undoRedo: rs.spyOn(ProjectController.prototype, 'undoRedo').mockImplementation(() => {}),
    };
    queueViewUpdateSpy = rs.spyOn(ProjectController.prototype, 'queueViewUpdate').mockResolvedValue(undefined);
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function renderEditor(props: EditorProps = makeProps()): RenderResult {
    let result!: RenderResult;
    act(() => {
      result = render(React.createElement(Editor, props));
    });
    return result;
  }

  function expectNoMutations(): void {
    expect(mutationSpies.applyPatch).not.toHaveBeenCalled();
    expect(mutationSpies.applyPatchOrReportError).not.toHaveBeenCalled();
    expect(mutationSpies.updateView).not.toHaveBeenCalled();
    expect(mutationSpies.undoRedo).not.toHaveBeenCalled();
  }

  // Drive every mutation-capable Canvas callback plus the keyboard paths --
  // the "barrage". In read-only mode none of them may reach the controller.
  async function fireMutationBarrage(): Promise<void> {
    const cp = canvasProps!;
    await act(async () => {
      cp.onRenameVariable('some var', 'renamed');
      cp.onMoveSelection({ x: 10, y: 10 });
      cp.onMoveLabel(9, 'left');
      cp.onCreateVariable({
        type: 'aux',
        uid: -2,
        name: 'new one',
        ident: 'new_one',
        var: undefined,
        x: 5,
        y: 5,
        labelSide: 'right',
        isZeroRadius: false,
      });
      cp.onMoveFlow(
        { type: 'flow', uid: 12, points: [] } as unknown as FlowViewElement,
        9,
        { x: 0, y: 0 },
        undefined,
        true,
      );
      cp.onAttachLink({ type: 'link', uid: -2, fromUid: 9 } as unknown as LinkViewElement, 'some_var');
      void cp.onDeleteSelection();
    });
    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
      fireEvent.keyDown(document, { key: 'z', ctrlKey: true });
      fireEvent.keyDown(document, { key: 'z', ctrlKey: true, shiftKey: true });
      fireEvent.keyDown(document, { key: 'y', ctrlKey: true });
    });
  }

  it('editable mode reaches the controller (spy sanity check)', async () => {
    renderEditor();
    await act(async () => {
      canvasProps!.onCreateVariable({
        type: 'aux',
        uid: -2,
        name: 'new one',
        ident: 'new_one',
        var: undefined,
        x: 5,
        y: 5,
        labelSide: 'right',
        isZeroRadius: false,
      });
    });
    expect(mutationSpies.applyPatchOrReportError).toHaveBeenCalled();
    expect(mutationSpies.updateView).toHaveBeenCalled();

    await act(async () => {
      fireEvent.keyDown(document, { key: 'z', ctrlKey: true });
    });
    expect(mutationSpies.undoRedo).toHaveBeenCalledWith('undo');
  });

  it('a barrage of attempted mutations never reaches the controller in readOnlyMode', async () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    // Give the barrage a selection to chew on: selection itself must stay live.
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
    });
    await fireMutationBarrage();
    expectNoMutations();
  });

  // The three tests below probe the DEFENSE-IN-DEPTH guards inside the
  // Editor's op-building handlers, not the noop swap at the Canvas edge: the
  // Editor hands its REAL handlers to the details panels and drawer even in
  // read-only mode (the panels render them inert), and a handler captured
  // before a flip is a REAL handler too. Removing any internal isReadOnly()/
  // readOnlyMode guard makes one of these fail; the barrage above cannot see
  // that (it only exercises the noop'd Canvas wiring).

  it('the variable-panel and sim-spec handlers are themselves inert in readOnlyMode', async () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
      canvasProps!.onShowVariableDetails();
    });
    expect(variableDetailsProps).toBeDefined();
    expect(drawerProps).toBeDefined();

    const gf: GraphicalFunction = {
      kind: 'continuous',
      xScale: { min: 0, max: 1 },
      yScale: { min: 0, max: 1 },
      xPoints: undefined,
      yPoints: [0, 1],
    };
    await act(async () => {
      void variableDetailsProps!.onEquationChange('some_var', '2', 'widgets', 'docs');
      void variableDetailsProps!.onTableChange('some_var', gf);
      drawerProps!.onSimSpecCommit('startTime', 1900);
      drawerProps!.onSimSpecCommit('timeUnits', 'months');
    });
    expectNoMutations();
  });

  it('the module-panel handlers are themselves inert in readOnlyMode', async () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    act(() => {
      canvasProps!.onSetSelection(new Set([11]));
      canvasProps!.onShowVariableDetails();
    });
    expect(moduleDetailsProps).toBeDefined();
    expect(moduleDetailsProps!.readOnly).toBe(true);

    await act(async () => {
      void moduleDetailsProps!.onModelReferenceChange('mod', 'sub');
      void moduleDetailsProps!.onUnitsDocsChange('mod', 'widgets', 'docs');
      void moduleDetailsProps!.onReferencesChange('mod', [{ src: 'some_var', dst: 'mod·input' }]);
      void moduleDetailsProps!.onCreateModel('mod');
      void moduleDetailsProps!.onDuplicateModel('mod', 'sub');
    });
    expectNoMutations();
  });

  it('handlers captured before a flip to read-only cannot mutate after it (stale-closure defense)', async () => {
    const result = renderEditor(makeProps({ readOnlyMode: false }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
    });
    // These are the REAL bound handlers -- the editable render wired them.
    const staleHandlers = canvasProps!;
    act(() => {
      result.rerender(React.createElement(Editor, makeProps({ readOnlyMode: true })));
    });

    await act(async () => {
      staleHandlers.onRenameVariable('some var', 'renamed');
      staleHandlers.onMoveSelection({ x: 10, y: 10 });
      staleHandlers.onMoveLabel(9, 'left');
      staleHandlers.onCreateVariable({
        type: 'aux',
        uid: -2,
        name: 'new one',
        ident: 'new_one',
        var: undefined,
        x: 5,
        y: 5,
        labelSide: 'right',
        isZeroRadius: false,
      });
      staleHandlers.onMoveFlow(
        { type: 'flow', uid: 12, points: [] } as unknown as FlowViewElement,
        9,
        { x: 0, y: 0 },
        undefined,
        true,
      );
      staleHandlers.onAttachLink({ type: 'link', uid: -2, fromUid: 9 } as unknown as LinkViewElement, 'some_var');
      void staleHandlers.onDeleteSelection();
    });
    expectNoMutations();
  });

  it('Canvas is told it is read-only and gets no creation tool', () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    expect(canvasProps!.readOnly).toBe(true);
    expect(canvasProps!.selectedTool).toBeUndefined();
  });

  it('selection and pan/zoom stay live in readOnlyMode', async () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
    });
    expect(canvasProps!.selection.has(9)).toBe(true);

    await act(async () => {
      void canvasProps!.onViewBoxChange({ x: 5, y: 5, width: 800, height: 600 }, 1.5);
    });
    // Viewport updates are view-only (never recorded in undo history, and the
    // host's save is a no-op), so panning a read-only project is allowed.
    expect(queueViewUpdateSpy).toHaveBeenCalled();
    expectNoMutations();
  });

  it('hides the SpeedDial toolbar and the UndoRedoBar in readOnlyMode', () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    expect(screen.queryByLabelText('dial-fab')).toBeNull();
    expect(screen.queryByLabelText('Undo')).toBeNull();
    expect(screen.queryByLabelText('Redo')).toBeNull();
  });

  it('shows the SpeedDial toolbar and the UndoRedoBar when editable', () => {
    renderEditor();
    expect(screen.queryByLabelText('dial-fab')).not.toBeNull();
    expect(screen.queryByLabelText('Undo')).not.toBeNull();
  });

  it('opening the details panel for inspection still works, rendered read-only', () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
      canvasProps!.onShowVariableDetails();
    });
    expect(screen.queryByTestId('variable-details')).not.toBeNull();
    expect(variableDetailsProps!.readOnly).toBe(true);
  });

  it('the drawer receives readOnly (sim-specs) but keeps the download affordance', () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    expect(drawerProps).toBeDefined();
    expect(drawerProps!.readOnly).toBe(true);
    expect(typeof drawerProps!.onDownloadXmile).toBe('function');
  });

  it('shows the persistent "View only" pill instead of the transient toast', () => {
    renderEditor(makeProps({ readOnlyMode: true }));
    const pill = screen.queryByText(viewOnlyLabel);
    expect(pill).not.toBeNull();
    // role="status" makes the pill a polite live region, so a mid-session flip
    // to read-only is announced to assistive tech; the aria-label carries the
    // full explanation so it is not tooltip-only.
    expect(pill!.getAttribute('role')).toBe('status');
    expect(pill!.getAttribute('aria-label')).toBeTruthy();
    expect(screen.queryByText(/read-only version/i)).toBeNull();
  });

  it('shows no pill when editable', () => {
    renderEditor();
    expect(screen.queryByText(viewOnlyLabel)).toBeNull();
  });
});

describe('Editor readOnlyMode flips (both directions)', () => {
  beforeEach(() => {
    canvasProps = undefined;
    variableDetailsProps = undefined;
    moduleDetailsProps = undefined;
    variableDetailsMounts = 0;
    drawerProps = undefined;
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
    rs.spyOn(ProjectController.prototype, 'getEngine').mockReturnValue({} as never);
    rs.spyOn(ProjectController.prototype, 'applyPatch').mockResolvedValue(true);
    rs.spyOn(ProjectController.prototype, 'applyPatchOrReportError').mockResolvedValue(true);
    rs.spyOn(ProjectController.prototype, 'updateView').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'undoRedo').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'queueViewUpdate').mockResolvedValue(undefined);
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function renderEditor(props: EditorProps): RenderResult {
    let result!: RenderResult;
    act(() => {
      result = render(React.createElement(Editor, props));
    });
    return result;
  }

  function setReadOnly(result: RenderResult, readOnlyMode: boolean): void {
    act(() => {
      result.rerender(React.createElement(Editor, makeProps({ readOnlyMode })));
    });
  }

  const toolSelected = (title: string): boolean =>
    screen.getByLabelText(title).getAttribute('data-selected') === 'true';

  it('read-only -> editable: pill disappears, affordances appear, no stale toast', () => {
    const result = renderEditor(makeProps({ readOnlyMode: true }));
    expect(screen.queryByText(viewOnlyLabel)).not.toBeNull();
    expect(screen.queryByLabelText('dial-fab')).toBeNull();

    setReadOnly(result, false);
    expect(screen.queryByText(viewOnlyLabel)).toBeNull();
    expect(screen.queryByText(/read-only version/i)).toBeNull();
    expect(screen.queryByLabelText('dial-fab')).not.toBeNull();
    expect(screen.queryByLabelText('Undo')).not.toBeNull();
    expect(canvasProps!.readOnly).toBe(false);
  });

  it('editable -> read-only: pill appears, tools/handlers gate immediately', async () => {
    const result = renderEditor(makeProps({ readOnlyMode: false }));
    expect(screen.queryByText(viewOnlyLabel)).toBeNull();

    setReadOnly(result, true);
    expect(screen.queryByText(viewOnlyLabel)).not.toBeNull();
    expect(screen.queryByLabelText('dial-fab')).toBeNull();
    expect(canvasProps!.readOnly).toBe(true);
    expect(canvasProps!.selectedTool).toBeUndefined();

    const updateView = ProjectController.prototype.updateView as unknown as ReturnType<typeof rs.fn>;
    await act(async () => {
      canvasProps!.onMoveSelection({ x: 10, y: 10 });
    });
    expect(updateView).not.toHaveBeenCalled();
  });

  it('an armed creation tool is disarmed by the flip and does not re-arm on flipping back', () => {
    const result = renderEditor(makeProps({ readOnlyMode: false }));
    fireEvent.click(screen.getByLabelText('dial-fab'));
    fireEvent.click(screen.getByLabelText('Flow'));
    expect(toolSelected('Flow')).toBe(true);
    expect(canvasProps!.selectedTool).toBe('flow');

    setReadOnly(result, true);
    expect(canvasProps!.selectedTool).toBeUndefined();

    setReadOnly(result, false);
    expect(toolSelected('Flow')).toBe(false);
    expect(canvasProps!.selectedTool).toBeUndefined();
  });

  it('selection is preserved across a flip to read-only', () => {
    const result = renderEditor(makeProps({ readOnlyMode: false }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
    });
    setReadOnly(result, true);
    expect(canvasProps!.selection.has(9)).toBe(true);
  });

  it('an open details panel remounts read-only on the flip (in-flight edits discarded)', () => {
    const result = renderEditor(makeProps({ readOnlyMode: false }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
      canvasProps!.onShowVariableDetails();
    });
    expect(variableDetailsMounts).toBe(1);
    expect(variableDetailsProps!.readOnly).toBe(false);

    setReadOnly(result, true);
    // The panel stays open for inspection but REMOUNTS (key change), so its
    // Slate editors re-seed from the committed model -- any in-flight typed
    // text is deterministically discarded, not silently half-kept.
    expect(variableDetailsMounts).toBe(2);
    expect(variableDetailsProps!.readOnly).toBe(true);
  });

  it('keyboard delete works again after flipping back to editable', async () => {
    const result = renderEditor(makeProps({ readOnlyMode: true }));
    act(() => {
      canvasProps!.onSetSelection(new Set([9]));
    });
    const updateView = ProjectController.prototype.updateView as unknown as ReturnType<typeof rs.fn>;
    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
    });
    expect(updateView).not.toHaveBeenCalled();

    setReadOnly(result, false);
    await act(async () => {
      fireEvent.keyDown(document, { key: 'Delete' });
    });
    expect(updateView).toHaveBeenCalled();
  });
});
