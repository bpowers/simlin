// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Fixtures for the gesture planner tests. Scenes are engine JSON loaded
 * through the production `modelFromJson`, with a variable for every named
 * element and stock inflow/outflow lists derived from the view's attachments,
 * which is what a well-formed saved model gives the editor. Inputs to
 * `planGesture` are assembled with the Canvas's defaults; routed flows are read
 * off the plan (every flow element the plan replaced).
 */

import type { JsonModel, JsonViewElement } from '@simlin/engine';
import { canonicalize } from '@simlin/core/canonicalize';
import { modelFromJson, type Model, type StockFlowView, type UID } from '@simlin/core/datamodel';

import type { GesturePlan, PlanInput, PressGesture } from '../../gesture-planner';
import { allocateVariableName } from '../../variable-names';
import { checkFlowInvariants, formatFlowViolations } from './flow-invariants';
import { checkReferentialIntegrity, formatViewViolations } from './view-invariants';

export type Pt = { readonly x: number; readonly y: number };

export interface Scene {
  readonly model: Model;
  readonly view: StockFlowView;
}

type JsonFlowElement = Extract<JsonViewElement, { type: 'flow' }>;

/**
 * A scene from view elements. Every named element gets a variable of its kind;
 * a stock lists the flows attached to it; `omit` leaves out the variables of the
 * named elements it lists (an element naming no variable, as an import can
 * carry); the auxes `arrayed` lists apply one equation over a dimension, so
 * their elements render (and anchor links) as stacked, arrayed shapes.
 */
export function scene(
  elements: readonly JsonViewElement[],
  omit: readonly string[] = [],
  arrayed: readonly string[] = [],
): Scene {
  const named = (type: string): string[] =>
    elements
      .filter((el) => el.type === type && !omit.includes((el as { name: string }).name))
      .map((el) => (el as { name: string }).name);
  const flows = elements.filter((el): el is JsonFlowElement => el.type === 'flow');
  const listed = (stockUid: UID, end: 'source' | 'sink'): string[] =>
    flows
      .filter((f) => {
        const p = end === 'source' ? f.points![0] : f.points![f.points!.length - 1];
        return p?.attachedToUid === stockUid;
      })
      .map((f) => f.name);
  const stocks = elements
    .filter((el) => el.type === 'stock' && !omit.includes((el as { name: string }).name))
    .map((el) => ({
      name: (el as { name: string }).name,
      initialEquation: '1',
      inflows: listed(el.uid!, 'sink'),
      outflows: listed(el.uid!, 'source'),
    }));
  const json = {
    name: 'main',
    stocks,
    flows: named('flow').map((name) => ({ name, equation: '1' })),
    auxiliaries: named('aux').map((name) =>
      arrayed.includes(name)
        ? { name, arrayedEquation: { dimensions: ['D'], equation: '1' } }
        : { name, equation: '1' },
    ),
    modules: named('module').map((name) => ({ name, modelName: 'sub' })),
    views: [{ elements: [...elements], viewBox: { x: 0, y: 0, width: 1000, height: 1000 }, zoom: 1 }],
  } as JsonModel;
  const model = modelFromJson(json);
  return { model, view: model.views[0] };
}

export const stock = (uid: UID, name: string, x: number, y: number): JsonViewElement => ({
  type: 'stock',
  uid,
  name,
  x,
  y,
});

export const aux = (uid: UID, name: string, x: number, y: number): JsonViewElement => ({
  type: 'aux',
  uid,
  name,
  x,
  y,
});

export const cloud = (uid: UID, flowUid: UID, x: number, y: number): JsonViewElement => ({
  type: 'cloud',
  uid,
  flowUid,
  x,
  y,
});

export const link = (uid: UID, fromUid: UID, toUid: UID, arc?: number): JsonViewElement =>
  arc === undefined ? { type: 'link', uid, fromUid, toUid } : { type: 'link', uid, fromUid, toUid, arc };

