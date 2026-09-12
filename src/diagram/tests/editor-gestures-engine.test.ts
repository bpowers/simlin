// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Every editing gesture, committed through the Editor into the REAL WASM
// engine. The Editor mounts with the Canvas mocked (its props are captured);
// each row plans a gesture with the production planner on the rendered view and
// hands the commit to onCommitGesture exactly as the Canvas's pointer-up does.
// Once the patch lands, the engine's own serialized project must hold M1 (kind
// agreement), M2 (every stock's lists name exactly the flows attached to it, once),
// M3 (referential integrity) and the strict flow invariants over the flows the
// gesture routed, and the rendered view must equal the engine's.
//
// Rows are keyed by GESTURE_KINDS minus the kinds that commit no view edit
// through onCommitGesture: createElement (placed by the name editor through
// onCreateVariable; editor-engine-races.test.ts T6/T11b), rubberBand (a
// selection) and pan (a viewport).
//
// What this establishes: a committed plan is a patch the engine accepts, and the
// model the engine derives from it holds M1-M3 and G1-G8. What it does not: the
// Canvas producing these gestures from pointer events (canvas-gestures-*.test.tsx)
// or edits racing an in-flight patch (editor-engine-races.test.ts).

import { it, expect, beforeAll, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, cleanup, render } from '@testing-library/react';

import * as react from 'react' with { rstest: 'importActual' };

import { canonicalize } from '@simlin/core/canonicalize';
import { projectFromJson, type LinkViewElement, type Model, type StockFlowView } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

import { ProjectController } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';
import { planGesture, type Gesture, type GesturePlan } from '../gesture-planner';
import { GESTURE_KINDS } from '../gesture-planner/types';
import { describeWithEngine, loadEngine, mainModel } from './support/engine';
import { checkFlowInvariants, formatFlowViolations } from './support/flow-invariants';
import { routedFlows, type Pt } from './support/gesture-fixtures';
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
}));

import { Editor, type EditorProps } from '../Editor';

// A (uid 1) -> B (uid 2) through f (uid 4); a cloud (uid 21) into C (uid 3)
// through h (uid 5); aux x (uid 7) links into f (link uid 8); aux y (uid 9).
function projectJson(): string {
  const model = {
    name: 'main',
    stocks: [
      { name: 'A', initialEquation: '10', inflows: [], outflows: ['f'] },
      { name: 'B', initialEquation: '0', inflows: ['f'], outflows: [] },
      { name: 'C', initialEquation: '0', inflows: ['h'], outflows: [] },
    ],
    flows: [
      { name: 'f', equation: 'x' },
      { name: 'h', equation: '1' },
    ],
    auxiliaries: [
      { name: 'x', equation: '0.1' },
      { name: 'y', equation: '2' },
    ],
    views: [
      {
        elements: [
          { type: 'stock', uid: 1, name: 'A', x: 100, y: 100 },
          { type: 'stock', uid: 2, name: 'B', x: 400, y: 100 },
          { type: 'stock', uid: 3, name: 'C', x: 400, y: 300 },
          {
            type: 'flow',
            uid: 4,
            name: 'f',
            x: 250,
            y: 100,
            points: [
              { x: 122.5, y: 100, attachedToUid: 1 },
              { x: 377.5, y: 100, attachedToUid: 2 },
            ],
          },
          {
            type: 'flow',
            uid: 5,
            name: 'h',
            x: 310,
            y: 300,
            points: [
              { x: 250, y: 300, attachedToUid: 21 },
              { x: 377.5, y: 300, attachedToUid: 3 },
            ],
          },
          { type: 'cloud', uid: 21, flowUid: 5, x: 250, y: 300 },
          { type: 'aux', uid: 7, name: 'x', x: 250, y: 20 },
          { type: 'link', uid: 8, fromUid: 7, toUid: 4 },
          { type: 'aux', uid: 9, name: 'y', x: 100, y: 400 },
        ],
      },
    ],
  };
  return JSON.stringify({ name: 'gestures', simSpecs: { startTime: 0, endTime: 3, dt: '1' }, models: [model] });
}

