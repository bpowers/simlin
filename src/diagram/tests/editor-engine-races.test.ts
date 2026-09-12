// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor's commit path against the REAL WASM engine, including edits that
// race an in-flight patch. The Editor mounts with the Canvas mocked (its props
// are captured, so tests call the gesture callbacks the Canvas would), the
// details panels real, and every engine patch delayed 30ms to emulate the
// worker round trip. After each scenario the engine's own serialized project
// must hold M1 (kind agreement), M3 (referential integrity), static stock/view
// agreement (every stock's lists name exactly the flows attached to it on the
// view, once), and "nothing invisible simulates" (every non-module variable has
// a primary element); the rendered view must equal the engine's view.
//
// The scenarios port the commit-path audit's races: two attaches onto one stock
// (T11), a create raced by a delete (T11b), an attach raced by a label move
// (T11c), an attach raced by a delete (T11e), undo pressed while an attach is in
// flight (T24), an attach onto a stock already listing the flow (T12), creating
// under a taken name (T6), two quick creates, deleting a stock with an alias
// (T23), deleting a lone cloud (T1), a non-finite geometry commit (T22), and
// typing in a details panel then dragging a stock. The Phase 4 review's repros
// follow: a second rename while the first is pending (R1), renaming a pending
// create (R3), typing in the panel of a variable whose rename is pending (R4), a
// patch that applied but could not be read back (R9), Delete right after Ctrl+Z
// (R10), two handlers in one tick, and Undo pressed with a draft in the panel.
// The delta review's follow: a patch whose read-back fails then re-reads (N4) or
// reopens while the user keeps editing (N1), an engine that cannot be reopened
// (N10), typed names committed while an undo is queued (N2), text typed after a
// flush (N5), a draft in the panel of a variable a rename rewrites, an unrelated
// landing while typing, Undo with a draft whose edit fails (N7), Redo with a
// draft (N8), and a swap of two names through three pending renames (O3).
//
// Gesture commits are planned by the production planner on the rendered view.
// What this does not establish: the Canvas turning pointer events into those
// gestures (canvas-gestures-*.test.tsx), or the strict geometry of the committed
// flows (editor-gestures-engine.test.ts).

import { it, expect, beforeAll, afterEach, rs } from '@rstest/core';

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
  if (!('getBoundingClientRect' in Range.prototype)) {
    const zero = () =>
      ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} }) as DOMRect;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Range.prototype as any).getBoundingClientRect = zero;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Range.prototype as any).getClientRects = () =>
      ({ length: 0, item: () => null, [Symbol.iterator]: function* () {} }) as unknown as DOMRectList;
  }
});

import * as React from 'react';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Editor as SlateEditor, Transforms } from 'slate';
import { ELEMENT_TO_NODE } from 'slate-dom';

import * as react from 'react' with { rstest: 'importActual' };

import { canonicalize } from '@simlin/core/canonicalize';
import {
  isNamedViewElement,
  projectFromJson,
  type FlowViewElement,
  type Model,
  type StockFlowView,
  type ViewElement,
} from '@simlin/core/datamodel';
import { Project as EngineProject, type JsonProject, type JsonProjectPatch } from '@simlin/engine';

import { ProjectController } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';
import { planGesture, sameGeometry, type Gesture } from '../gesture-planner';
import { describeWithEngine, loadEngine, mainModel, type EngineModule } from './support/engine';
import {
  checkKindAgreement,
  checkReferentialIntegrity,
  checkStockFlowAgreement,
  checkStockListDuplicates,
  formatViewViolations,
} from './support/view-invariants';

let canvasProps: CanvasProps | undefined;
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CanvasProps) => {
    canvasProps = p;
    return react.createElement('div', { 'data-testid': 'canvas' });
  },
  inCreationUid: -2,
}));

import { Editor, type EditorProps } from '../Editor';

// ---------------------------------------------------------------------------
// Fixture (the audit's): A -> B via f, g from a cloud into A, h from B to a
// cloud, k between two clouds, x reads A and feeds f, an alias of A feeding k.

function baseModel(): Record<string, unknown> & { stocks: Array<{ name: string; inflows: string[] }> } {
  return {
    name: 'main',
    stocks: [
      { name: 'A', initialEquation: '10', inflows: ['g'], outflows: ['f'] },
      { name: 'B', initialEquation: '0', inflows: ['f'], outflows: ['h'] },
    ],
    flows: [
      { name: 'g', equation: '1' },
      { name: 'f', equation: 'x' },
      { name: 'h', equation: 'B * 0.1' },
      { name: 'k', equation: '0' },
    ],
    auxiliaries: [{ name: 'x', equation: 'A * 0.1' }],
    views: [
      {
        elements: [
          { type: 'cloud', uid: 20, flowUid: 3, x: 0, y: 100 },
          {
            type: 'flow',
            uid: 3,
            name: 'g',
            x: 40,
            y: 100,
            points: [
              { x: 0, y: 100, attachedToUid: 20 },
              { x: 77.5, y: 100, attachedToUid: 1 },
            ],
          },
          { type: 'stock', uid: 1, name: 'A', x: 100, y: 100 },
          {
            type: 'flow',
            uid: 4,
            name: 'f',
            x: 200,
            y: 100,
            points: [
              { x: 122.5, y: 100, attachedToUid: 1 },
              { x: 277.5, y: 100, attachedToUid: 2 },
            ],
          },
          { type: 'stock', uid: 2, name: 'B', x: 300, y: 100 },
          {
            type: 'flow',
            uid: 5,
            name: 'h',
            x: 400,
            y: 100,
            points: [
              { x: 322.5, y: 100, attachedToUid: 2 },
              { x: 500, y: 100, attachedToUid: 21 },
            ],
          },
          { type: 'cloud', uid: 21, flowUid: 5, x: 500, y: 100 },
          { type: 'cloud', uid: 22, flowUid: 6, x: 0, y: 300 },
          {
            type: 'flow',
            uid: 6,
            name: 'k',
            x: 100,
            y: 300,
            points: [
              { x: 0, y: 300, attachedToUid: 22 },
              { x: 200, y: 300, attachedToUid: 23 },
            ],
          },
          { type: 'cloud', uid: 23, flowUid: 6, x: 200, y: 300 },
          { type: 'aux', uid: 7, name: 'x', x: 200, y: 20 },
          { type: 'link', uid: 8, fromUid: 7, toUid: 4 },
          { type: 'link', uid: 9, fromUid: 1, toUid: 7 },
          { type: 'alias', uid: 10, aliasOfUid: 1, x: 100, y: 250 },
          { type: 'link', uid: 11, fromUid: 10, toUid: 6 },
        ],
      },
    ],
  };
}

