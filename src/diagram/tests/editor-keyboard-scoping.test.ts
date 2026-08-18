// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Keyboard scoping across Editor instances. The Editor listens for shortcuts at
// the document level (Delete/Backspace, Escape, undo/redo), which is fine when
// one Editor owns the whole page (src/app, simlin-serve) but not when several
// share one -- a Jupyter notebook with an Editor per output cell. The
// document-level handler must therefore act only when the key event belongs to
// THIS instance. The four arms of that decision (editor-key-scope.ts):
//
//  1. the event's composedPath() includes this Editor's root -> this instance;
//  2. the path includes ANOTHER Editor's root -> not this instance;
//  3. the path reaches no element besides <body>/<html> (focus is "nowhere",
//     which is where it falls when a DOM change unmounts the focused control --
//     a details panel closing on delete, the inline name editor committing) ->
//     the instance that most recently saw pointer/focus activity inside it, and
//     only that one;
//  4. the path holds some element outside every Editor (focus moved to the
//     host page) -> nobody.
//
// The existing "never in an editable element" rule stays in front of all four.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, fireEvent, render, type RenderResult } from '@testing-library/react';

import * as react from 'react' with { rstest: 'importActual' };

import type { StockFlowView, Variable } from '@simlin/core/datamodel';

import { ProjectController, type ProjectSnapshot } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';
import { EDITOR_ROOT_ATTRIBUTE, activeEditorRoot } from '../editor-key-scope';

rs.mock('../components/SpeedDial', () => {
  return {
    __esModule: true,
    default: (p: { children?: React.ReactNode }) => react.createElement('div', null, p.children),
    SpeedDialAction: () => null,
    SpeedDialIcon: () => null,
  };
});

// Each mocked Canvas renders a marker element and hangs its latest props on it,
// so a test with several Editors mounted can address each instance's canvas
// contract (selection in / selection out) through the DOM it owns.
const CANVAS_PROPS = 'canvasProps';
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CanvasProps) =>
    react.createElement('div', {
      'data-testid': 'canvas',
      ref: (el: (HTMLElement & Record<string, unknown>) | null) => {
        if (el) {
          el[CANVAS_PROPS] = p;
        }
      },
    }),
  inCreationUid: -2,
}));

