// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Details-panel edits against the REAL WASM engine, one row per kind of
// variable the panels open on: stock, flow and aux, each as a scalar, an
// apply-to-all variable, a per-element variable and a per-element variable with
// an EXCEPT default; the scalars that carry something beside their equation
// (ACTIVE INITIAL, a conveyor, a queue, a lookup); and a module. The fixture
// opens through the engine, so every row's starting JSON is what the engine
// itself serializes.
//
// For each row the panel edits units, then notes, then the equation, as a user
// does (type in the field, press the canvas), and after each edit the engine's
// serialized project must differ from before in that one field alone, through a
// patch naming only that field. Undo then takes each edit back.
//
// The lookup tab's edits are tabled the same way: each control of the lookup
// editor over a table with x points of its own and one without, a table added to
// and removed from each shape of equation.
//
// The Editor mounts with the Canvas mocked and the panels real. What this does
// not establish: what the panel shows for each kind
// (variable-details-arrayed.test.tsx), or edits racing one another
// (editor-engine-races.test.ts).

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
  // jsdom lays nothing out, and the lookup's chart draws its points only once it
  // has a width.
  Object.defineProperty(globalThis, 'ResizeObserver', {
    configurable: true,
    value: class {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element): void {
        const entry = { target, contentRect: { width: 400, height: 300 } } as ResizeObserverEntry;
        this.callback([entry], this as unknown as ResizeObserver);
      }
      unobserve(): void {}
      disconnect(): void {}
    },
  });
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
});

import * as React from 'react';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Editor as SlateEditor, Transforms } from 'slate';
import { ELEMENT_TO_NODE } from 'slate-dom';

import * as react from 'react' with { rstest: 'importActual' };

import type { JsonModelOperation, JsonProject, JsonProjectPatch } from '@simlin/engine';

import { ProjectController } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';
import { describeWithEngine, loadEngine } from './support/engine';

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
// Fixture: one variable per row of ROWS below.

type JsonVar = Record<string, unknown> & { name: string };

const table = { yPoints: [0, 5, 9], xScale: { min: 0, max: 2 }, yScale: { min: 0, max: 10 } };
const described = { units: 'widgets', documentation: 'before' };

const applyToAll = (equation: string) => ({ arrayedEquation: { dimensions: ['Region'], equation } });
const perElement = {
  arrayedEquation: {
    dimensions: ['Region'],
    elements: [
      { subscript: 'north', equation: '1', compat: { activeInitial: '4' } },
      { subscript: 'south', equation: '2' },
    ],
  },
};
const exceptDefault = {
  arrayedEquation: {
    dimensions: ['Region'],
    equation: '7',
    hasExceptDefault: true,
    elements: [{ subscript: 'north', equation: '1' }],
  },
};

const stocks: JsonVar[] = [
  {
    name: 'stock scalar',
    initialEquation: '10',
    inflows: ['flow_scalar'],
    outflows: [],
    compat: { nonNegative: true },
    ...described,
  },
  { name: 'stock a2a', inflows: ['flow_a2a'], outflows: [], compat: { nonNegative: true }, ...applyToAll('10') },
  { name: 'stock per element', inflows: ['flow_per_element'], outflows: [], ...perElement },
  { name: 'stock except', inflows: ['flow_except'], outflows: [], ...exceptDefault },
  {
    name: 'belt',
    initialEquation: '3',
    inflows: ['onto'],
    outflows: ['off'],
    compat: { conveyor: { transitTime: '4', capacity: '50', discrete: true } },
  },
  { name: 'line', initialEquation: '3', inflows: ['joining'], outflows: ['leaving'], compat: { queue: {} } },
];
const flows: JsonVar[] = [
  { name: 'flow scalar', equation: '1', compat: { nonNegative: true, leakage: { fraction: '0.1' } }, ...described },
  { name: 'flow a2a', compat: { nonNegative: true }, ...applyToAll('price * 2') },
  { name: 'flow per element', ...perElement },
  { name: 'flow except', ...exceptDefault },
  { name: 'onto', equation: '1' },
  { name: 'off', equation: '0' },
  { name: 'joining', equation: '1' },
  { name: 'leaving', equation: '1', compat: { overflow: true } },
];
const auxiliaries: JsonVar[] = [
  { name: 'aux scalar', equation: '1', compat: { canBeModuleInput: true, isPublic: true }, ...described },
  { name: 'price', compat: { activeInitial: '8' }, ...applyToAll('10'), ...described },
  { name: 'aux two dims', arrayedEquation: { dimensions: ['Region', 'Product'], equation: '1' } },
  { name: 'aux per element', ...perElement },
  { name: 'aux except', ...exceptDefault },
  {
    name: 'element lookups',
    arrayedEquation: {
      dimensions: ['Region'],
      elements: [
        { subscript: 'north', equation: 'TIME', graphicalFunction: table },
        { subscript: 'south', equation: '2' },
      ],
    },
  },
  { name: 'active initial', equation: 'price[north]', compat: { activeInitial: '6' } },
  { name: 'lookup only', equation: '', graphicalFunction: table },
  { name: 'with lookup', equation: 'TIME', graphicalFunction: table },
  {
    name: 'uneven lookup',
    equation: 'TIME',
    graphicalFunction: {
      points: [
        [0, 0],
        [2, 4],
        [10, 8],
      ],
      kind: 'discrete',
      xScale: { min: 0, max: 10 },
      yScale: { min: 0, max: 10 },
    },
  },
  { name: 'a2a with lookup', graphicalFunction: table, ...applyToAll('TIME') },
  {
    name: 'data',
    equation: '0',
    compat: { dataSource: { kind: 'data', file: 'a.xlsx', tabOrDelimiter: 'A', rowOrCol: '1', cell: 'B2' } },
  },
];
const modules: JsonVar[] = [
  {
    name: 'mod',
    modelName: 'child',
    references: [{ src: 'aux_scalar', dst: 'mod·input' }],
    compat: { isPublic: true },
    ...described,
  },
];

