// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  type CloudViewElement,
  type Equation,
  type FlowViewElement,
  type Stock,
  type StockFlowView,
  type StockViewElement,
  type Variable,
  type ViewElement,
} from '@simlin/core/datamodel';
import type { JsonModelOperation } from '@simlin/engine';

import {
  computeFlowAttachment,
  fauxCloudTargetUid,
  growEndpointDrag,
  growInCreationFlow,
  inCreationCloudUid,
  inCreationUid,
  type FlowAttachParams,
} from '../flow-attach';
import {
  fauxCloudTargetUid as canvasFauxCloudTargetUid,
  inCreationCloudUid as canvasInCreationCloudUid,
  inCreationUid as canvasInCreationUid,
} from '../drawing/Canvas';
import {
  fauxCloudTargetUid as sharedFauxCloudTargetUid,
  inCreationCloudUid as sharedInCreationCloudUid,
  inCreationUid as sharedInCreationUid,
} from '../drawing/creation-sentinels';
import { StockWidth, StockHeight } from '../drawing/default';

// ----- fixture helpers (mirroring flow-routing.test.ts patterns) -----

function makeStockEl(
  uid: number,
  ident: string,
  x: number,
  y: number,
  inflows: number[] = [],
  outflows: number[] = [],
): StockViewElement {
  return {
    type: 'stock',
    uid,
    name: ident,
    ident,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
    inflows,
    outflows,
  };
}

function makeFlowEl(
  uid: number,
  ident: string,
  x: number,
  y: number,
  points: Array<{ x: number; y: number; attachedToUid?: number }>,
): FlowViewElement {
  return {
    type: 'flow',
    uid,
    name: ident,
    ident,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    points: points.map((p) => ({ x: p.x, y: p.y, attachedToUid: p.attachedToUid })),
    isZeroRadius: false,
  };
}

function makeCloudEl(uid: number, flowUid: number, x: number, y: number): CloudViewElement {
  return {
    type: 'cloud',
    uid,
    flowUid,
    x,
    y,
    isZeroRadius: false,
    ident: undefined,
  };
}

function makeView(elements: ViewElement[], nextUid: number): StockFlowView {
  return {
    nextUid,
    elements,
    viewBox: { x: 0, y: 0, width: 1000, height: 1000 },
    zoom: 1,
    useLetteredPolarity: false,
  };
}

const emptyEquation: Equation = { type: 'scalar', equation: '' };

function makeStockVar(ident: string, inflows: string[] = [], outflows: string[] = []): Stock {
  return {
    type: 'stock',
    ident,
    equation: emptyEquation,
    documentation: '',
    units: '',
    inflows,
    outflows,
    nonNegative: false,
    canBeModuleInput: false,
    isPublic: false,
    activeInitial: undefined,
    dataSource: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
  };
}

function varsOf(...stocks: Stock[]): ReadonlyMap<string, Variable> {
  return new Map<string, Variable>(stocks.map((s) => [s.ident, s]));
}

const NO_DELTA = { x: 0, y: 0 };

function params(overrides: Partial<FlowAttachParams> & { flow: FlowViewElement }): FlowAttachParams {
  return {
    targetUid: 0,
    cursorMoveDelta: NO_DELTA,
    fauxTargetCenter: undefined,
    inCreation: false,
    isSourceAttach: false,
    ...overrides,
  };
}

// Convenience: assert exactly one updateStockFlows op for `ident` and return it.
function stockFlowsOpFor(ops: readonly JsonModelOperation[], ident: string): JsonModelOperation {
  const matches = ops.filter(
    (op) => op.type === 'updateStockFlows' && (op as { payload: { ident: string } }).payload.ident === ident,
  );
  expect(matches.length).toBe(1);
  return matches[0];
}

function payloadOf(op: JsonModelOperation): { ident: string; inflows: string[]; outflows: string[] } {
  return (op as unknown as { payload: { ident: string; inflows: string[]; outflows: string[] } }).payload;
}