function projectJson(model: Record<string, unknown>): string {
  return JSON.stringify({ name: 'races', simSpecs: { startTime: 0, endTime: 3, dt: '1' }, models: [model] });
}

// ---------------------------------------------------------------------------
// Mounting

interface Mounted {
  readonly controller: ProjectController;
  readonly patches: JsonProjectPatch[];
  view(): StockFlowView;
  element(uid: number): ViewElement;
  settle(): Promise<void>;
  engineModel(): Promise<Model>;
  // The engine's next `count` project reads (with stdlib, as a read-back does)
  // throw, as a worker fault would.
  armReadFailure(count?: number): void;
  // The engine's next applyPatch rejects.
  armPatchFailure(): void;
}

const PATCH_LATENCY_MS = 30;

async function mount(json: string, extraProps: Partial<EditorProps> = {}): Promise<Mounted> {
  const controllers: ProjectController[] = [];
  const patches: JsonProjectPatch[] = [];
  const originalOpen = ProjectController.prototype.openInitialProject;
  rs.spyOn(ProjectController.prototype, 'openInitialProject').mockImplementation(function (this: ProjectController) {
    controllers.push(this);
    return originalOpen.call(this);
  });
  rs.spyOn(console, 'error').mockImplementation(() => {});

  const props = {
    inputFormat: 'json',
    initialProjectJson: json,
    initialProjectVersion: 1,
    name: 'races',
    onSave: async () => 1,
    ...extraProps,
  } as EditorProps;
  act(() => {
    render(React.createElement(Editor, props));
  });
  const controller = controllers[0];
  await act(async () => {
    await controller.whenIdle();
  });

  // Every patch from here on waits out a worker round trip first. The engine
  // is reached through a query so the wrapper is installed inside the executor.
  let readFailures = 0;
  let patchFailureArmed = false;
  await act(async () => {
    await controller.query(async (engine) => {
      const apply = engine.applyPatch.bind(engine);
      engine.applyPatch = async (patch, options) => {
        await new Promise((resolve) => setTimeout(resolve, PATCH_LATENCY_MS));
        if (patchFailureArmed) {
          patchFailureArmed = false;
          throw new Error('injected patch failure');
        }
        patches.push(patch);
        return apply(patch, options);
      };
      const serializeJson = engine.serializeJson.bind(engine);
      engine.serializeJson = async (format, includeStdlib) => {
        if (includeStdlib && readFailures > 0) {
          readFailures -= 1;
          throw new Error('injected read failure');
        }
        return serializeJson(format, includeStdlib);
      };
    });
  });

  const view = (): StockFlowView => {
    if (canvasProps === undefined) {
      throw new Error('Canvas never rendered');
    }
    return canvasProps.view;
  };
  return {
    controller,
    patches,
    view,
    element: (uid) => {
      const el = view().elements.find((e) => e.uid === uid);
      if (el === undefined) {
        throw new Error(`no uid ${uid} on the rendered view`);
      }
      return el;
    },
    settle: async () => {
      for (let i = 0; i < 3; i++) {
        await act(async () => {
          await controller.whenIdle();
          await new Promise((resolve) => setTimeout(resolve, 0));
        });
      }
    },
    engineModel: async () => {
      const serialized = await controller.query((engine) => engine.serializeJson(undefined, true));
      return mainModel(projectFromJson(JSON.parse(serialized!) as JsonProject).models);
    },
    armReadFailure: (count = 1) => {
      readFailures = count;
    },
    armPatchFailure: () => {
      patchFailureArmed = true;
    },
  };
}

// The toasts the Editor shows.
function alerts(): string[] {
  return Array.from(document.querySelectorAll('[id="client-snackbar"]')).map((el) => el.textContent ?? '');
}

function nameOf(el: ViewElement): string | undefined {
  return isNamedViewElement(el) ? el.name : undefined;
}

function labelSideOf(model: Model, uid: number): string | undefined {
  return (model.views[0].elements.find((el) => el.uid === uid) as { labelSide?: string } | undefined)?.labelSide;
}

async function typeUnits(text: string): Promise<SlateEditor> {
  const unitsEl = document.querySelector('.unitsEditor') as HTMLElement;
  expect(unitsEl).not.toBeNull();
  const editor = ELEMENT_TO_NODE.get(unitsEl) as unknown as SlateEditor;
  await act(async () => {
    Transforms.insertText(editor, text, { at: SlateEditor.end(editor, []) });
    editor.onChange();
    await Promise.resolve();
  });
  return editor;
}

// The text in the open panel's units editor.
function unitsText(): string {
  return SlateEditor.string(
    ELEMENT_TO_NODE.get(document.querySelector('.unitsEditor') as HTMLElement) as unknown as SlateEditor,
    [],
  );
}

// A canvas press and release: the press flushes an open panel's draft.
function pressCanvas(): void {
  const canvas = screen.getByTestId('canvas');
  act(() => {
    fireEvent.pointerDown(canvas);
    fireEvent.pointerUp(canvas);
  });
}

// A press on an Undo/Redo button as a browser delivers it: React commits what
// the pointerdown changed before the click is dispatched.
function pressButton(label: string): void {
  const button = screen.getByLabelText(label) as HTMLButtonElement;
  expect(button.disabled).toBe(false);
  act(() => {
    fireEvent.pointerDown(button);
    fireEvent.mouseDown(button);
  });
  act(() => {
    fireEvent.pointerUp(button);
    fireEvent.mouseUp(button);
    fireEvent.click(button);
  });
}

// Replace the text of the panel field matching `selector`, as selecting it all
// and typing does.
async function setField(selector: string, text: string): Promise<void> {
  const editor = ELEMENT_TO_NODE.get(document.querySelector(selector) as HTMLElement) as unknown as SlateEditor;
  await act(async () => {
    Transforms.delete(editor, { at: { anchor: SlateEditor.start(editor, []), focus: SlateEditor.end(editor, []) } });
    if (text !== '') {
      Transforms.insertText(editor, text, { at: SlateEditor.end(editor, []) });
    }
    editor.onChange();
    await Promise.resolve();
  });
}