const all = [...stocks, ...flows, ...auxiliaries, ...modules];
const uidOf = (name: string): number => 1 + all.findIndex((v) => v.name === name);

function viewElements(): unknown[] {
  return all.map((v, i) => {
    const at = { uid: i + 1, name: v.name, x: 100 + 60 * i, y: 100 };
    if (stocks.includes(v)) {
      return { type: 'stock', ...at };
    } else if (flows.includes(v)) {
      // Drawn loose: a panel edit reads no geometry.
      return {
        type: 'flow',
        ...at,
        points: [
          { x: at.x - 20, y: 100 },
          { x: at.x + 20, y: 100 },
        ],
      };
    } else if (modules.includes(v)) {
      return { type: 'module', ...at };
    }
    return { type: 'aux', ...at };
  });
}

function projectJson(): string {
  return JSON.stringify({
    name: 'panel edits',
    simSpecs: { startTime: 0, endTime: 3, dt: '1' },
    dimensions: [
      { name: 'Region', elements: ['North', 'South'] },
      { name: 'Product', elements: ['Hat', 'Scarf'] },
    ],
    models: [
      { name: 'main', stocks, flows, auxiliaries, modules, views: [{ elements: viewElements() }] },
      {
        name: 'child',
        stocks: [],
        flows: [],
        auxiliaries: [{ name: 'input', equation: '0', compat: { canBeModuleInput: true } }],
        views: [{ elements: [{ type: 'aux', uid: 1, name: 'input', x: 0, y: 0 }] }],
      },
    ],
  });
}

// ---------------------------------------------------------------------------
// Mounting

interface Mounted {
  readonly patches: JsonProjectPatch[];
  settle(): Promise<void>;
  // Every variable of 'main' as the engine serializes it, by name.
  engineVariables(): Promise<Map<string, JsonVar>>;
}