interface Mounted {
  readonly controller: ProjectController;
  settle(): Promise<void>;
  engineModel(): Promise<Model>;
}

async function mount(): Promise<Mounted> {
  const controllers: ProjectController[] = [];
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
        name: 'gestures',
        onSave: async () => 1,
      } as EditorProps),
    );
  });
  const controller = controllers[0];
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await act(async () => {
        await controller.whenIdle();
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
  };
  await settle();
  return {
    controller,
    settle,
    engineModel: async () => {
      const serialized = await controller.query((engine) => engine.serializeJson(undefined, true));
      return mainModel(projectFromJson(JSON.parse(serialized!) as JsonProject).models);
    },
  };
}

// The gesture released at `current`, planned on the rendered view with the
// Canvas's inputs and committed as the Canvas's pointer-up commits it.
function release(gesture: Gesture, selection: ReadonlySet<number>, press: Pt, current: Pt): GesturePlan {
  const p = canvasProps!;
  const plan = planGesture({
    view: p.view,
    variables: p.model.variables,
    selection,
    gesture,
    press,
    current,
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
      token: p.token,
      baseView: p.view,
      editName: plan.handoff?.editName,
    });
  });
  return plan;
}

function lists(model: Model, ident: string): { inflows: string[]; outflows: string[] } {
  const stock = model.variables.get(ident);
  if (stock?.type !== 'stock') {
    throw new Error(`no stock ${ident}`);
  }
  return { inflows: stock.inflows.map(canonicalize).sort(), outflows: stock.outflows.map(canonicalize).sort() };
}

function engineElement(model: Model, uid: number) {
  const el = model.views[0].elements.find((e) => e.uid === uid);
  if (el === undefined) {
    throw new Error(`no uid ${uid} on the engine's view`);
  }
  return el;
}

function shape(view: StockFlowView): string[] {
  return view.elements
    .map((el) =>
      el.type === 'flow'
        ? `${el.uid}:flow:${canonicalize(el.name)}:${el.points[0].attachedToUid}->${el.points[el.points.length - 1].attachedToUid}`
        : `${el.uid}:${el.type}`,
    )
    .sort();
}

interface Row {
  readonly name: string;
  readonly gesture: Gesture;
  readonly selection: ReadonlySet<number>;
  readonly press: Pt;
  readonly current: Pt;
  readonly check: (model: Model) => void;
}

