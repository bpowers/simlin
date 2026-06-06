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
  inCreationCloudUid,
  inCreationUid,
  type FlowAttachParams,
} from '../flow-attach';
import {
  fauxCloudTargetUid as canvasFauxCloudTargetUid,
  inCreationCloudUid as canvasInCreationCloudUid,
  inCreationUid as canvasInCreationUid,
} from '../drawing/Canvas';

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
    (op) =>
      op.type === 'updateStockFlows' &&
      (op as { payload: { ident: string } }).payload.ident === ident,
  );
  expect(matches.length).toBe(1);
  return matches[0];
}

function payloadOf(op: JsonModelOperation): { ident: string; inflows: string[]; outflows: string[] } {
  return (op as unknown as { payload: { ident: string; inflows: string[]; outflows: string[] } }).payload;
}

describe('computeFlowAttachment', () => {
  // The flow-attach module re-declares the Canvas creation sentinels to stay
  // free of React/DOM imports. Guard that the duplicates never drift.
  it('keeps creation sentinel constants in sync with Canvas', () => {
    expect(inCreationUid).toBe(canvasInCreationUid);
    expect(inCreationCloudUid).toBe(canvasInCreationCloudUid);
    expect(fauxCloudTargetUid).toBe(canvasFauxCloudTargetUid);
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

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 0, cursorMoveDelta: { x: -40, y: 0 } }));

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

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 0, cursorMoveDelta: { x: -30, y: 0 } }));

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

      const result = computeFlowAttachment(view, variables, params({ flow, targetUid: 0, cursorMoveDelta: { x: -40, y: 0 } }));

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

      expect(() => computeFlowAttachment(view, variables, params({ flow, targetUid: 999 }))).toThrow(
        'unknown uid 999',
      );
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
});