async function mount(): Promise<Mounted> {
  const controllers: ProjectController[] = [];
  const patches: JsonProjectPatch[] = [];
  const originalOpen = ProjectController.prototype.openInitialProject;
  rs.spyOn(ProjectController.prototype, 'openInitialProject').mockImplementation(function (this: ProjectController) {
    controllers.push(this);
    return originalOpen.call(this);
  });
  rs.spyOn(console, 'error').mockImplementation(() => {});
  act(() => {
    render(
      React.createElement(Editor, {
        inputFormat: 'json',
        initialProjectJson: projectJson(),
        initialProjectVersion: 1,
        name: 'panel edits',
        onSave: async () => 1,
      } as EditorProps),
    );
  });
  const controller = controllers[0];
  const settle = async (): Promise<void> => {
    for (let i = 0; i < 3; i++) {
      await act(async () => {
        await controller.whenIdle();
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
  };
  await settle();
  await act(async () => {
    await controller.query(async (engine) => {
      const apply = engine.applyPatch.bind(engine);
      engine.applyPatch = async (patch, options) => {
        patches.push(patch);
        return apply(patch, options);
      };
    });
  });
  return {
    patches,
    settle,
    engineVariables: async () => {
      const serialized = await controller.query((engine) => engine.serializeJson());
      const main = (JSON.parse(serialized!) as JsonProject).models.find((m) => m.name === 'main')!;
      return new Map(
        [...main.stocks, ...main.flows, ...main.auxiliaries, ...(main.modules ?? [])].map((v) => [
          v.name,
          v as unknown as JsonVar,
        ]),
      );
    },
  };
}

function alerts(): string[] {
  return Array.from(document.querySelectorAll('[id="client-snackbar"]')).map((el) => el.textContent ?? '');
}

function openPanel(name: string): void {
  act(() => {
    canvasProps!.onSetSelection(new Set([uidOf(name)]));
  });
  act(() => {
    canvasProps!.onShowVariableDetails();
  });
  expect(document.querySelector('.unitsEditor')).not.toBeNull();
}

// The equation shows as a preview that a click turns into the raw editor, unless
// the variable has an equation error, which opens the raw editor by itself.
function openEquationEditor(): void {
  const preview = document.querySelector('.eqnPreview');
  if (preview !== null) {
    act(() => {
      fireEvent.click(preview);
    });
  }
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

// A canvas press and release: the press submits an open panel's draft.
function pressCanvas(): void {
  const canvas = screen.getByTestId('canvas');
  act(() => {
    fireEvent.pointerDown(canvas);
    fireEvent.pointerUp(canvas);
  });
}

function pressButton(label: string): void {
  const button = screen.getByLabelText(label) as HTMLButtonElement;
  expect(button.disabled).toBe(false);
  act(() => {
    fireEvent.pointerDown(button);
    fireEvent.pointerUp(button);
    fireEvent.click(button);
  });
}

function lastOps(m: Mounted): JsonModelOperation[] {
  const patch = m.patches[m.patches.length - 1];
  expect(patch.projectOps ?? []).toEqual([]);
  expect(patch.models!.map((model) => model.name)).toEqual(['main']);
  return patch.models![0].ops;
}

function without(v: JsonVar, ...keys: string[]): JsonVar {
  return Object.fromEntries(Object.entries(v).filter(([key]) => !keys.includes(key))) as JsonVar;
}

// `before` with one variable replaced, as the engine should serialize it.
function withVariable(before: Map<string, JsonVar>, name: string, change: Record<string, unknown>) {
  return new Map([...before].map(([n, v]) => [n, n === name ? { ...v, ...change } : v]));
}

// ---------------------------------------------------------------------------

// How the engine's JSON spells a variable with a new equation, by its shape.
type EquationShape = 'equation' | 'initialEquation' | 'applyToAll' | 'perElement' | 'module';

const ROWS: ReadonlyArray<{ name: string; shape: EquationShape }> = [
  { name: 'stock scalar', shape: 'initialEquation' },
  { name: 'stock a2a', shape: 'applyToAll' },
  { name: 'stock per element', shape: 'perElement' },
  { name: 'stock except', shape: 'perElement' },
  { name: 'belt', shape: 'initialEquation' },
  { name: 'line', shape: 'initialEquation' },
  { name: 'flow scalar', shape: 'equation' },
  { name: 'flow a2a', shape: 'applyToAll' },
  { name: 'flow per element', shape: 'perElement' },
  { name: 'flow except', shape: 'perElement' },
  { name: 'leaving', shape: 'equation' },
  { name: 'aux scalar', shape: 'equation' },
  { name: 'price', shape: 'applyToAll' },
  { name: 'aux two dims', shape: 'applyToAll' },
  { name: 'aux per element', shape: 'perElement' },
  { name: 'aux except', shape: 'perElement' },
  { name: 'element lookups', shape: 'perElement' },
  { name: 'active initial', shape: 'equation' },
  { name: 'lookup only', shape: 'equation' },
  { name: 'with lookup', shape: 'equation' },
  { name: 'uneven lookup', shape: 'equation' },
  { name: 'a2a with lookup', shape: 'applyToAll' },
  { name: 'data', shape: 'equation' },
  { name: 'mod', shape: 'module' },
];

const identOf = (name: string): string => name.replace(/ /g, '_');

function withEquation(v: JsonVar, shape: EquationShape, equation: string): Record<string, unknown> {
  if (shape === 'applyToAll') {
    return { arrayedEquation: { ...(v.arrayedEquation as object), equation } };
  }
  return { [shape]: equation };
}

describeWithEngine('details-panel edits on the real engine', () => {
  beforeAll(async () => {
    await loadEngine();
  });

  afterEach(async () => {
    cleanup();
    rs.restoreAllMocks();
    canvasProps = undefined;
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  for (const { name, shape } of ROWS) {
    it(`${name}: an edit changes the one field it names, and undo takes it back`, async () => {
      const m = await mount();
      const original = await m.engineVariables();
      const ident = identOf(name);
      const states = [original];

      const step = async (
        change: Record<string, unknown>,
        op: JsonModelOperation,
        edit: () => Promise<void>,
      ): Promise<void> => {
        const before = states[states.length - 1];
        const patches = m.patches.length;
        openPanel(name);
        await edit();
        pressCanvas();
        await m.settle();
        const after = withVariable(before, name, change);
        expect(await m.engineVariables()).toEqual(after);
        expect(m.patches).toHaveLength(patches + 1);
        expect(lastOps(m)).toEqual([op]);
        states.push(after);
      };

      await step({ units: 'gizmos' }, { type: 'setUnits', payload: { ident, units: 'gizmos' } }, () =>
        setField('.unitsEditor', 'gizmos'),
      );
      await step({ documentation: 'after' }, { type: 'setNotes', payload: { ident, notes: 'after' } }, () =>
        setField('.notesEditor', 'after'),
      );
      if (shape === 'perElement' || shape === 'module') {
        // No field an equation could be typed into.
        openPanel(name);
        expect(document.querySelector('.eqnPreview')).toBeNull();
        expect(document.querySelector('.eqnEditor')).toBeNull();
      } else {
        const equation = '12 + 1';
        await step(
          withEquation(original.get(name)!, shape, equation),
          { type: 'setEquation', payload: { ident, equation } },
          async () => {
            openEquationEditor();
            await setField('.eqnEditor', equation);
          },
        );
      }

      while (states.length > 1) {
        states.pop();
        pressButton('Undo');
        await m.settle();
        expect(await m.engineVariables()).toEqual(states[states.length - 1]);
      }
      expect(alerts()).toEqual([]);
    });
  }

  it('every variable of the fixture that the panels open on is a row, or plumbing for one', () => {
    const plumbing = ['onto', 'off', 'joining'];
    expect(
      ROWS.map((row) => row.name)
        .concat(plumbing)
        .sort(),
    ).toEqual(all.map((v) => v.name).sort());
  });

  it('clearing a field clears it and keeps the variable: an apply-to-all equation keeps its dimensions', async () => {
    const m = await mount();
    const before = await m.engineVariables();
    openPanel('price');
    openEquationEditor();
    await setField('.eqnEditor', '');
    await setField('.unitsEditor', '');
    await setField('.notesEditor', '');
    pressCanvas();
    await m.settle();
    const price = without(before.get('price')!, 'units', 'documentation');
    const cleared: JsonVar = { ...price, ...withEquation(price, 'applyToAll', '') };
    expect(await m.engineVariables()).toEqual(new Map([...before].map(([n, v]) => [n, n === 'price' ? cleared : v])));
    // All three fields are one submission, so one patch and one undo step.
    expect(lastOps(m)).toEqual([
      { type: 'setEquation', payload: { ident: 'price', equation: '' } },
      { type: 'setUnits', payload: { ident: 'price', units: '' } },
      { type: 'setNotes', payload: { ident: 'price', notes: '' } },
    ]);

    // The panel still reads it as apply-to-all: text typed into the empty field
    // is the equation of every element again.
    openPanel('price');
    openEquationEditor();
    expect(fieldText('.eqnEditor')).toBe('');
    await setField('.eqnEditor', '12');
    pressCanvas();
    await m.settle();
    expect((await m.engineVariables()).get('price')!.arrayedEquation).toEqual({
      dimensions: ['Region'],
      equation: '12',
    });
    expect(alerts()).toEqual([]);
  });

  it('a scalar cleared of its equation, units and notes is a scalar with none of them', async () => {
    const m = await mount();
    const before = await m.engineVariables();
    openPanel('aux scalar');
    openEquationEditor();
    await setField('.eqnEditor', '');
    await setField('.unitsEditor', '');
    await setField('.notesEditor', '');
    pressCanvas();
    await m.settle();
    const after = (await m.engineVariables()).get('aux scalar')!;
    expect(without(after, 'equation')).toEqual(
      without(before.get('aux scalar')!, 'equation', 'units', 'documentation'),
    );
    expect(after.equation ?? '').toBe('');
  });

  it('an apply-to-all edit shows its text while it is pending, once it lands, and after an undo and a redo', async () => {
    const m = await mount();
    const preview = () => document.querySelector('.eqnPreview')?.textContent;
    openPanel('price');
    expect(preview()).toBe('10');
    openEquationEditor();
    await setField('.eqnEditor', '12');
    pressCanvas();
    // Reopened while its edit is pending.
    openPanel('data');
    openPanel('price');
    expect(preview()).toBe('12');
    await m.settle();
    expect(preview()).toBe('12');
    pressButton('Undo');
    await m.settle();
    expect(preview()).toBe('10');
    pressButton('Redo');
    await m.settle();
    expect(preview()).toBe('12');
    expect((await m.engineVariables()).get('price')!.arrayedEquation).toEqual({
      dimensions: ['Region'],
      equation: '12',
    });
  });

  it("an apply-to-all equation's error is underlined where the engine says it is", async () => {
    const m = await mount();
    openPanel('price');
    openEquationEditor();
    await setField('.eqnEditor', '3 + bad_ref * 2');
    pressCanvas();
    await m.settle();
    const marked = () => Array.from(document.querySelectorAll('.eqnEditor .eqnError'), (el) => el.textContent).join('');
    expect(marked()).toBe('bad_ref');
    expect(fieldText('.eqnEditor')).toBe('3 + bad_ref * 2');
  });

  it('a per-element variable keeps its panel and its equations through an undo of its units', async () => {
    const m = await mount();
    const listed = () => Array.from(document.querySelectorAll('dt, dd'), (el) => el.textContent);
    const equations = ['north', '1', 'all others', '7'];
    openPanel('aux except');
    expect(listed()).toEqual(equations);
    await setField('.unitsEditor', 'gizmos');
    pressCanvas();
    await m.settle();
    expect(listed()).toEqual(equations);
    pressButton('Undo');
    await m.settle();
    expect(fieldText('.unitsEditor')).toBe('');
    expect(listed()).toEqual(equations);
  });

  const openLookup = (name: string): void => {
    openPanel(name);
    act(() => {
      fireEvent.mouseDown(screen.getByRole('tab', { name: 'Lookup Function' }), { button: 0 });
    });
  };

  const setLookupField = (label: string, value: string): void => {
    act(() => {
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
    });
  };

  // Drag the table's last point to the top of the chart.
  const dragLastPointToTop = (): void => {
    const overlay = document.querySelector('.overlay') as SVGRectElement;
    act(() => {
      fireEvent.pointerDown(overlay, { clientX: 5000, clientY: 150, pointerId: 1 });
    });
    act(() => {
      fireEvent.pointerMove(overlay, { clientX: 5000, clientY: -5000, pointerId: 1 });
    });
    act(() => {
      fireEvent.pointerUp(overlay, { clientX: 5000, clientY: -5000, pointerId: 1 });
    });
  };

  const pressCancel = (): void => {
    act(() => {
      fireEvent.click(screen.getByText('Cancel'));
    });
  };

  // What each edit makes of a table with x points of its own, (0,0), (2,4),
  // (10,8) drawn over 0 to 10, and of one with y points 0, 5, 9 spread over 0 to
  // 2. These are every control the lookup editor has; it has none for the kind,
  // which the first table's rows hold to 'discrete'.
  const lookupEdits: ReadonlyArray<{
    name: string;
    edit: () => void;
    uneven: Record<string, unknown>;
    even: Record<string, unknown>;
  }> = [
    {
      name: 'a dragged point',
      edit: dragLastPointToTop,
      uneven: {
        points: [
          [0, 0],
          [2, 4],
          [10, 10],
        ],
      },
      even: { yPoints: [0, 5, 10] },
    },
    {
      name: 'Y axis max',
      edit: () => setLookupField('Y axis max', '20'),
      uneven: { yScale: { min: 0, max: 20 } },
      even: { yScale: { min: 0, max: 20 } },
    },
    {
      name: 'Y axis min',
      edit: () => setLookupField('Y axis min', '-5'),
      uneven: { yScale: { min: -5, max: 10 } },
      even: { yScale: { min: -5, max: 10 } },
    },
    // Points of their own stay where they are: the x scale is the range they are
    // drawn over. Without, the x scale is where the points are.
    {
      name: 'X axis max',
      edit: () => setLookupField('X axis max', '20'),
      uneven: { xScale: { min: 0, max: 20 } },
      even: { xScale: { min: 0, max: 20 } },
    },
    {
      name: 'X axis min',
      edit: () => setLookupField('X axis min', '-10'),
      uneven: { xScale: { min: -10, max: 10 } },
      even: { xScale: { min: -10, max: 2 } },
    },
    {
      name: 'the point count, spread over the x the table covers',
      edit: () => setLookupField('Datapoint Count', '5'),
      uneven: {
        points: [
          [0, 0],
          [2.5, 4.25],
          [5, 5.5],
          [7.5, 6.75],
          [10, 8],
        ],
      },
      even: { yPoints: [0, 2.5, 5, 7, 9] },
    },
    {
      name: 'the point count after the x scale',
      edit: () => {
        setLookupField('X axis max', '20');
        setLookupField('Datapoint Count', '2');
      },
      uneven: {
        points: [
          [0, 0],
          [10, 8],
        ],
        xScale: { min: 0, max: 20 },
      },
      even: { yPoints: [0, 9], xScale: { min: 0, max: 20 } },
    },
    {
      name: 'a point count typed through a smaller one',
      edit: () => {
        setLookupField('Datapoint Count', '1');
        setLookupField('Datapoint Count', '5');
      },
      uneven: {
        points: [
          [0, 0],
          [2.5, 4.25],
          [5, 5.5],
          [7.5, 6.75],
          [10, 8],
        ],
      },
      even: { yPoints: [0, 2.5, 5, 7, 9] },
    },
    {
      name: 'a point count put back, across an x scale edit',
      edit: () => {
        setLookupField('Datapoint Count', '2');
        setLookupField('X axis max', '20');
        setLookupField('Datapoint Count', '3');
      },
      uneven: { xScale: { min: 0, max: 20 } },
      even: { xScale: { min: 0, max: 20 } },
    },
    {
      name: 'the point count of a table with a dragged point',
      edit: () => {
        dragLastPointToTop();
        setLookupField('Datapoint Count', '2');
      },
      uneven: {
        points: [
          [0, 0],
          [10, 10],
        ],
      },
      even: { yPoints: [0, 10] },
    },
    {
      name: 'the point count after Cancel put a dragged point back',
      edit: () => {
        dragLastPointToTop();
        pressCancel();
        setLookupField('Datapoint Count', '2');
      },
      uneven: {
        points: [
          [0, 0],
          [10, 8],
        ],
      },
      even: { yPoints: [0, 9] },
    },
  ];
  for (const { name, edit, ...expected } of lookupEdits) {
    for (const [variable, change] of [
      ['uneven lookup', expected.uneven],
      ['a2a with lookup', expected.even],
    ] as const) {
      it(`${variable}: a lookup edit changes what it says and keeps the rest (${name})`, async () => {
        const m = await mount();
        const before = await m.engineVariables();
        openLookup(variable);
        edit();
        act(() => {
          fireEvent.click(screen.getByText('Save'));
        });
        await m.settle();
        const lookup = { ...(before.get(variable)!.graphicalFunction as object), ...change };
        expect(await m.engineVariables()).toEqual(withVariable(before, variable, { graphicalFunction: lookup }));
        expect(lastOps(m).map((op) => op.type)).toEqual(['setLookup']);
        pressButton('Undo');
        await m.settle();
        expect(await m.engineVariables()).toEqual(before);
        expect(alerts()).toEqual([]);
      });
    }
  }

  for (const name of [
    'data',
    'flow scalar',
    'price',
    'flow a2a',
    'aux per element',
    'flow except',
    'element lookups',
  ]) {
    it(`${name}: adding a lookup and removing it leave everything else as it was`, async () => {
      const m = await mount();
      const before = await m.engineVariables();
      openLookup(name);
      act(() => {
        fireEvent.click(screen.getByText('Add lookup table'));
      });
      await m.settle();
      const added = (await m.engineVariables()).get(name)!.graphicalFunction as Record<string, unknown>;
      expect(added.yPoints).toEqual([0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1]);
      expect(await m.engineVariables()).toEqual(withVariable(before, name, { graphicalFunction: added }));
      act(() => {
        fireEvent.click(screen.getByText('Remove'));
      });
      await m.settle();
      expect(await m.engineVariables()).toEqual(before);
      expect(lastOps(m)).toEqual([{ type: 'setLookup', payload: { ident: identOf(name), lookup: null } }]);
      expect(alerts()).toEqual([]);
    });
  }
});