const ROWS: Readonly<Record<Exclude<Gesture['kind'], 'createElement' | 'rubberBand' | 'pan'>, Row>> = {
  moveSelection: {
    name: 'stock A dragged down: f routes to its moved face',
    gesture: { kind: 'moveSelection' },
    selection: new Set([1]),
    press: { x: 100, y: 100 },
    current: { x: 100, y: 160 },
    check: (model) => {
      expect(engineElement(model, 1)).toMatchObject({ x: 100, y: 160 });
      expect(lists(model, 'a').outflows).toEqual(['f']);
    },
  },
  slideValve: {
    name: 'f`s valve slid along its pipe',
    gesture: { kind: 'slideValve', flow: 4 },
    selection: new Set([4]),
    press: { x: 250, y: 100 },
    current: { x: 300, y: 100 },
    check: (model) => {
      expect((engineElement(model, 4) as { x: number }).x).toBeCloseTo(300);
    },
  },
  offsetSegment: {
    name: 'f offset perpendicular into a bracket between A and B',
    gesture: { kind: 'offsetSegment', flow: 4, segmentIndex: 0 },
    selection: new Set([4]),
    press: { x: 250, y: 100 },
    current: { x: 250, y: 170 },
    check: (model) => {
      const f = engineElement(model, 4);
      expect(f.type === 'flow' && f.points.length > 2).toBe(true);
    },
  },
  flowEndpoint: {
    name: 'h`s sink moved from C onto B (M2: C loses h, B gains it)',
    gesture: { kind: 'flowEndpoint', flow: 5, end: 'sink' },
    selection: new Set([5]),
    press: { x: 377.5, y: 300 },
    current: { x: 400, y: 100 },
    check: (model) => {
      expect(lists(model, 'c').inflows).toEqual([]);
      expect(lists(model, 'b').inflows).toEqual(['f', 'h']);
    },
  },
  createFlow: {
    name: 'a flow drawn from C onto A (M1/M2: a new flow variable listed by both)',
    gesture: { kind: 'createFlow', from: { stock: 3 } },
    selection: new Set(),
    press: { x: 400, y: 300 },
    current: { x: 100, y: 100 },
    check: (model) => {
      expect(model.variables.get('new_flow')?.type).toBe('flow');
      expect(lists(model, 'c').outflows).toEqual(['new_flow']);
      expect(lists(model, 'a').inflows).toEqual(['new_flow']);
    },
  },
  createLink: {
    name: 'a link drawn from y to x',
    gesture: { kind: 'createLink', from: 9 },
    selection: new Set(),
    press: { x: 100, y: 400 },
    current: { x: 250, y: 20 },
    check: (model) => {
      const links = model.views[0].elements.filter((e): e is LinkViewElement => e.type === 'link');
      expect(links.some((l) => l.fromUid === 9 && l.toUid === 7)).toBe(true);
    },
  },
  linkEndpoint: {
    name: 'x`s link arrowhead moved from f onto y',
    gesture: { kind: 'linkEndpoint', link: 8 },
    selection: new Set([8]),
    press: { x: 250, y: 100 },
    current: { x: 100, y: 400 },
    check: (model) => {
      expect(engineElement(model, 8)).toMatchObject({ fromUid: 7, toUid: 9 });
    },
  },
  linkArc: {
    name: 'x`s link curved by its body',
    gesture: { kind: 'linkArc', link: 8 },
    selection: new Set([8]),
    press: { x: 250, y: 60 },
    current: { x: 290, y: 60 },
    check: (model) => {
      const arc = (engineElement(model, 8) as LinkViewElement).arc;
      expect(arc !== undefined && Math.abs(arc) > 1).toBe(true);
    },
  },
  label: {
    name: 'x`s label dragged to the top',
    gesture: { kind: 'label', uid: 7 },
    selection: new Set([7]),
    press: { x: 250, y: -20 },
    current: { x: 250, y: -20 },
    check: (model) => {
      expect(engineElement(model, 7)).toMatchObject({ labelSide: 'top' });
    },
  },
};

describeWithEngine('Editor + real engine: every editing gesture lands holding M1-M3 and G1-G8', () => {
  beforeAll(async () => {
    await loadEngine();
  });

  afterEach(async () => {
    cleanup();
    rs.restoreAllMocks();
    canvasProps = undefined;
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  it('has a row for every kind that commits a view edit', () => {
    const covered = new Set<string>([...Object.keys(ROWS), 'createElement', 'rubberBand', 'pan']);
    expect([...GESTURE_KINDS].filter((k) => !covered.has(k))).toEqual([]);
  });

  for (const [kind, row] of Object.entries(ROWS)) {
    it(`${kind}: ${row.name}`, async () => {
      const m = await mount();
      const base = { model: canvasProps!.model, view: canvasProps!.view };
      const plan = release(row.gesture, row.selection, row.press, row.current);
      await m.settle();

      const model = await m.engineModel();
      const view = model.views[0];
      expect(
        formatViewViolations([
          ...checkKindAgreement(view, model.variables),
          ...checkReferentialIntegrity(view),
          ...checkStockListDuplicates(model.variables),
          ...checkStockFlowAgreement(view, model.variables),
        ]),
      ).toBe('');
      expect(formatFlowViolations(checkFlowInvariants(view, { mode: 'strict', routed: routedFlows(base, plan) }))).toBe(
        '',
      );
      expect(shape(canvasProps!.view)).toEqual(shape(view));
      row.check(model);
    });
  }
});