describe('computeFlowAttachment', () => {
  // The creation sentinels are defined once in drawing/creation-sentinels and
  // re-exported by both flow-attach and Canvas. This guards that every import
  // path stays wired to that single source (a trivial identity now, but it
  // keeps the compatibility of both re-export paths under test).
  it('keeps creation sentinel constants in sync across import paths', () => {
    expect(inCreationUid).toBe(sharedInCreationUid);
    expect(inCreationCloudUid).toBe(sharedInCreationCloudUid);
    expect(fauxCloudTargetUid).toBe(sharedFauxCloudTargetUid);
    expect(canvasInCreationUid).toBe(sharedInCreationUid);
    expect(canvasInCreationCloudUid).toBe(sharedInCreationCloudUid);
    expect(canvasFauxCloudTargetUid).toBe(sharedFauxCloudTargetUid);
  });

  describe('sink reattach', () => {
    it('stock -> stock: detaches old inflow, attaches new inflow', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const newSink = makeStockEl(4, 'new_sink', 200, 300);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, newSink, flow], 5);
      const variables = varsOf(makeStockVar('old_sink', ['f']), makeStockVar('new_sink'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4 }));

      // attach new inflow on new_sink, detach from old_sink
      const attach = payloadOf(stockFlowsOpFor(result.ops, 'new_sink'));
      expect(attach.inflows).toEqual(['f']);
      const detach = payloadOf(stockFlowsOpFor(result.ops, 'old_sink'));
      expect(detach.inflows).toEqual([]);
      // no clouds created or deleted; element count unchanged
      expect(result.elements.length).toBe(4);
      expect(result.isCreatingNew).toBe(false);
      expect(result.selection).toBeUndefined();
      // flow's last point now references the new sink
      const outFlow = result.elements.find((e) => e.uid === 3) as FlowViewElement;
      expect(outFlow.points[outFlow.points.length - 1].attachedToUid).toBe(4);
    });

    it('stock -> empty space: creates a cloud at release, detaches inflow', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, flow], 5);
      const variables = varsOf(makeStockVar('old_sink', ['f']));

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, cursorMoveDelta: { x: -40, y: 0 } }),
      );

      // a new cloud was created (uid 5)
      const clouds = result.elements.filter((e) => e.type === 'cloud');
      expect(clouds.length).toBe(1);
      expect(result.nextUid).toBe(6);
      // detach op only
      const detach = payloadOf(stockFlowsOpFor(result.ops, 'old_sink'));
      expect(detach.inflows).toEqual([]);
      // no attach op (no stock target)
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(1);
    });

    it('drops the op for a stock missing from the variables map', () => {
      // The detaching stock's view element exists, but its Variable is absent
      // from the model map (e.g. mid-edit). stockFlowsOp returns undefined and
      // the op is dropped -- matching the original `if (stockVar?.type ===
      // 'stock')` guard. The attach op for the present stock still fires.
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const newSink = makeStockEl(4, 'new_sink', 200, 300);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, newSink, flow], 5);
      // old_sink intentionally omitted from the variables map.
      const variables = varsOf(makeStockVar('new_sink'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4 }));

      // attach op present, detach op dropped (no var for old_sink)
      expect(payloadOf(stockFlowsOpFor(result.ops, 'new_sink')).inflows).toEqual(['f']);
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(1);
    });

    it('cloud -> stock: deletes old cloud, attaches inflow', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldCloud = makeCloudEl(2, 3, 200, 100);
      const newSink = makeStockEl(4, 'new_sink', 200, 300);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldCloud, newSink, flow], 5);
      const variables = varsOf(makeStockVar('new_sink'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4 }));

      // old cloud (uid 2) deleted
      expect(result.elements.find((e) => e.uid === 2)).toBeUndefined();
      // attach inflow on new_sink, no detach (old end was a cloud)
      const attach = payloadOf(stockFlowsOpFor(result.ops, 'new_sink'));
      expect(attach.inflows).toEqual(['f']);
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(1);
    });

    it('cloud -> empty space: moves the cloud, emits no ops', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldCloud = makeCloudEl(2, 3, 200, 100);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldCloud, flow], 5);
      const variables = varsOf();

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, cursorMoveDelta: { x: -30, y: 0 } }),
      );

      // cloud preserved (updated), no new cloud, no ops
      expect(result.elements.filter((e) => e.type === 'cloud').length).toBe(1);
      expect(result.elements.find((e) => e.uid === 2)).toBeDefined();
      expect(result.ops.length).toBe(0);
      expect(result.nextUid).toBe(5);
    });
  });

  describe('source reattach (mirror cases, outflows)', () => {
    it('stock -> stock: detaches old outflow, attaches new outflow', () => {
      const oldSrc = makeStockEl(1, 'old_src', 0, 100, [], [3]);
      const newSrc = makeStockEl(4, 'new_src', 0, 300);
      const sinkStock = makeStockEl(2, 'sink', 200, 100);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([oldSrc, newSrc, sinkStock, flow], 5);
      const variables = varsOf(makeStockVar('old_src', [], ['f']), makeStockVar('new_src'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4, isSourceAttach: true }));

      const attach = payloadOf(stockFlowsOpFor(result.ops, 'new_src'));
      expect(attach.outflows).toEqual(['f']);
      const detach = payloadOf(stockFlowsOpFor(result.ops, 'old_src'));
      expect(detach.outflows).toEqual([]);
      const outFlow = result.elements.find((e) => e.uid === 3) as FlowViewElement;
      expect(outFlow.points[0].attachedToUid).toBe(4);
    });

    it('stock -> empty space: creates a cloud at release, detaches outflow', () => {
      const oldSrc = makeStockEl(1, 'old_src', 0, 100, [], [3]);
      const sinkStock = makeStockEl(2, 'sink', 200, 100);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([oldSrc, sinkStock, flow], 5);
      const variables = varsOf(makeStockVar('old_src', [], ['f']));

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, cursorMoveDelta: { x: 40, y: 0 }, isSourceAttach: true }),
      );

      expect(result.elements.filter((e) => e.type === 'cloud').length).toBe(1);
      const detach = payloadOf(stockFlowsOpFor(result.ops, 'old_src'));
      expect(detach.outflows).toEqual([]);
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(1);
    });

    it('cloud -> stock: deletes old cloud, attaches outflow', () => {
      const oldCloud = makeCloudEl(1, 3, 0, 100);
      const newSrc = makeStockEl(4, 'new_src', 0, 300);
      const sinkStock = makeStockEl(2, 'sink', 200, 100);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 0, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([oldCloud, newSrc, sinkStock, flow], 5);
      const variables = varsOf(makeStockVar('new_src'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4, isSourceAttach: true }));

      expect(result.elements.find((e) => e.uid === 1)).toBeUndefined();
      const attach = payloadOf(stockFlowsOpFor(result.ops, 'new_src'));
      expect(attach.outflows).toEqual(['f']);
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(1);
    });

    it('cloud -> empty space: moves the cloud, emits no ops', () => {
      const oldCloud = makeCloudEl(1, 3, 0, 100);
      const sinkStock = makeStockEl(2, 'sink', 200, 100);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 0, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([oldCloud, sinkStock, flow], 5);
      const variables = varsOf();

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, cursorMoveDelta: { x: 30, y: 0 }, isSourceAttach: true }),
      );

      expect(result.elements.find((e) => e.uid === 1)).toBeDefined();
      expect(result.ops.length).toBe(0);
      expect(result.nextUid).toBe(5);
    });
  });

  describe('creation', () => {
    it('cloud source to empty space: two new clouds, upsertFlow, selects new flow', () => {
      // in-creation flow: source attached to inCreationCloudUid, sink to faux cloud target
      const flow = makeFlowEl(inCreationUid, 'new_flow', 100, 100, [
        { x: 50, y: 100, attachedToUid: inCreationCloudUid },
        { x: 150, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      // The in-creation flow is a transient Canvas element, never part of the
      // persisted view, so it is NOT in view.elements.
      const view = makeView([], 5);
      const variables = varsOf();

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, fauxTargetCenter: { x: 150, y: 100 }, inCreation: true }),
      );

      expect(result.isCreatingNew).toBe(true);
      // upsertFlow present
      const upsert = result.ops.find((o) => o.type === 'upsertFlow');
      expect(upsert).toBeDefined();
      expect((upsert as { payload: { flow: { name: string } } }).payload.flow.name).toBe('new_flow');
      // two clouds materialized (source + sink)
      expect(result.elements.filter((e) => e.type === 'cloud').length).toBe(2);
      // selection is the realized flow uid (sentinel replaced by a real uid)
      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      expect(realFlow.uid).not.toBe(inCreationUid);
      expect(result.selection).toEqual(new Set([realFlow.uid]));
      // no stock ops (no stocks involved)
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(0);
    });

    it('stock source to target stock: upsertFlow + source outflow + sink inflow', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const sinkStock = makeStockEl(2, 'snk', 300, 100);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 150, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 280, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([srcStock, sinkStock], 5);
      const variables = varsOf(makeStockVar('src'), makeStockVar('snk'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 2, inCreation: true }));

      expect(result.isCreatingNew).toBe(true);
      expect(result.ops.find((o) => o.type === 'upsertFlow')).toBeDefined();
      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const flowIdent = realFlow.ident;
      // source stock gets an outflow
      const srcOp = payloadOf(stockFlowsOpFor(result.ops, 'src'));
      expect(srcOp.outflows).toEqual([flowIdent]);
      // sink stock gets an inflow
      const snkOp = payloadOf(stockFlowsOpFor(result.ops, 'snk'));
      expect(snkOp.inflows).toEqual([flowIdent]);
    });

    it('cloud source to target stock: sink pins to the stock EDGE, flow horizontal, valve on the segment', () => {
      // The exact UI scenario, faithful to runtime: the flow tool stages a
      // DEGENERATE in-creation flow (both points AND the valve at the press
      // point, since the drag offset is applied only at render time and never
      // committed back to the element); the user drags the sink onto a stock and
      // releases (targetUid = the stock). The sink must land on the stock's EDGE
      // (not its center -- the center put the arrowhead behind the stock, the
      // 71752f3b regression), the flow must be orthogonal, and the valve must sit
      // ON the source->sink segment (not at/behind the stock center).
      const sinkStock = makeStockEl(1, 'snk', 300, 100);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 50, 100, [
        { x: 50, y: 100, attachedToUid: inCreationCloudUid },
        { x: 50, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([sinkStock], 5);
      const variables = varsOf(makeStockVar('snk'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 1, inCreation: true }));

      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const sourcePt = realFlow.points[0];
      const sinkPt = realFlow.points[realFlow.points.length - 1];
      // the sink point attaches to the stock (uid 1), not a freshly-created cloud
      expect(sinkPt.attachedToUid).toBe(1);
      // ...landing on the stock's LEFT edge (center.x - StockWidth/2), NOT its
      // center: the source is to the left, so the flow enters the left face.
      expect(sinkPt.x).toBeCloseTo(300 - StockWidth / 2);
      expect(sinkPt.x).not.toBe(300);
      // ...and the flow is horizontal: the sink shares the source's y.
      expect(sinkPt.y).toBe(sourcePt.y);
      // a straight (2-point) orthogonal segment
      expect(realFlow.points.length).toBe(2);
      // the valve sits strictly between the source and the (edge-clipped) sink,
      // i.e. on the visible segment -- never at/behind the stock center.
      expect(realFlow.x).toBeGreaterThan(sourcePt.x);
      expect(realFlow.x).toBeLessThan(sinkPt.x);
      expect(realFlow.y).toBe(sourcePt.y);
      // only the source cloud is materialized; no sink cloud
      expect(result.elements.filter((e) => e.type === 'cloud').length).toBe(1);
      // the stock gains the flow as an inflow
      const snkOp = payloadOf(stockFlowsOpFor(result.ops, 'snk'));
      expect(snkOp.inflows).toEqual([realFlow.ident]);
    });

    it('cloud source above a target stock: sink pins to the TOP edge and the flow is vertical', () => {
      // Press above the stock, drag straight down onto it: the flow must route
      // vertically and pin to the stock's top edge (center.y - StockHeight/2).
      const sinkStock = makeStockEl(1, 'snk', 300, 200);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 300, 120, [
        { x: 300, y: 50, attachedToUid: inCreationCloudUid },
        { x: 300, y: 180, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([sinkStock], 5);
      const variables = varsOf(makeStockVar('snk'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 1, inCreation: true }));

      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const sourcePt = realFlow.points[0];
      const sinkPt = realFlow.points[realFlow.points.length - 1];
      expect(sinkPt.attachedToUid).toBe(1);
      expect(sinkPt.y).toBeCloseTo(200 - StockHeight / 2); // top edge, not center
      expect(sinkPt.y).not.toBe(200);
      expect(sinkPt.x).toBe(sourcePt.x); // vertical
    });

    it('degenerate creation onto a stock to the LEFT: edge-pinned, not degenerate', () => {
      // Faithful to runtime: the in-creation flow is degenerate (both points at
      // the press point). Releasing on a stock to the LEFT of the press must pin
      // the sink to the stock's RIGHT edge -- a signed axis comparison in
      // adjustFlows previously left this (and upward) degenerate on the commit
      // path, not just the preview.
      const sinkStock = makeStockEl(1, 'snk', 40, 200); // stock to the left of the press
      const flow = makeFlowEl(inCreationUid, 'new_flow', 100, 200, [
        { x: 100, y: 200, attachedToUid: inCreationCloudUid },
        { x: 100, y: 200, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([sinkStock], 5);
      const variables = varsOf(makeStockVar('snk'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 1, inCreation: true }));

      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const sourcePt = realFlow.points[0];
      const sinkPt = realFlow.points[realFlow.points.length - 1];
      expect(sinkPt.attachedToUid).toBe(1);
      expect(sinkPt.x).toBeCloseTo(40 + StockWidth / 2); // 62.5: right edge
      expect(sinkPt.x).not.toBe(100); // not degenerate / stuck at the press point
      expect(sinkPt.y).toBe(sourcePt.y); // horizontal
    });

    it('stock source: the source endpoint pins to the stock EDGE facing the sink, not its center', () => {
      // Faithful to runtime: pressing the flow tool ON a stock stages the
      // source point at the stock's CENTER, and only the sink is routed at
      // commit -- so the persisted source endpoint hid under the stock body
      // (violating the edge-attachment rule) until the next stock drag
      // re-pinned it. The commit must pin the source to the facing edge.
      const srcStock = makeStockEl(1, 'src', 100, 100);
      const sinkStock = makeStockEl(2, 'snk', 400, 100);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 100, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 100, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([srcStock, sinkStock], 5);
      const variables = varsOf(makeStockVar('src'), makeStockVar('snk'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 2, inCreation: true }));

      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const sourcePt = realFlow.points[0];
      const sinkPt = realFlow.points[realFlow.points.length - 1];
      // Source on the RIGHT edge of the source stock (the sink is to the right).
      expect(sourcePt.attachedToUid).toBe(1);
      expect(sourcePt.x).toBeCloseTo(100 + StockWidth / 2);
      expect(sourcePt.x).not.toBe(100);
      // Sink on the LEFT edge of the target stock; flow horizontal and straight.
      expect(sinkPt.x).toBeCloseTo(400 - StockWidth / 2);
      expect(sinkPt.y).toBe(sourcePt.y);
      expect(realFlow.points.length).toBe(2);
      // The valve sits on the visible segment between the two edges.
      expect(realFlow.x).toBeGreaterThan(sourcePt.x);
      expect(realFlow.x).toBeLessThan(sinkPt.x);
    });

    it('stock source dragged UP to empty space: the source pins to the TOP edge', () => {
      const srcStock = makeStockEl(1, 'src', 100, 300);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 100, 300, [
        { x: 100, y: 300, attachedToUid: 1 },
        { x: 100, y: 300, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([srcStock], 5);
      const variables = varsOf(makeStockVar('src'));

      // Release 150px above the press: the sink cloud materializes there.
      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, inCreation: true, fauxTargetCenter: { x: 100, y: 150 }, cursorMoveDelta: { x: 0, y: 150 } }),
      );

      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const sourcePt = realFlow.points[0];
      const sinkPt = realFlow.points[realFlow.points.length - 1];
      expect(sourcePt.attachedToUid).toBe(1);
      expect(sourcePt.y).toBeCloseTo(300 - StockHeight / 2); // top edge, not center
      expect(sourcePt.x).toBe(sinkPt.x); // vertical
    });

    it('stock source to empty space: faux target becomes a new cloud at fauxTargetCenter', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 150, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 280, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([srcStock], 5);
      const variables = varsOf(makeStockVar('src'));

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, fauxTargetCenter: { x: 280, y: 100 }, inCreation: true }),
      );

      // a cloud was created for the faux target
      const clouds = result.elements.filter((e) => e.type === 'cloud') as CloudViewElement[];
      expect(clouds.length).toBe(1);
      // source stock gets an outflow only (no sink stock)
      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const srcOp = payloadOf(stockFlowsOpFor(result.ops, 'src'));
      expect(srcOp.outflows).toEqual([realFlow.ident]);
      expect(result.ops.filter((o) => o.type === 'updateStockFlows').length).toBe(1);
    });
  });

  describe('nextUid monotonicity and element validity', () => {
    it('nextUid never decreases and all element uids are valid integers', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, flow], 5);
      const variables = varsOf(makeStockVar('old_sink', ['f']));

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, cursorMoveDelta: { x: -40, y: 0 } }),
      );

      expect(result.nextUid).toBeGreaterThanOrEqual(view.nextUid);
      for (const el of result.elements) {
        expect(Number.isInteger(el.uid)).toBe(true);
        expect(el.uid).toBeGreaterThanOrEqual(0);
        expect(el.uid).toBeLessThan(result.nextUid);
      }
    });
  });

  describe('op deduplication', () => {
    // The original emitted two byte-identical outflow-add ops only if
    // sourceStockIdent (creation path) AND sourceStockAttachingIdent (reattach
    // path) were both set for the same stock. Those two flags are mutually
    // exclusive in practice -- reattachEndpoint never runs during creation
    // (the in-creation flow isn't yet a view element), so sourceStockAttaching
    // is never set when sourceStockIdent is. The collapse is therefore a
    // defensive normalization. These tests pin the two observable guarantees:
    // (1) genuinely-distinct ops are NOT collapsed, and (2) the normal paths
    // never emit duplicates.
    it('does not collapse ops that differ only by list (outflow add vs inflow add)', () => {
      // Creation from a source stock to a distinct sink stock yields an outflow
      // add on src and an inflow add on snk. They share neither ident nor list,
      // so dedup must keep both -- a guard that the collapse keys on full op
      // content, not just on ident.
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const sinkStock = makeStockEl(2, 'snk', 300, 100);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 150, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 280, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([srcStock, sinkStock], 5);
      const variables = varsOf(makeStockVar('src'), makeStockVar('snk'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 2, inCreation: true }));
      const stockOps = result.ops.filter((o) => o.type === 'updateStockFlows');
      expect(stockOps.length).toBe(2);
      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      expect(payloadOf(stockFlowsOpFor(result.ops, 'src')).outflows).toEqual([realFlow.ident]);
      expect(payloadOf(stockFlowsOpFor(result.ops, 'snk')).inflows).toEqual([realFlow.ident]);
    });

    it('emits no duplicate ops for a plain reattach', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const newSink = makeStockEl(4, 'new_sink', 200, 300);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, newSink, flow], 5);
      const variables = varsOf(makeStockVar('old_sink', ['f']), makeStockVar('new_sink'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4 }));
      const serialized = result.ops.map((o) => JSON.stringify(o));
      expect(new Set(serialized).size).toBe(serialized.length);
    });
  });

  describe('error handling', () => {
    it('throws on unknown targetUid', () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, flow], 5);
      const variables = varsOf(makeStockVar('old_sink', ['f']));

      expect(() => computeFlowAttachment(view, variables, params({ flow, targetUid: 999 }))).toThrow('unknown uid 999');
    });

    it("throws when the snap target isn't a stock or cloud", () => {
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      // an aux is not a valid flow endpoint target
      const aux: ViewElement = {
        type: 'aux',
        uid: 7,
        name: 'a',
        ident: 'a',
        var: undefined,
        x: 200,
        y: 300,
        labelSide: 'center',
        isZeroRadius: false,
      };
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, aux, flow], 8);
      const variables = varsOf(makeStockVar('old_sink', ['f']));

      expect(() => computeFlowAttachment(view, variables, params({ flow, targetUid: 7 }))).toThrow(
        "new target isn't a stock or cloud",
      );
    });
  });

  // These tests assert the exact x/y of cloud placement and flow endpoints so
  // a future edit to the cloud-placement / UpdateCloudAndFlow math can't pass
  // silently. They use non-zero deltas where a sign flip would change the
  // result.
  describe('coordinate math', () => {
    it('(a) detach sink to empty space places the cloud at oldEnd - cursorMoveDelta', () => {
      // Sink stock at (200,100); release delta {-40,0} -> cloud at x = 200 - (-40) = 240.
      // A sign flip (200 + -40 = 160) would fail this assertion.
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const oldSink = makeStockEl(2, 'old_sink', 200, 100, [3]);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([srcStock, oldSink, flow], 5);
      const variables = varsOf(makeStockVar('old_sink', ['f']));

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, cursorMoveDelta: { x: -40, y: 0 } }),
      );

      const cloud = result.elements.find((e) => e.type === 'cloud') as CloudViewElement;
      expect(cloud.x).toBe(240);
      expect(cloud.y).toBe(100);
      // The flow's last point now attaches to that cloud at the same position.
      const outFlow = result.elements.find((e) => e.uid === 3) as FlowViewElement;
      const lastPt = outFlow.points[outFlow.points.length - 1];
      expect(lastPt.attachedToUid).toBe(cloud.uid);
      expect(lastPt.x).toBe(240);
      expect(lastPt.y).toBe(100);
    });

    it('(b) creation to empty space places the cloud at fauxTargetCenter', () => {
      // Source stock at (0,100); faux target center at (280,100), on the same
      // horizontal axis as the source point so the flow stays straight. With a
      // zero cursor delta the realized sink cloud lands exactly at the faux
      // center, and the flow's last point attaches to it there. A regression
      // that ignored fauxTargetCenter (e.g. placing the cloud at the source)
      // would put x at 20 instead of 280.
      const srcStock = makeStockEl(1, 'src', 0, 100);
      const flow = makeFlowEl(inCreationUid, 'new_flow', 150, 100, [
        { x: 20, y: 100, attachedToUid: 1 },
        { x: 280, y: 100, attachedToUid: fauxCloudTargetUid },
      ]);
      const view = makeView([srcStock], 5);
      const variables = varsOf(makeStockVar('src'));

      const result = computeFlowAttachment(
        view,
        variables,
        params({ flow, targetUid: 0, fauxTargetCenter: { x: 280, y: 100 }, inCreation: true }),
      );

      const cloud = result.elements.find((e) => e.type === 'cloud') as CloudViewElement;
      expect(cloud.x).toBe(280);
      expect(cloud.y).toBe(100);
      // The realized flow's last point attaches to that cloud, at the cloud.
      const realFlow = result.elements.find((e) => e.type === 'flow') as FlowViewElement;
      const lastPt = realFlow.points[realFlow.points.length - 1];
      expect(lastPt.attachedToUid).toBe(cloud.uid);
      expect(lastPt.x).toBe(280);
      expect(lastPt.y).toBe(100);
    });

    it('(c) cloud -> stock reattach reroutes the moved endpoint via UpdateCloudAndFlow', () => {
      // Flow source attached to a cloud at (0,100); reattach to a stock whose
      // center is (0,300). moveDelta = oldCloud - stock = (0-0, 100-300) =
      // (0,-200). The 200px perpendicular move converts the straight flow into
      // an L-shape: the source endpoint lands on the stock center (0,300), a
      // corner is introduced at (180,300), and the fixed sink stays at
      // (180,100). A sign error in the moveDelta math would move the endpoint
      // the wrong direction and break the asserted geometry.
      const oldCloud = makeCloudEl(1, 3, 0, 100);
      const newSrc = makeStockEl(4, 'new_src', 0, 300);
      const sinkStock = makeStockEl(2, 'sink', 200, 100);
      const flow = makeFlowEl(3, 'f', 100, 100, [
        { x: 0, y: 100, attachedToUid: 1 },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
      const view = makeView([oldCloud, newSrc, sinkStock, flow], 5);
      const variables = varsOf(makeStockVar('new_src'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4, isSourceAttach: true }));

      const outFlow = result.elements.find((e) => e.uid === 3) as FlowViewElement;
      expect(outFlow.points.map((p) => ({ x: p.x, y: p.y, attachedToUid: p.attachedToUid }))).toEqual([
        { x: 0, y: 300, attachedToUid: 4 },
        { x: 180, y: 300, attachedToUid: undefined },
        { x: 180, y: 100, attachedToUid: 2 },
      ]);
    });

    it('(d) L-shaped (3-point) flow reattach reroutes via UpdateCloudAndFlow', () => {
      // 3-point L-shaped flow: source cloud at (0,100) -> corner (0,300) ->
      // sink stock at (200,300). Reattach the SOURCE cloud endpoint to a stock
      // at (0,500), exercising UpdateCloudAndFlow's multi-segment reroute path.
      // The cloud-adjacent vertical segment keeps x=0, so the source endpoint
      // slides down to the new stock center (0,500) while the corner (0,300)
      // and the fixed sink (180,300) are preserved.
      const oldCloud = makeCloudEl(1, 3, 0, 100);
      const newSrc = makeStockEl(4, 'new_src', 0, 500);
      const sinkStock = makeStockEl(2, 'sink', 200, 300);
      const flow = makeFlowEl(3, 'f', 0, 200, [
        { x: 0, y: 100, attachedToUid: 1 },
        { x: 0, y: 300, attachedToUid: undefined },
        { x: 180, y: 300, attachedToUid: 2 },
      ]);
      const view = makeView([oldCloud, newSrc, sinkStock, flow], 5);
      const variables = varsOf(makeStockVar('new_src'));

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 4, isSourceAttach: true }));

      const outFlow = result.elements.find((e) => e.uid === 3) as FlowViewElement;
      // Source endpoint now attaches to the new stock; the old cloud is gone.
      expect(result.elements.find((e) => e.uid === 1)).toBeUndefined();
      expect(outFlow.points.map((p) => ({ x: p.x, y: p.y, attachedToUid: p.attachedToUid }))).toEqual([
        { x: 0, y: 500, attachedToUid: 4 },
        { x: 0, y: 300, attachedToUid: undefined },
        { x: 180, y: 300, attachedToUid: 2 },
      ]);
    });
  });
});