function fieldText(selector: string): string {
  return SlateEditor.string(
    ELEMENT_TO_NODE.get(document.querySelector(selector) as HTMLElement) as unknown as SlateEditor,
    [],
  );
}

async function waitFor(predicate: () => boolean): Promise<void> {
  for (let i = 0; i < 400 && !predicate(); i++) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
  expect(predicate()).toBe(true);
}

// ---------------------------------------------------------------------------
// Invariants over the engine's serialized project

function expectModelViewAgreement(model: Model): void {
  const view = model.views[0];
  const violations = [
    ...checkKindAgreement(view, model.variables),
    ...checkReferentialIntegrity(view),
    ...checkStockListDuplicates(model.variables),
    ...checkStockFlowAgreement(view, model.variables),
  ];
  expect(formatViewViolations(violations)).toBe('');
  const primaries = new Set(view.elements.filter(isNamedViewElement).map((el) => canonicalize(el.name)));
  for (const variable of model.variables.values()) {
    if (variable.type !== 'module') {
      expect(primaries.has(variable.ident)).toBe(true);
    }
    if (variable.type === 'stock') {
      for (const entry of [...variable.inflows, ...variable.outflows]) {
        expect(model.variables.get(canonicalize(entry))?.type).toBe('flow');
      }
    }
  }
}

// The rendered view (what the user sees) agrees with the engine's view: same
// elements, same flow attachments.
function expectRenderedMatchesEngine(rendered: StockFlowView, engine: StockFlowView): void {
  const shape = (view: StockFlowView) =>
    view.elements
      .map((el) =>
        el.type === 'flow'
          ? `${el.uid}:flow:${canonicalize(el.name)}:${el.points[0].attachedToUid}->${el.points[el.points.length - 1].attachedToUid}`
          : `${el.uid}:${el.type}`,
      )
      .sort();
  expect(shape(rendered)).toEqual(shape(engine));
}

async function expectConsistent(m: Mounted): Promise<Model> {
  const model = await m.engineModel();
  expectModelViewAgreement(model);
  expectRenderedMatchesEngine(m.view(), model.views[0]);
  return model;
}

function stockLists(model: Model, ident: string): { inflows: string[]; outflows: string[] } {
  const stock = model.variables.get(ident);
  if (stock?.type !== 'stock') {
    throw new Error(`no stock ${ident}`);
  }
  return { inflows: stock.inflows.map(canonicalize).sort(), outflows: stock.outflows.map(canonicalize).sort() };
}

// A gesture released at `current`: planned by the production planner on the
// rendered view with the Canvas's inputs, and its commit handed to the Editor
// exactly as the Canvas's pointer-up hands it.
function release(gesture: Gesture, press: { x: number; y: number }, current: { x: number; y: number }): void {
  const p = canvasProps!;
  const plan = planGesture({
    view: p.view,
    variables: p.model.variables,
    selection: p.selection,
    gesture,
    press,
    current,
    zoom: 1,
    pointerType: 'mouse',
    readOnly: !!p.readOnly,
    names: p.newVariableName!,
  });
  expect(plan.commit).toBe('edit');
  p.onCommitGesture({
    label: plan.label,
    elements: plan.elements,
    nextUid: plan.nextUid,
    selection: plan.selection,
    token: p.token,
    baseView: p.view,
    editName: plan.handoff?.editName,
  });
}

// Reattach `flowUid`'s sink onto the stock `targetUid`: its arrowhead dragged
// onto the stock's center.
function attachSink(m: Mounted, flowUid: number, targetUid: number): void {
  const flow = m.element(flowUid) as FlowViewElement;
  const target = m.element(targetUid);
  act(() => {
    release({ kind: 'flowEndpoint', flow: flowUid, end: 'sink' }, flow.points[flow.points.length - 1], target);
  });
}

// Drag element `uid`'s label 40px toward `side`.
function moveLabel(uid: number, side: 'top' | 'bottom' | 'left' | 'right'): void {
  const el = canvasProps!.view.elements.find((e) => e.uid === uid)!;
  const at = {
    top: { x: el.x, y: el.y - 40 },
    bottom: { x: el.x, y: el.y + 40 },
    left: { x: el.x - 40, y: el.y },
    right: { x: el.x + 40, y: el.y },
  }[side];
  release({ kind: 'label', uid }, at, at);
}

function select(uids: number[]): void {
  act(() => {
    canvasProps!.onSetSelection(new Set(uids));
  });
}

function pressDelete(): void {
  const root = document.querySelector('[data-simlin-editor-root]') as HTMLElement;
  act(() => {
    fireEvent.pointerDown(root);
    fireEvent.pointerUp(root);
    fireEvent.keyDown(root, { key: 'Delete' });
  });
}

function newAux(name: string, x: number, y: number): ViewElement {
  return {
    type: 'aux',
    uid: -2,
    name,
    ident: canonicalize(name),
    var: undefined,
    x,
    y,
    labelSide: 'right',
    isZeroRadius: false,
  };
}

// ---------------------------------------------------------------------------