rs.mock('../VariableDetails', () => ({
  __esModule: true,
  VariableDetails: () => null,
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
    serverVersion: 1,
    projectGeneration: 0,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelStack: [],
    canUndo: true,
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

// One mounted Editor instance as the test sees it: its root element (the
// Editor's outermost <div>), the controller the Editor built for it, and the
// mocked Canvas's live props.
interface Instance {
  result: RenderResult;
  root: HTMLElement;
  controller: ProjectController;
  canvas: () => CanvasProps;
}

describe('Editor keyboard scoping across instances', () => {
  // The controllers each Editor constructs, in mount order: the `subscribe`
  // spy records `this`, which is how a test learns which instance a
  // prototype-level spy (undoRedo) was invoked on.
  let controllers: ProjectController[];
  let undoRedoCalls: Array<{ controller: ProjectController; kind: string }>;
  let updateViewCalls: Array<{ controller: ProjectController; view: StockFlowView }>;

  beforeEach(() => {
    controllers = [];
    undoRedoCalls = [];
    updateViewCalls = [];
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockImplementation(function (this: ProjectController) {
      controllers.push(this);
      return () => {};
    });
    rs.spyOn(ProjectController.prototype, 'getEngine').mockReturnValue({} as never);
    rs.spyOn(ProjectController.prototype, 'applyPatchOrReportError').mockResolvedValue(true);
    rs.spyOn(ProjectController.prototype, 'updateView').mockImplementation(async function (
      this: ProjectController,
      view: StockFlowView,
    ) {
      updateViewCalls.push({ controller: this, view });
    });
    rs.spyOn(ProjectController.prototype, 'undoRedo').mockImplementation(function (
      this: ProjectController,
      kind: 'undo' | 'redo',
    ) {
      undoRedoCalls.push({ controller: this, kind });
    });
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function mountEditor(props: EditorProps = makeProps()): Instance {
    const before = controllers.length;
    let result!: RenderResult;
    act(() => {
      result = render(React.createElement(Editor, props));
    });
    const root = result.container.firstElementChild as HTMLElement;
    expect(root).not.toBeNull();
    // The root's scoping identity and its focus sink: the attribute is how
    // instances recognize each other in a path; tabindex=-1 is what makes a
    // click on non-focusable chrome settle focus here instead of on <body>.
    expect(root.hasAttribute(EDITOR_ROOT_ATTRIBUTE)).toBe(true);
    expect(root.getAttribute('tabindex')).toBe('-1');
    const controller = controllers[before];
    expect(controller).toBeDefined();
    const canvas = (): CanvasProps => {
      const el = result.container.querySelector('[data-testid="canvas"]') as
        | (HTMLElement & Record<string, unknown>)
        | null;
      expect(el).not.toBeNull();
      return el![CANVAS_PROPS] as CanvasProps;
    };
    return { result, root, controller, canvas };
  }

  function select(inst: Instance, uid: number): void {
    act(() => {
      inst.canvas().onSetSelection(new Set([uid]));
    });
  }

  // A pointer press anywhere inside the instance (the canvas, in real use).
  // The Canvas prevents the default focus change on pointerdown, so the press
  // itself -- not a focus change -- is what marks the instance active; the
  // container focus the Canvas applies on release is a separate step that
  // these tests (with a stub Canvas) do not perform, which is exactly the
  // focus-nowhere situation arm 3 exists for.
  function pressInside(inst: Instance): void {
    act(() => {
      fireEvent.pointerDown(inst.root);
    });
  }

  async function pressKey(target: EventTarget, init: KeyboardEventInit): Promise<void> {
    await act(async () => {
      fireEvent.keyDown(target, init);
    });
  }

  it("Delete on <body> after pressing in B deletes B's selection only (arm 3)", async () => {
    const a = mountEditor();
    const b = mountEditor();
    select(a, 9);
    select(b, 9);
    pressInside(a);
    pressInside(b);

    await pressKey(document.body, { key: 'Delete' });

    expect(updateViewCalls.map((c) => c.controller)).toEqual([b.controller]);
    expect(b.canvas().selection.size).toBe(0);
    expect(a.canvas().selection.has(9)).toBe(true);
  });

  it('Ctrl+Z on <body> after pressing in A undoes A only, and moves with the most recent press (arm 3)', async () => {
    const a = mountEditor();
    const b = mountEditor();
    pressInside(b);
    pressInside(a);

    await pressKey(document.body, { key: 'z', ctrlKey: true });
    expect(undoRedoCalls).toEqual([{ controller: a.controller, kind: 'undo' }]);

    pressInside(b);
    await pressKey(document.body, { key: 'z', ctrlKey: true });
    expect(undoRedoCalls.map((c) => c.controller)).toEqual([a.controller, b.controller]);
  });

  it('a key whose target is inside B acts on B even when A was the last pressed (arms 1 and 2)', async () => {
    const a = mountEditor();
    const b = mountEditor();
    select(a, 9);
    select(b, 9);
    pressInside(b);
    pressInside(a);

    // Dispatch on B's own root: composedPath() runs B.root -> body -> html ->
    // document -> window, so it names B and never A.
    await pressKey(b.root, { key: 'Delete' });

    expect(updateViewCalls.map((c) => c.controller)).toEqual([b.controller]);
    expect(b.canvas().selection.size).toBe(0);
    expect(a.canvas().selection.has(9)).toBe(true);
  });

  it('focus moving into an instance (keyboard navigation) marks it active (arm 3 via focus)', async () => {
    const a = mountEditor();
    const b = mountEditor();
    pressInside(a);

    // A focusable control inside B receives focus (as Tab would give it).
    const button = document.createElement('button');
    b.root.appendChild(button);
    act(() => {
      button.focus();
    });
    // The control then vanishes (a panel closes, a toolbar re-renders) and
    // focus falls back to <body>: the most recent activity was still B's.
    act(() => {
      button.remove();
    });
    expect(document.activeElement).toBe(document.body);

    await pressKey(document.body, { key: 'z', ctrlKey: true });
    expect(undoRedoCalls).toEqual([{ controller: b.controller, kind: 'undo' }]);
  });

  it('with no activity in any instance, a key on <body> reaches nobody (arm 3, no active instance)', async () => {
    const a = mountEditor();
    const b = mountEditor();
    select(a, 9);
    select(b, 9);

    await pressKey(document.body, { key: 'Delete' });
    await pressKey(document, { key: 'z', ctrlKey: true });

    expect(updateViewCalls).toEqual([]);
    expect(undoRedoCalls).toEqual([]);
    expect(a.canvas().selection.has(9)).toBe(true);
    expect(b.canvas().selection.has(9)).toBe(true);
  });

  it('a key targeting an element outside every Editor reaches nobody, even the last-active one (arm 4)', async () => {
    const a = mountEditor();
    select(a, 9);
    pressInside(a);

    // The host page's own control (a notebook toolbar button, say) takes the
    // key: focus has left the editors, so their shortcuts stay quiet.
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    await pressKey(outside, { key: 'Delete' });
    await pressKey(outside, { key: 'z', ctrlKey: true });
    outside.remove();

    expect(updateViewCalls).toEqual([]);
    expect(undoRedoCalls).toEqual([]);
    expect(a.canvas().selection.has(9)).toBe(true);
  });

  it('a key typed in an editable element inside the active instance is left to that element', async () => {
    const a = mountEditor();
    select(a, 9);
    pressInside(a);

    const input = document.createElement('input');
    a.root.appendChild(input);
    await pressKey(input, { key: 'Delete' });
    await pressKey(input, { key: 'z', ctrlKey: true });
    input.remove();

    expect(updateViewCalls).toEqual([]);
    expect(undoRedoCalls).toEqual([]);
  });

  it('unmounting the active instance releases it: keys on <body> then reach nobody until another press', async () => {
    const a = mountEditor();
    const b = mountEditor();
    pressInside(a);
    pressInside(b);
    expect(activeEditorRoot()).toBe(b.root);
    act(() => {
      b.result.unmount();
    });
    // The unmount cleanup released it; the slot does not silently fall back to
    // a sibling either.
    expect(activeEditorRoot()).toBeNull();

    await pressKey(document.body, { key: 'z', ctrlKey: true });
    expect(undoRedoCalls).toEqual([]);

    pressInside(a);
    await pressKey(document.body, { key: 'z', ctrlKey: true });
    expect(undoRedoCalls).toEqual([{ controller: a.controller, kind: 'undo' }]);
  });

  it('a single Editor still handles a key on <body> after a press inside it (the app/serve hosts)', async () => {
    const a = mountEditor();
    select(a, 9);
    pressInside(a);

    await pressKey(document.body, { key: 'Escape' });
    expect(a.canvas().selection.size).toBe(0);
  });
});