describe('growInCreationFlow (live drag preview)', () => {
  // The in-creation flow is degenerate while dragging: both points sit at the
  // press point, and the drag is recorded only as moveDelta (= press - cursor).
  function degenerateFlow(px: number, py: number): FlowViewElement {
    return makeFlowEl(inCreationUid, 'new_flow', px, py, [
      { x: px, y: py, attachedToUid: inCreationCloudUid },
      { x: px, y: py, attachedToUid: fauxCloudTargetUid },
    ]);
  }

  it('over empty space: the sink follows the cursor along a horizontal segment', () => {
    const flow = degenerateFlow(100, 200);
    // cursor 80px to the right of the press point -> moveDelta = press - cursor.
    const grown = growInCreationFlow(flow, { x: -80, y: 0 }, undefined);

    expect(grown.points.length).toBe(2);
    const src = grown.points[0];
    const sink = grown.points[grown.points.length - 1];
    // source stays at the press point
    expect(src.x).toBe(100);
    expect(src.y).toBe(200);
    // sink reaches the cursor; flow is horizontal (sink shares source y)
    expect(sink.x).toBeCloseTo(180);
    expect(sink.y).toBe(200);
    // the sink still references the faux cursor target during the preview
    expect(sink.attachedToUid).toBe(fauxCloudTargetUid);
  });

  it('over empty space: a dominant vertical drag grows a vertical segment', () => {
    const flow = degenerateFlow(100, 200);
    // cursor 80px below -> dominant y component picks the vertical axis.
    const grown = growInCreationFlow(flow, { x: 0, y: -80 }, undefined);

    const src = grown.points[0];
    const sink = grown.points[grown.points.length - 1];
    expect(sink.x).toBe(src.x); // vertical
    expect(sink.y).toBeCloseTo(280);
  });

  it('over a stock: the sink snaps to the stock EDGE (not the cursor or center)', () => {
    const flow = degenerateFlow(100, 200);
    const stock = makeStockEl(1, 'snk', 300, 200);
    // moveDelta is irrelevant once a target is hovered; the sink pins to the edge.
    const grown = growInCreationFlow(flow, { x: -200, y: 0 }, stock);

    const src = grown.points[0];
    const sink = grown.points[grown.points.length - 1];
    expect(sink.attachedToUid).toBe(1);
    expect(sink.x).toBeCloseTo(300 - StockWidth / 2); // left edge, not center (300)
    expect(sink.x).not.toBe(300);
    expect(sink.y).toBe(src.y); // horizontal
    expect(src.x).toBe(100); // source unchanged
  });

  // The degenerate axis-recovery in adjustFlows must work in BOTH directions on
  // each axis; a signed comparison there silently left LEFT/UP drags degenerate.
  it('over empty space: the sink follows a LEFTWARD cursor (not degenerate)', () => {
    const flow = degenerateFlow(100, 200);
    // cursor 80px to the LEFT -> moveDelta = press - cursor = +80.
    const grown = growInCreationFlow(flow, { x: 80, y: 0 }, undefined);

    const src = grown.points[0];
    const sink = grown.points[grown.points.length - 1];
    expect(sink.x).toBeCloseTo(20); // reached the cursor to the left
    expect(sink.x).not.toBe(100); // not stuck at the press point
    expect(sink.y).toBe(src.y); // horizontal
  });

  it('over empty space: the sink follows an UPWARD cursor (not degenerate)', () => {
    const flow = degenerateFlow(100, 200);
    // cursor 80px ABOVE -> moveDelta = press - cursor = +80 in y.
    const grown = growInCreationFlow(flow, { x: 0, y: 80 }, undefined);

    const src = grown.points[0];
    const sink = grown.points[grown.points.length - 1];
    expect(sink.x).toBe(src.x); // vertical
    expect(sink.y).toBeCloseTo(120); // reached the cursor above
    expect(sink.y).not.toBe(200); // not stuck at the press point
  });

  it('over a stock to the LEFT: the sink snaps to the stock RIGHT edge', () => {
    const flow = degenerateFlow(100, 200);
    const stock = makeStockEl(1, 'snk', 40, 200); // stock to the left of the press
    const grown = growInCreationFlow(flow, { x: 0, y: 0 }, stock);

    const sink = grown.points[grown.points.length - 1];
    expect(sink.attachedToUid).toBe(1);
    expect(sink.x).toBeCloseTo(40 + StockWidth / 2); // 62.5: right edge, not the center
    expect(sink.x).not.toBe(100); // not stuck at the press point
    expect(sink.y).toBe(200);
  });

  it('over a stock ABOVE: the sink snaps to the stock BOTTOM edge', () => {
    const flow = degenerateFlow(100, 200);
    const stock = makeStockEl(1, 'snk', 100, 40); // stock above the press
    const grown = growInCreationFlow(flow, { x: 0, y: 0 }, stock);

    const sink = grown.points[grown.points.length - 1];
    expect(sink.attachedToUid).toBe(1);
    expect(sink.y).toBeCloseTo(40 + StockHeight / 2); // bottom edge, not the center
    expect(sink.y).not.toBe(200); // not stuck at the press point
    expect(sink.x).toBe(100);
  });
});