describeWithEngine('Editor + real engine: edits racing in-flight patches', () => {
  let engine: EngineModule;

  beforeAll(async () => {
    engine = await loadEngine();
  });

  afterEach(async () => {
    cleanup();
    rs.restoreAllMocks();
    canvasProps = undefined;
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  it('T11: two attaches onto one stock while the first patch is in flight both land', async () => {
    const m = await mount(projectJson(baseModel()));
    select([5]);
    attachSink(m, 5, 1);
    select([6]);
    attachSink(m, 6, 1);
    await m.settle();
    const model = await expectConsistent(m);
    expect(stockLists(model, 'a').inflows).toEqual(['g', 'h', 'k']);
    // Only h's sink moved; its source stays on B.
    expect(stockLists(model, 'b').outflows).toEqual(['h']);
  });

  it('T11b: a create raced by a delete: the created variable is visible and the deleted flow is gone', async () => {
    const m = await mount(projectJson(baseModel()));
    let refusal: string | undefined | void;
    act(() => {
      refusal = canvasProps!.onCreateVariable(newAux('brand new', 600, 300));
    });
    expect(refusal).toBeUndefined();
    select([6]);
    pressDelete();
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('brand_new')).toBe(true);
    expect(model.variables.has('k')).toBe(false);
  });

  it('T11c: an attach raced by a label move: both land', async () => {
    const m = await mount(projectJson(baseModel()));
    select([5]);
    attachSink(m, 5, 1);
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect(stockLists(model, 'a').inflows).toEqual(['g', 'h']);
    expect((model.views[0].elements.find((el) => el.uid === 7) as { labelSide: string }).labelSide).toBe('top');
  });

  it('T11e: an attach raced by deleting another flow: the saved bytes hold the invariants', async () => {
    const m = await mount(projectJson(baseModel()));
    select([5]);
    attachSink(m, 5, 1);
    select([6]);
    pressDelete();
    await m.settle();
    const model = await expectConsistent(m);
    expect(stockLists(model, 'a').inflows).toEqual(['g', 'h']);
    expect(model.variables.has('k')).toBe(false);

    const bytes = await m.controller.query((e) => e.serializeProtobuf());
    const reopened = await engine.Project.openProtobuf(bytes!);
    try {
      const saved = mainModel(projectFromJson(JSON.parse(await reopened.serializeJson()) as JsonProject).models);
      expectModelViewAgreement(saved);
    } finally {
      await reopened.dispose();
    }
  });

  it('T24: undo is unavailable while an attach is in flight, and undoes it once the attach lands', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    expect((screen.getByLabelText('Undo') as HTMLButtonElement).disabled).toBe(false);

    select([5]);
    attachSink(m, 5, 1);
    expect((screen.getByLabelText('Undo') as HTMLButtonElement).disabled).toBe(true);
    act(() => {
      fireEvent.click(screen.getByLabelText('Undo'));
    });
    await m.settle();
    let model = await expectConsistent(m);
    expect(stockLists(model, 'a').inflows).toEqual(['g', 'h']);

    act(() => {
      fireEvent.click(screen.getByLabelText('Undo'));
    });
    await m.settle();
    model = await expectConsistent(m);
    expect(stockLists(model, 'a').inflows).toEqual(['g']);
    expect(stockLists(model, 'b').outflows).toEqual(['h']);
  });

  it('T12: attaching onto a stock that already lists the flow does not list it twice', async () => {
    const model = baseModel();
    model.stocks[0].inflows = ['g', 'k'];
    const m = await mount(projectJson(model));
    select([6]);
    attachSink(m, 6, 1);
    await m.settle();
    const after = await expectConsistent(m);
    expect(stockLists(after, 'a').inflows).toEqual(['g', 'k']);
  });

  it('T6: creating a variable under a taken name is refused and enqueues nothing', async () => {
    const m = await mount(projectJson(baseModel()));
    const patchesBefore = m.patches.length;
    let refusal: string | undefined | void;
    act(() => {
      refusal = canvasProps!.onCreateVariable(newAux('A', 600, 300));
    });
    expect(typeof refusal).toBe('string');
    await m.settle();
    expect(m.patches.length).toBe(patchesBefore);
    const model = await expectConsistent(m);
    expect(model.variables.get('a')?.type).toBe('stock');
  });

  it('T20: renaming a variable onto a taken name is refused and enqueues nothing', async () => {
    const m = await mount(projectJson(baseModel()));
    const patchesBefore = m.patches.length;
    select([7]);
    let refusal: string | undefined | void;
    act(() => {
      refusal = canvasProps!.onRenameVariable('x', 'A');
    });
    expect(typeof refusal).toBe('string');
    await m.settle();
    expect(m.patches.length).toBe(patchesBefore);
    const model = await expectConsistent(m);
    expect(model.variables.get('x')?.type).toBe('aux');
    expect(model.variables.get('a')?.type).toBe('stock');
  });

  it('two quick creates get distinct default names, and both variables exist', async () => {
    const m = await mount(projectJson(baseModel()));
    const first = canvasProps!.newVariableName!('New Variable');
    act(() => {
      canvasProps!.onCreateVariable(newAux(first, 600, 300));
    });
    const second = canvasProps!.newVariableName!('New Variable');
    expect(second).not.toBe(first);
    act(() => {
      canvasProps!.onCreateVariable(newAux(second, 700, 300));
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.get(canonicalize(first))?.type).toBe('aux');
    expect(model.variables.get(canonicalize(second))?.type).toBe('aux');
  });

  it('T23: deleting a stock removes its alias and every link touching either', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    pressDelete();
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('a')).toBe(false);
    expect(model.views[0].elements.some((el) => el.type === 'alias')).toBe(false);
  });

  it('T1: deleting a lone cloud whose flow survives changes nothing', async () => {
    const m = await mount(projectJson(baseModel()));
    const before = await m.engineModel();
    select([21]);
    pressDelete();
    await m.settle();
    const after = await expectConsistent(m);
    expect(after.views[0].elements.map((el) => el.uid).sort()).toEqual(
      before.views[0].elements.map((el) => el.uid).sort(),
    );
  });

  it('T22: an attach whose geometry is non-finite is refused whole: no stock ops, no view', async () => {
    const m = await mount(projectJson(baseModel()));
    const patchesBefore = m.patches.length;
    select([4]);
    // The planner never produces a non-finite coordinate (its fuzz asserts
    // finiteness); this hand-builds one to exercise the executor's refusal, the
    // defense in depth behind it (#818): f's sink detached to a NaN cloud.
    const view = canvasProps!.view;
    const f = m.element(4) as FlowViewElement;
    const detached: FlowViewElement = {
      ...f,
      points: [f.points[0], { x: NaN, y: 100, attachedToUid: view.nextUid }],
    };
    act(() => {
      canvasProps!.onCommitGesture({
        label: 'flow attach',
        elements: [
          ...view.elements.map((el) => (el.uid === 4 ? detached : el)),
          { type: 'cloud', uid: view.nextUid, flowUid: 4, x: NaN, y: 100, isZeroRadius: false, ident: undefined },
        ],
        nextUid: view.nextUid + 1,
        selection: new Set([4]),
        token: canvasProps!.token,
        baseView: view,
      });
    });
    await m.settle();
    expect(m.patches.length).toBe(patchesBefore);
    const model = await expectConsistent(m);
    expect(stockLists(model, 'b').inflows).toEqual(['f']);
  });

  it('typing in a details panel then dragging a stock: the draft lands first and the text survives', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    const unitsEl = document.querySelector('.unitsEditor') as HTMLElement;
    expect(unitsEl).not.toBeNull();
    const editor = ELEMENT_TO_NODE.get(unitsEl) as unknown as SlateEditor;
    await act(async () => {
      Transforms.insertText(editor, 'widgets', { at: SlateEditor.end(editor, []) });
      editor.onChange();
      await Promise.resolve();
    });

    // The press that starts the drag (the Canvas prevents its default, so the
    // panel never blurs), then the drag's commit.
    const canvas = screen.getByTestId('canvas');
    act(() => {
      fireEvent.pointerDown(canvas);
    });
    act(() => {
      release({ kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 150, y: 140 });
    });
    act(() => {
      fireEvent.pointerUp(canvas);
    });
    await m.settle();

    const model = await expectConsistent(m);
    expect(model.variables.get('a')?.units).toBe('widgets');
    const stock = model.views[0].elements.find((el) => el.uid === 1)!;
    expect({ x: stock.x, y: stock.y }).toEqual({ x: 150, y: 140 });
    // The draft's edit reached the engine before the move.
    const opTypes = m.patches.map((p) => p.models![0].ops[0].type);
    expect(opTypes.indexOf('upsertStock')).toBeLessThan(opTypes.lastIndexOf('upsertView'));
    expect(document.querySelector('.unitsEditor')?.textContent).toBe('widgets');
  });

  it('R1: renaming an element again while its first rename is pending takes the second name', async () => {
    const m = await mount(projectJson(baseModel()));
    select([7]);
    let first: string | undefined | void;
    let second: string | undefined | void;
    act(() => {
      first = canvasProps!.onRenameVariable('x', 'Y');
    });
    expect(first).toBeUndefined();
    // The Canvas commits the next rename with the name the element renders.
    const renderedName = nameOf(m.element(7))!;
    expect(renderedName).toBe('Y');
    act(() => {
      second = canvasProps!.onRenameVariable(renderedName, 'Z');
    });
    expect(second).toBeUndefined();
    expect(nameOf(m.element(7))).toBe('Z');
    await m.settle();
    const model = await expectConsistent(m);
    expect([...model.variables.keys()].filter((k) => ['x', 'y', 'z'].includes(k))).toEqual(['z']);
    expect(model.variables.get('f')?.type === 'flow' && model.variables.get('f')!.equation).toMatchObject({
      equation: 'z',
    });
    expect(alerts()).toEqual([]);
  });

  it('R3: renaming a just-created element while its create is pending takes the new name', async () => {
    const m = await mount(projectJson(baseModel()));
    // As the Canvas commits a create: staged under the default name's ident,
    // with the typed name.
    act(() => {
      canvasProps!.onCreateVariable({ ...newAux('Births', 600, 300), ident: canonicalize('New Variable') });
    });
    const created = m.view().elements.find((el) => nameOf(el) === 'Births')!;
    // Every rendered element's ident is its name's, a pending create's too.
    expect(created).toMatchObject({ ident: 'births' });
    select([created.uid]);
    act(() => {
      canvasProps!.onRenameVariable('Births', 'Deaths');
    });
    expect(m.element(created.uid)).toMatchObject({ name: 'Deaths', ident: 'deaths' });
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('deaths')).toBe(true);
    expect(model.variables.has('births')).toBe(false);
    expect(alerts()).toEqual([]);
  });

  it('R4: typing in the panel of a variable whose rename is pending keeps the text, and a later edit survives', async () => {
    const m = await mount(projectJson(baseModel()));
    select([7]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    act(() => {
      canvasProps!.onRenameVariable('x', 'Y');
    });
    // The panel stays open on the renamed element while the rename is pending.
    await typeUnits('widgets');
    // A canvas press flushes the draft; then an unrelated diagram edit.
    const canvas = screen.getByTestId('canvas');
    act(() => {
      fireEvent.pointerDown(canvas);
      fireEvent.pointerUp(canvas);
    });
    act(() => {
      moveLabel(1, 'top');
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect({ alerts: alerts(), units: model.variables.get('y')?.units, labelSide: labelSideOf(model, 1) }).toEqual({
      alerts: [],
      units: 'widgets',
      labelSide: 'top',
    });
  });

  it('a draft typed while a rename is pending lands on the variable even when the rename rolls back', async () => {
    const m = await mount(projectJson(baseModel()));
    select([7]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    m.armPatchFailure();
    act(() => {
      canvasProps!.onRenameVariable('x', 'Y');
    });
    await typeUnits('widgets');
    // Flushed under the pending name; the edit resolves its variable through
    // the element on the committed view when it runs.
    const canvas = screen.getByTestId('canvas');
    act(() => {
      fireEvent.pointerDown(canvas);
      fireEvent.pointerUp(canvas);
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('y')).toBe(false);
    expect(model.variables.get('x')?.units).toBe('widgets');
    expect(nameOf(m.element(7))).toBe('x');
    expect(alerts()).toEqual(['injected patch failure']);
  });

  it('R9/N4: a patch whose read-back fails once lands on the re-read: an edit planned on it lands too, and nothing is reported', async () => {
    const m = await mount(projectJson(baseModel()));
    m.armReadFailure();
    act(() => {
      canvasProps!.onCreateVariable(newAux('brand new', 600, 300));
    });
    act(() => {
      moveLabel(1, 'top');
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('brand_new')).toBe(true);
    expect(labelSideOf(model, 1)).toBe('top');
    expect(alerts()).toEqual([]);
  });

  it('N1: an edit made while a failed read-back reopens the project is discarded with the failed edit', async () => {
    const m = await mount(projectJson(baseModel()));
    let openGate!: () => void;
    const gate = new Promise<void>((resolve) => {
      openGate = resolve;
    });
    let reopenStarted = false;
    const openProtobuf = EngineProject.openProtobuf.bind(EngineProject);
    rs.spyOn(EngineProject, 'openProtobuf').mockImplementation(async (...args) => {
      reopenStarted = true;
      await gate;
      return openProtobuf(...args);
    });
    // The create's read-back fails, and so does the re-read.
    m.armReadFailure(2);
    act(() => {
      canvasProps!.onCreateVariable(newAux('brand new', 600, 300));
    });
    await waitFor(() => reopenStarted);
    // While the new engine opens the failed create still renders, and a label
    // move is planned on it.
    expect(m.view().elements.some((el) => nameOf(el) === 'brand new')).toBe(true);
    act(() => {
      moveLabel(1, 'top');
    });
    openGate();
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('brand_new')).toBe(false);
    expect(labelSideOf(model, 1)).not.toBe('top');
    expect(alerts()).toEqual([
      'reading the project back after variable creation failed: injected read failure (1 later edit discarded)',
    ]);
  });

  it('N10: an engine that cannot be reopened shows one persistent notice offering a reload, and edits are refused without a toast each', async () => {
    const onReload = rs.fn();
    const m = await mount(projectJson(baseModel()), { onReload });
    // History to undo, so undo refusal below is the unavailable state's doing.
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    expect((screen.getByLabelText('Undo') as HTMLButtonElement).disabled).toBe(false);
    rs.spyOn(EngineProject, 'openProtobuf').mockImplementation(async () => {
      throw new Error('reopen failed');
    });
    m.armReadFailure(2);
    act(() => {
      canvasProps!.onCreateVariable(newAux('brand new', 600, 300));
    });
    await m.settle();
    expect(screen.getAllByText('The model engine stopped working')).toHaveLength(1);

    let refusal: string | undefined | void;
    act(() => {
      refusal = canvasProps!.onCreateVariable(newAux('another', 600, 400));
      moveLabel(1, 'top');
    });
    await m.settle();
    expect(refusal).toBe('The project cannot be edited until it is reloaded');
    expect(alerts()).toEqual([]);
    expect(screen.getAllByText('The model engine stopped working')).toHaveLength(1);
    act(() => {
      fireEvent.click(screen.getByText('Reload'));
    });
    expect(onReload).toHaveBeenCalledTimes(1);
    // Undo and redo are unavailable too.
    expect((screen.getByLabelText('Undo') as HTMLButtonElement).disabled).toBe(true);
    m.controller.undoRedo('undo');
    expect(m.controller.getSnapshot().undoRedoQueued).toBe(false);
  });

  it('R10/N2: while an undo is queued, Delete does nothing and reports nothing, and a typed name is refused visibly', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    select([6]);
    const root = document.querySelector('[data-simlin-editor-root]') as HTMLElement;
    let createRefusal: string | undefined | void;
    let renameRefusal: string | undefined | void;
    act(() => {
      fireEvent.keyDown(root, { key: 'z', ctrlKey: true });
      fireEvent.keyDown(root, { key: 'Delete' });
      // A name commit returning a message keeps the Canvas's name editor open.
      createRefusal = canvasProps!.onCreateVariable(newAux('Births', 600, 300));
      renameRefusal = canvasProps!.onRenameVariable('x', 'Renamed');
    });
    expect([createRefusal, renameRefusal]).toEqual([
      'Wait for the undo or redo to finish',
      'Wait for the undo or redo to finish',
    ]);
    // The selection was not cleared for a delete that could not happen.
    expect(canvasProps!.selection).toEqual(new Set([6]));
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('k')).toBe(true);
    expect(model.variables.has('births')).toBe(false);
    expect(model.variables.has('renamed')).toBe(false);
    expect(labelSideOf(model, 7)).not.toBe('top');
    expect(alerts()).toEqual([]);
  });

  it('two edits made in one tick both land: each handler plans on the view the previous one produced', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      canvasProps!.onCreateVariable(newAux('brand new', 600, 300));
      pressDelete();
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect(model.variables.has('brand_new')).toBe(true);
  });

  it('a gesture planned on a view another edit replaced in the same tick is dropped quietly (E5), the first landing', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(1, 'top');
      // Planned on the same rendered view the first gesture planned on, which
      // the first edit has replaced: committing it would revert that edit.
      moveLabel(2, 'bottom');
    });
    await m.settle();
    const model = await expectConsistent(m);
    expect(labelSideOf(model, 1)).toBe('top');
    expect(labelSideOf(model, 2)).not.toBe('bottom');
    expect(alerts()).toEqual([]);
  });

  it('Undo pressed with a draft in the details panel takes the draft back; Redo restores it', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');

    // The press on Undo, as a browser delivers it: React commits what the
    // pointerdown changed before the click is dispatched, so a draft flushed on
    // the press would disable the button before its click.
    const undo = screen.getByLabelText('Undo') as HTMLButtonElement;
    expect(undo.disabled).toBe(false);
    act(() => {
      fireEvent.pointerDown(undo);
      fireEvent.mouseDown(undo);
    });
    act(() => {
      fireEvent.pointerUp(undo);
      fireEvent.mouseUp(undo);
      fireEvent.click(undo);
    });
    await m.settle();
    let model = await expectConsistent(m);
    expect(model.variables.get('a')?.units).toBe('');
    // Only the draft was taken back.
    expect(labelSideOf(model, 7)).toBe('top');
    expect(
      SlateEditor.string(
        ELEMENT_TO_NODE.get(document.querySelector('.unitsEditor') as HTMLElement) as unknown as SlateEditor,
        [],
      ),
    ).toBe('');

    act(() => {
      fireEvent.click(screen.getByLabelText('Redo'));
    });
    await m.settle();
    model = await expectConsistent(m);
    expect(model.variables.get('a')?.units).toBe('widgets');
    expect(alerts()).toEqual([]);
  });

  it('N5: text typed after a flush survives the flushed edit landing, and lands once submitted', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    pressCanvas();
    // The flushed edit is in flight; the user keeps typing.
    await typeUnits(' more');
    await m.settle();
    let model = await m.engineModel();
    expect({ units: model.variables.get('a')?.units, panel: unitsText() }).toEqual({
      units: 'widgets',
      panel: 'widgets more',
    });
    pressCanvas();
    await m.settle();
    model = await expectConsistent(m);
    expect({ units: model.variables.get('a')?.units, panel: unitsText() }).toEqual({
      units: 'widgets more',
      panel: 'widgets more',
    });
  });

  it('a draft in the panel of a variable whose equation a rename rewrites survives the rename, and submitting it keeps the rewrite', async () => {
    const m = await mount(projectJson(baseModel()));
    select([4]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    // f's equation reads x, so renaming x rewrites it when the rename lands.
    act(() => {
      canvasProps!.onRenameVariable('x', 'Y');
    });
    await m.settle();
    expect(unitsText()).toBe('widgets');
    pressCanvas();
    await m.settle();
    const model = await expectConsistent(m);
    const f = model.variables.get('f');
    expect({ units: f?.units, equation: f?.type === 'flow' ? f.equation : undefined, alerts: alerts() }).toEqual({
      units: 'widgets',
      equation: expect.objectContaining({ equation: 'y' }),
      alerts: [],
    });
  });

  it('an unrelated edit landing while the user types keeps the panel and its draft', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    const editor = await typeUnits('widgets');
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    expect(labelSideOf(await m.engineModel(), 7)).toBe('top');
    expect(ELEMENT_TO_NODE.get(document.querySelector('.unitsEditor') as HTMLElement)).toBe(editor);
    expect(unitsText()).toBe('widgets');
  });

  it('N7: Undo pressed with a draft whose edit fails keeps the draft and undoes nothing', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    m.armPatchFailure();
    pressButton('Undo');
    await m.settle();
    const model = await expectConsistent(m);
    expect({
      label7: labelSideOf(model, 7),
      units: model.variables.get('a')?.units,
      panel: unitsText(),
      alerts: alerts(),
    }).toEqual({ label7: 'top', units: '', panel: 'widgets', alerts: ['injected patch failure'] });
  });

  it('N8: Redo pressed with a draft in the panel redoes, then lands the draft on the redone project', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    act(() => {
      fireEvent.click(screen.getByLabelText('Undo'));
    });
    await m.settle();
    expect(labelSideOf(await m.engineModel(), 7)).not.toBe('top');
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    pressButton('Redo');
    await m.settle();
    const model = await expectConsistent(m);
    expect({
      label7: labelSideOf(model, 7),
      units: model.variables.get('a')?.units,
      panel: unitsText(),
      alerts: alerts(),
    }).toEqual({ label7: 'top', units: 'widgets', panel: 'widgets', alerts: [] });
  });

  it('O3: two names swapped through three pending renames resolve every rendered element mid-flight and land as a swap', async () => {
    const m = await mount(projectJson(baseModel()));
    const refusals: Array<string | undefined | void> = [];
    act(() => {
      refusals.push(canvasProps!.onRenameVariable('x', 'tmp'));
    });
    act(() => {
      refusals.push(canvasProps!.onRenameVariable('A', 'x'));
    });
    act(() => {
      refusals.push(canvasProps!.onRenameVariable('tmp', 'A'));
    });
    const rendered = m.controller.getModel()!;
    const unresolved = m
      .view()
      .elements.filter((el) => isNamedViewElement(el) && !rendered.variables.has(el.ident))
      .map((el) => el.uid);
    expect({
      refusals,
      unresolved,
      a: rendered.variables.get('a')?.type,
      x: rendered.variables.get('x')?.type,
    }).toEqual({ refusals: [undefined, undefined, undefined], unresolved: [], a: 'aux', x: 'stock' });
    await m.settle();
    const model = await expectConsistent(m);
    expect({ a: model.variables.get('a')?.type, x: model.variables.get('x')?.type, alerts: alerts() }).toEqual({
      a: 'aux',
      x: 'stock',
      alerts: [],
    });
  });

  it('N6: a draft whose edit is lost to a reopen stays in the panel, to submit again', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    // The flushed edit applies, and neither its read-back nor the re-read works,
    // so the last recorded snapshot is reopened without it.
    m.armReadFailure(2);
    pressCanvas();
    await m.settle();
    const model = await expectConsistent(m);
    expect({ units: model.variables.get('a')?.units, panel: unitsText(), alerts: alerts() }).toEqual({
      units: '',
      panel: 'widgets',
      alerts: ['reading the project back after equation update failed: injected read failure'],
    });
    // It is still a draft: the next press submits it again, and it lands.
    pressCanvas();
    await m.settle();
    expect((await m.engineModel()).variables.get('a')?.units).toBe('widgets');
  });

  it('F1/P1: a field changed back while its flushed edit is in flight holds its text, and the next press lands it', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    pressCanvas();
    // The flushed edit is in flight; the user changes their mind.
    await setField('.unitsEditor', '');
    await m.settle();
    // The edit landed underneath, without remounting the panel over the cleared field.
    expect((await m.engineModel()).variables.get('a')?.units).toBe('widgets');
    expect(fieldText('.unitsEditor')).toBe('');
    pressCanvas();
    await m.settle();
    const model = await expectConsistent(m);
    expect({ units: model.variables.get('a')?.units, panel: fieldText('.unitsEditor'), alerts: alerts() }).toEqual({
      units: '',
      panel: '',
      alerts: [],
    });
  });

  it('F1/P1b: with another field holding the panel, a field changed back after its edit landed is submitted', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    pressCanvas();
    // While the units edit is in flight, the user writes documentation.
    await setField('.notesEditor', 'hello');
    await m.settle();
    expect((await m.engineModel()).variables.get('a')?.units).toBe('widgets');
    // Documentation holds the panel; units goes back to what it was.
    await setField('.unitsEditor', '');
    pressCanvas();
    await m.settle();
    const model = await expectConsistent(m);
    expect({
      units: model.variables.get('a')?.units,
      docs: model.variables.get('a')?.documentation,
      panel: fieldText('.unitsEditor'),
      alerts: alerts(),
    }).toEqual({ units: '', docs: 'hello', panel: '', alerts: [] });
  });

  it("HK4: selecting another element while the panel holds a draft opens that element's own panel", async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    pressCanvas();
    // A draft past the submitted text holds the panel's key.
    await typeUnits(' more');
    select([2]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    expect(unitsText()).toBe('');
    pressCanvas();
    await m.settle();
    const model = await expectConsistent(m);
    expect({ a: model.variables.get('a')?.units, b: model.variables.get('b')?.units }).toEqual({ a: 'widgets', b: '' });
  });

  it('M1: while an undo is queued the details panel is read-only, and it is editable again once the undo lands', async () => {
    const m = await mount(projectJson(baseModel()));
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    let openGate!: () => void;
    const gate = new Promise<void>((resolve) => {
      openGate = resolve;
    });
    const openProtobuf = EngineProject.openProtobuf.bind(EngineProject);
    rs.spyOn(EngineProject, 'openProtobuf').mockImplementation(async (...args) => {
      await gate;
      return openProtobuf(...args);
    });
    pressButton('Undo');
    expect(m.controller.getSnapshot().undoRedoQueued).toBe(true);
    expect(document.querySelector('.unitsEditor')!.getAttribute('contenteditable')).toBe('false');
    openGate();
    await m.settle();
    expect(labelSideOf(await m.engineModel(), 7)).not.toBe('top');
    expect(document.querySelector('.unitsEditor')!.getAttribute('contenteditable')).toBe('true');
  });

  it('F1/P1 docs: a documentation field changed back while its flushed edit is in flight holds its text, and the next press lands it', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await setField('.notesEditor', 'hello');
    pressCanvas();
    await setField('.notesEditor', '');
    await m.settle();
    expect((await m.engineModel()).variables.get('a')?.documentation).toBe('hello');
    expect(fieldText('.notesEditor')).toBe('');
    pressCanvas();
    await m.settle();
    const model = await expectConsistent(m);
    expect({ docs: model.variables.get('a')?.documentation, panel: fieldText('.notesEditor') }).toEqual({
      docs: '',
      panel: '',
    });
  });

  it('P14: a panel reopened while its own submission is in flight shows the submitted text, not the text it replaces', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    pressCanvas();
    select([2]);
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    const reopened = unitsText();
    await m.settle();
    const model = await expectConsistent(m);
    expect({ reopened, afterLanding: unitsText(), units: model.variables.get('a')?.units, alerts: alerts() }).toEqual({
      reopened: 'widgets',
      afterLanding: 'widgets',
      units: 'widgets',
      alerts: [],
    });
    // The reopened panel took the submission as its base: the text is not a
    // draft, so the next press submits nothing.
    const patches = m.patches.length;
    pressCanvas();
    await m.settle();
    expect(m.patches).toHaveLength(patches);
  });

  it('P14 failure: a panel reopened on a pending submission that then fails keeps the text as a draft to submit again', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    await typeUnits('widgets');
    m.armPatchFailure();
    pressCanvas();
    select([2]);
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    expect(unitsText()).toBe('widgets');
    await m.settle();
    expect({
      units: (await m.engineModel()).variables.get('a')?.units,
      panel: unitsText(),
      alerts: alerts(),
    }).toEqual({ units: '', panel: 'widgets', alerts: ['injected patch failure'] });
    pressCanvas();
    await m.settle();
    expect((await expectConsistent(m)).variables.get('a')?.units).toBe('widgets');
  });

  it("M4: an element with no stored label side renders centered, and a pan's persist or an edit's view keeps it absent", async () => {
    const m = await mount(projectJson(baseModel()));
    // x (uid 7) has no labelSide in the fixture.
    expect((m.element(7) as { labelSide: string }).labelSide).toBe('center');
    const rawElement = async (uid: number) => {
      const raw = JSON.parse((await m.controller.query((engine) => engine.serializeJson()))!) as JsonProject;
      const main = raw.models.find((model) => model.name === 'main')!;
      const view = main.views![0] as unknown as { zoom?: number; elements: Array<{ uid: number; labelSide?: string }> };
      return { zoom: view.zoom, element: view.elements.find((el) => el.uid === uid)! };
    };
    expect((await rawElement(7)).element.labelSide).toBeUndefined();
    act(() => {
      canvasProps!.onViewBoxChange({ x: 40, y: 40, width: 800, height: 600 }, 1.5);
    });
    await m.settle();
    let persisted = await rawElement(7);
    expect({ zoom: persisted.zoom, labelSide: persisted.element.labelSide }).toEqual({
      zoom: 1.5,
      labelSide: undefined,
    });
    act(() => {
      moveLabel(1, 'top');
    });
    await m.settle();
    persisted = await rawElement(7);
    expect(persisted.element.labelSide).toBeUndefined();
    expect((await rawElement(1)).element.labelSide).toBe('top');
  });

  it('ED4: a drawn flow keeps the details panel closed until it is named, landed or not', async () => {
    const m = await mount(projectJson(baseModel()));
    select([1]);
    act(() => {
      canvasProps!.onShowVariableDetails();
    });
    expect(document.querySelector('.unitsEditor')).not.toBeNull();
    act(() => {
      release({ kind: 'createFlow', from: 'empty' }, { x: 600, y: 500 }, { x: 720, y: 500 });
    });
    expect(canvasProps!.selection.size).toBe(1);
    expect(document.querySelector('.unitsEditor')).toBeNull();
    await m.settle();
    expect((await m.engineModel()).variables.get('new_flow')?.type).toBe('flow');
    expect(document.querySelector('.unitsEditor')).toBeNull();
  });

  it('ED2: a commit carries its press token, so once another edit moves the token it is refused', async () => {
    const m = await mount(projectJson(baseModel()));
    const pressToken = canvasProps!.token;
    const pressView = canvasProps!.view;
    // A failing edit rolls back, moving the token; the rendered geometry returns
    // to what the press saw, so only the token tells the commit is stale.
    m.armPatchFailure();
    act(() => {
      moveLabel(7, 'top');
    });
    await m.settle();
    expect(canvasProps!.token).not.toBe(pressToken);
    expect(sameGeometry(pressView, canvasProps!.view)).toBe(true);

    const p = canvasProps!;
    const plan = planGesture({
      view: pressView,
      variables: p.model.variables,
      selection: new Set([1]),
      gesture: { kind: 'label', uid: 1 },
      press: { x: 100, y: 60 },
      current: { x: 100, y: 60 },
      zoom: 1,
      pointerType: 'mouse',
      readOnly: false,
      names: p.newVariableName!,
    });
    expect(plan.commit).toBe('edit');
    act(() => {
      p.onCommitGesture({
        label: plan.label,
        elements: plan.elements,
        nextUid: plan.nextUid,
        selection: plan.selection,
        token: pressToken,
        baseView: p.view,
      });
    });
    await m.settle();
    expect(labelSideOf(await m.engineModel(), 1)).not.toBe('top');
  });
});