export function flow(
  uid: UID,
  name: string,
  valve: Pt,
  points: ReadonlyArray<readonly [number, number, UID?]>,
): JsonViewElement {
  return {
    type: 'flow',
    uid,
    name,
    x: valve.x,
    y: valve.y,
    points: points.map(([x, y, attachedToUid]) => (attachedToUid === undefined ? { x, y } : { x, y, attachedToUid })),
  };
}

/** Stock S (uid 1) at (100,100) -> cloud (uid 2) at (300,100) through flow F (uid 3), its source on S's right face. */
export function stockToCloud(): Scene {
  return scene([
    stock(1, 'S', 100, 100),
    cloud(2, 3, 300, 100),
    flow(3, 'F', { x: 200, y: 100 }, [
      [122.5, 100, 1],
      [300, 100, 2],
    ]),
  ]);
}

/** Stock A (uid 1) at (100,100) -> stock B (uid 2) at (400,100) through flow F (uid 3), on facing faces. */
export function stockToStock(): Scene {
  return scene([
    stock(1, 'A', 100, 100),
    stock(2, 'B', 400, 100),
    flow(3, 'F', { x: 250, y: 100 }, [
      [122.5, 100, 1],
      [377.5, 100, 2],
    ]),
  ]);
}

/** Auxes a (uid 10) at (100,300), b (uid 11) at (300,300), c (uid 12) at (300,450), and a link a -> b (uid 13). */
export function linkedAuxes(): Scene {
  return scene([aux(10, 'a', 100, 300), aux(11, 'b', 300, 300), aux(12, 'c', 300, 450), link(13, 10, 11, 20)]);
}

/** The editor's allocator over a scene's variables and view names (see ProjectController.usedIdents). */
export function namesOf(s: Scene): (base: string) => string {
  const used = new Set<string>(s.model.variables.keys());
  for (const el of s.view.elements) {
    if ('name' in el && typeof el.name === 'string') {
      used.add(canonicalize(el.name));
    }
  }
  return (base) => allocateVariableName(base, used);
}

export function planInput(
  s: Scene,
  gesture: PressGesture,
  press: Pt,
  current: Pt,
  overrides: Partial<PlanInput> = {},
): PlanInput {
  return {
    view: s.view,
    variables: s.model.variables,
    selection: new Set(),
    gesture,
    press,
    current,
    zoom: 1,
    pointerType: 'mouse',
    readOnly: false,
    names: namesOf(s),
    ...overrides,
  };
}

/** The view a plan renders and commits. */
export function planned(s: Scene, plan: GesturePlan): StockFlowView {
  return { ...s.view, elements: plan.elements, nextUid: plan.nextUid };
}

/**
 * Every flow the plan created or whose path or valve it changed: the flows its
 * edit routed. A flow whose label alone changed was not routed.
 */
export function routedFlows(s: Scene, plan: GesturePlan): Set<UID> {
  const before = new Map(s.view.elements.map((el) => [el.uid, el]));
  const out = new Set<UID>();
  for (const el of plan.elements) {
    const b = before.get(el.uid);
    if (el.type !== 'flow' || b === el) {
      continue;
    }
    const same =
      b?.type === 'flow' &&
      b.x === el.x &&
      b.y === el.y &&
      b.points.length === el.points.length &&
      b.points.every(
        (p, i) => p.x === el.points[i].x && p.y === el.points[i].y && p.attachedToUid === el.points[i].attachedToUid,
      );
    if (!same) {
      out.add(el.uid);
    }
  }
  return out;
}

/**
 * The strict flow violations of the routed flows and the M3 violations of the
 * planned view, formatted so a failure shows every arm and its numbers; empty
 * when the plan's view holds them.
 */
export function committedReport(s: Scene, plan: GesturePlan): string {
  const view = planned(s, plan);
  const flows = checkFlowInvariants(view, { mode: 'strict', routed: routedFlows(s, plan) });
  const refs = checkReferentialIntegrity(view);
  return [formatFlowViolations(flows), formatViewViolations(refs)].filter((t) => t !== '').join('\n');
}

export function elementOf(plan: GesturePlan, uid: UID) {
  const el = plan.elements.find((e) => e.uid === uid);
  if (el === undefined) {
    throw new Error(`no element ${uid}`);
  }
  return el;
}