describe('growEndpointDrag (existing cloud endpoint drag)', () => {
  // A straight, already-attached flow: stock (uid 1) on the left, cloud (uid 2)
  // on the right, valve in the middle. This is the geometry the reported bug is
  // about -- dragging the cloud along the flow axis previously moved only the
  // valve, leaving the path/arrowhead stale.
  function stockToCloudFlow(): FlowViewElement {
    return makeFlowEl(10, 'f', 200, 200, [
      { x: 122.5, y: 200, attachedToUid: 1 }, // stock right edge (100 + StockWidth/2)
      { x: 300, y: 200, attachedToUid: 2 }, // cloud center
    ]);
  }
  // The mirror: cloud (uid 2) on the left is the SOURCE, stock (uid 1) the sink.
  function cloudToStockFlow(): FlowViewElement {
    return makeFlowEl(10, 'f', 200, 200, [
      { x: 100, y: 200, attachedToUid: 2 }, // cloud center (source)
      { x: 277.5, y: 200, attachedToUid: 1 }, // stock left edge (300 - StockWidth/2)
    ]);
  }

  it('sink cloud, along-axis drag: the sink endpoint follows the cursor (not just the valve)', () => {
    const flow = stockToCloudFlow();
    // cursor 50px further right -> moveDelta = press - cursor = -50 in x.
    const grown = growEndpointDrag(flow, false, { x: -50, y: 0 }, undefined);

    expect(grown.points.length).toBe(2);
    const src = grown.points[0];
    const sink = grown.points[1];
    // sink tracked the cursor; flow stayed horizontal; source fixed on its edge
    expect(sink.x).toBeCloseTo(350);
    expect(sink.y).toBe(200);
    expect(sink.attachedToUid).toBe(2);
    expect(src.x).toBeCloseTo(122.5);
    expect(src.y).toBe(200);
    // the valve stayed between the two ends (no longer stuck lagging behind)
    expect(grown.x).toBeGreaterThan(src.x);
    expect(grown.x).toBeLessThan(sink.x);
  });

  it('sink cloud, along-axis drag TOWARD the source: the sink still tracks the cursor', () => {
    const flow = stockToCloudFlow();
    // cursor 120px to the LEFT -> moveDelta = press - cursor = +120 in x.
    const grown = growEndpointDrag(flow, false, { x: 120, y: 0 }, undefined);

    const sink = grown.points[grown.points.length - 1];
    expect(sink.x).toBeCloseTo(180);
    expect(sink.x).not.toBeCloseTo(300); // not stuck at the original cloud position
    expect(sink.y).toBe(200);
  });

  it('source cloud, along-axis drag: the source endpoint follows the cursor', () => {
    const flow = cloudToStockFlow();
    // cursor 50px to the LEFT -> moveDelta = press - cursor = +50 in x.
    const grown = growEndpointDrag(flow, true, { x: 50, y: 0 }, undefined);

    const src = grown.points[0];
    const sink = grown.points[grown.points.length - 1];
    expect(src.x).toBeCloseTo(50); // source cloud followed the cursor left
    expect(src.x).not.toBeCloseTo(100); // not stuck at the original position
    expect(src.y).toBe(200);
    expect(sink.x).toBeCloseTo(277.5); // stock endpoint fixed on its edge
    expect(sink.attachedToUid).toBe(1);
  });

  it('vertical flow, sink cloud along-axis drag: the sink tracks the cursor vertically', () => {
    const flow = makeFlowEl(10, 'f', 200, 200, [
      { x: 200, y: 122.5, attachedToUid: 1 }, // stock bottom edge
      { x: 200, y: 300, attachedToUid: 2 }, // cloud below
    ]);
    // cursor 40px further down -> moveDelta = press - cursor = -40 in y.
    const grown = growEndpointDrag(flow, false, { x: 0, y: -40 }, undefined);

    const sink = grown.points[grown.points.length - 1];
    expect(sink.y).toBeCloseTo(340);
    expect(sink.x).toBe(200); // stayed vertical
  });

  it('sink cloud, perpendicular drag: bends into an L and the cloud moves perpendicular', () => {
    const flow = stockToCloudFlow();
    // cursor 60px DOWN -> moveDelta = press - cursor = -60 in y (perpendicular).
    const grown = growEndpointDrag(flow, false, { x: 0, y: -60 }, undefined);

    expect(grown.points.length).toBe(3);
    const sink = grown.points[grown.points.length - 1];
    expect(sink.y).toBeCloseTo(260); // cloud descended with the drag
    expect(sink.attachedToUid).toBe(2);
  });

  it('sink cloud dragged onto a stock: the sink pins to the stock EDGE (matches the commit)', () => {
    const flow = stockToCloudFlow();
    const target = makeStockEl(3, 'snk', 500, 200);
    const grown = growEndpointDrag(flow, false, { x: -200, y: 0 }, target);

    const sink = grown.points[grown.points.length - 1];
    expect(sink.attachedToUid).toBe(3);
    expect(sink.x).toBeCloseTo(500 - StockWidth / 2); // left edge, not the center
    expect(sink.x).not.toBe(500);
    expect(sink.y).toBe(200); // horizontal
  });

  it('source cloud dragged onto a stock: the source pins to the stock EDGE', () => {
    const flow = cloudToStockFlow();
    const target = makeStockEl(3, 'src', 0, 200); // stock to the left
    const grown = growEndpointDrag(flow, true, { x: 100, y: 0 }, target);

    const src = grown.points[0];
    expect(src.attachedToUid).toBe(3);
    expect(src.x).toBeCloseTo(0 + StockWidth / 2); // right edge of the left-hand stock
    expect(src.x).not.toBe(0);
    expect(src.y).toBe(200);
  });
});
