// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// buildEditOps over the enumerations the plan names: flow end (source, sink) x
// the attachment the end has before the edit (cloud, existing stock, stock
// element with no variable, stock element naming a non-stock) x the one it has
// after (cloud, a different existing stock, a stock created by the same edit,
// the two degenerate stock elements); deleting a stock with a flow attached as
// source, sink, or both; deleting a stock together with one of its flows; a
// flow listed in two stocks' lists; renames with and without a move; creates
// and deletes of each variable kind; op order.
//
// How the inputs are built, and why that is production's input: the base view
// and committed model come from the engine's own serialization through the
// production loader (projectFromJson), exactly as the controller rebuilds
// `committed`. A next view is that serialized view JSON with attachments
// rewritten and loaded through stockFlowViewFromJson. buildEditOps reads only
// element uids, names, types and flow endpoint attachments, so a next view that
// differs from base in those fields is the input production's gestures supply
// (Phase 4's gestures still compute geometry with the old routing; the geometry
// fields they change are not read here). Deletes are planned by the production
// planDelete.
//
// Every row is checked twice: the op payloads against the row's expectation
// (derived from the row's axes, not hand-listed per row), and -- through the
// real engine -- the committed checkers M1/M2/M3 on the model the patch
// produces. The engine rows skip on a checkout without libsimlin.wasm and run
// under CI (support/engine.ts).

import { describe, it, expect, beforeAll } from '@rstest/core';

import { canonicalize } from '@simlin/core/canonicalize';
import { projectFromJson, stockFlowViewFromJson, type Model, type StockFlowView } from '@simlin/core/datamodel';
import type { JsonModelOperation, JsonProject } from '@simlin/engine';

import { buildEditOps, EditConflictError } from '../view-model-sync';
import { planDelete } from '../plan-delete';
import { describeWithEngine, editorModel, loadEngine, type EngineModule } from './support/engine';
import {
  checkCreatedVariables,
  checkKindAgreement,
  checkReferentialIntegrity,
  checkStockFlowDelta,
  checkStockListDuplicates,
  formatViewViolations,
} from './support/view-invariants';

// ---------------------------------------------------------------------------
// Fixture

// JSON view elements are plain objects on the wire.
type JsonElement = Record<string, unknown> & { type: string; uid: number };
interface JsonModelLike {
  name: string;
  stocks: Array<{ name: string; initialEquation: string; inflows: string[]; outflows: string[] }>;
  flows: Array<{ name: string; equation: string }>;
  auxiliaries: Array<{ name: string; equation: string }>;
  modules?: Array<Record<string, unknown>>;
  views: Array<{ elements: JsonElement[] }>;
}

const UID = {
  stockA: 1,
  stockB: 2,
  stockC: 3,
  flowF: 4,
  flowG: 5,
  flowH: 6,
  flowK: 7,
  auxX: 8,
  linkAtoX: 9,
  aliasA: 10,
  linkAliasToK: 11,
  ghostStock: 12,
  wrongKindStock: 13,
  cloudG: 20,
  cloudH: 21,
  cloudKSource: 22,
  cloudKSink: 23,
  // Uids the edits allocate.
  createdStock: 30,
  createdFlow: 31,
  createdAux: 32,
  createdModule: 33,
  newCloud: 40,
} as const;

function baseModelJson(): JsonModelLike {
  return {
    name: 'main',
    stocks: [
      { name: 'Stock A', initialEquation: '10', inflows: ['Flow G'], outflows: ['Flow F'] },
      { name: 'Stock B', initialEquation: '0', inflows: ['Flow F'], outflows: ['Flow H'] },
      { name: 'Stock C', initialEquation: '0', inflows: [], outflows: [] },
    ],
    flows: [
      { name: 'Flow F', equation: '1' },
      { name: 'Flow G', equation: '1' },
      { name: 'Flow H', equation: '1' },
      { name: 'Flow K', equation: '1' },
    ],
    auxiliaries: [
      { name: 'Aux X', equation: 'Stock A * 0.1' },
      { name: 'Aux W', equation: '2' },
    ],
    views: [
      {
        elements: [
          { type: 'stock', uid: UID.stockA, name: 'Stock A', x: 100, y: 100 },
          { type: 'stock', uid: UID.stockB, name: 'Stock B', x: 300, y: 100 },
          { type: 'stock', uid: UID.stockC, name: 'Stock C', x: 300, y: 300 },
          {
            type: 'flow',
            uid: UID.flowF,
            name: 'Flow F',
            x: 200,
            y: 100,
            points: [
              { x: 122.5, y: 100, attachedToUid: UID.stockA },
              { x: 277.5, y: 100, attachedToUid: UID.stockB },
            ],
          },
          { type: 'cloud', uid: UID.cloudG, flowUid: UID.flowG, x: 0, y: 100 },
          {
            type: 'flow',
            uid: UID.flowG,
            name: 'Flow G',
            x: 40,
            y: 100,
            points: [
              { x: 0, y: 100, attachedToUid: UID.cloudG },
              { x: 77.5, y: 100, attachedToUid: UID.stockA },
            ],
          },
          {
            type: 'flow',
            uid: UID.flowH,
            name: 'Flow H',
            x: 400,
            y: 100,
            points: [
              { x: 322.5, y: 100, attachedToUid: UID.stockB },
              { x: 500, y: 100, attachedToUid: UID.cloudH },
            ],
          },
          { type: 'cloud', uid: UID.cloudH, flowUid: UID.flowH, x: 500, y: 100 },
          { type: 'cloud', uid: UID.cloudKSource, flowUid: UID.flowK, x: 0, y: 500 },
          {
            type: 'flow',
            uid: UID.flowK,
            name: 'Flow K',
            x: 100,
            y: 500,
            points: [
              { x: 0, y: 500, attachedToUid: UID.cloudKSource },
              { x: 200, y: 500, attachedToUid: UID.cloudKSink },
            ],
          },
          { type: 'cloud', uid: UID.cloudKSink, flowUid: UID.flowK, x: 200, y: 500 },
          { type: 'aux', uid: UID.auxX, name: 'Aux X', x: 200, y: 20 },
          { type: 'link', uid: UID.linkAtoX, fromUid: UID.stockA, toUid: UID.auxX },
          { type: 'alias', uid: UID.aliasA, aliasOfUid: UID.stockA, x: 100, y: 250 },
          { type: 'link', uid: UID.linkAliasToK, fromUid: UID.aliasA, toUid: UID.flowK },
          // A stock element whose variable does not exist, and one naming an aux:
          // imported shapes the editor must accept without touching any list.
          { type: 'stock', uid: UID.ghostStock, name: 'Ghost', x: 500, y: 300 },
          { type: 'stock', uid: UID.wrongKindStock, name: 'Aux W', x: 600, y: 300 },
        ],
      },
    ],
  };
}

function projectJsonOf(model: JsonModelLike): JsonProject {
  return {
    name: 'view-model-sync',
    simSpecs: { startTime: 0, endTime: 1, dt: '1' },
    models: [model],
  } as unknown as JsonProject;
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function elementByUid(model: JsonModelLike, uid: number): JsonElement {
  const el = model.views[0].elements.find((e) => e.uid === uid);
  if (el === undefined) {
    throw new Error(`fixture has no uid ${uid}`);
  }
  return el;
}

type End = 'source' | 'sink';

// Point `flowUid`'s `end` at `target` (a stock uid) or at a fresh cloud,
// removing the cloud it was attached to, if any.
function setEnd(model: JsonModelLike, flowUid: number, end: End, target: number | { cloud: number }): void {
  const flow = elementByUid(model, flowUid) as JsonElement & {
    points: Array<{ x: number; y: number; attachedToUid?: number }>;
  };
  const point = end === 'source' ? flow.points[0] : flow.points[flow.points.length - 1];
  const old = model.views[0].elements.find((e) => e.uid === point.attachedToUid);
  if (old?.type === 'cloud') {
    model.views[0].elements = model.views[0].elements.filter((e) => e.uid !== old.uid);
  }
  if (typeof target === 'number') {
    point.attachedToUid = target;
  } else {
    model.views[0].elements.push({ type: 'cloud', uid: target.cloud, flowUid, x: point.x, y: point.y });
    point.attachedToUid = target.cloud;
  }
}

function listOf(model: JsonModelLike, stockName: string, end: End): string[] {
  const stock = model.stocks.find((s) => s.name === stockName);
  if (stock === undefined) {
    throw new Error(`fixture has no stock ${stockName}`);
  }
  return end === 'source' ? stock.outflows : stock.inflows;
}

function addStockElement(model: JsonModelLike, uid: number, name: string): void {
  model.views[0].elements.push({ type: 'stock', uid, name, x: 700, y: 500 });
}

// ---------------------------------------------------------------------------
// Running an edit

interface Edit {
  /** Adjust the authored model before it is opened (the committed state). */
  readonly setup?: (model: JsonModelLike) => void;
  /** Rewrite the committed view JSON into the next view. */
  readonly next: (view: JsonModelLike) => void;
  /** Build the next view through planDelete instead (a delete edit). */
  readonly deleteSelection?: readonly number[];
}

interface Planned {
  readonly committed: Model;
  readonly base: StockFlowView;
  readonly next: StockFlowView;
}

// Rewrites the serialized committed model's view, never the authored fixture:
// the base is what the engine (or loader) actually returned.
function planFrom(committedJson: JsonProject, edit: Edit): Planned {
  const committed = projectFromJson(committedJson).models.get('main')!;
  const base = committed.views[0];
  if (edit.deleteSelection !== undefined) {
    return { committed, base, next: planDelete(base, new Set(edit.deleteSelection)) };
  }
  const modelJson = clone(committedJson.models.find((m) => m.name === 'main')) as unknown as JsonModelLike;
  edit.next(modelJson);
  const next = stockFlowViewFromJson(modelJson.views[0] as never, committed.variables);
  return { committed, base, next };
}

function authoredProject(edit: Edit): JsonProject {
  const model = baseModelJson();
  edit.setup?.(model);
  return projectJsonOf(model);
}

type StockLists = { inflows: string[]; outflows: string[] };

function stockOpsOf(ops: readonly JsonModelOperation[]): Map<string, StockLists> {
  const out = new Map<string, StockLists>();
  for (const op of ops) {
    if (op.type === 'updateStockFlows') {
      out.set(op.payload.ident, {
        inflows: op.payload.inflows.map(canonicalize).sort(),
        outflows: op.payload.outflows.map(canonicalize).sort(),
      });
    }
  }
  return out;
}

function opTypes(ops: readonly JsonModelOperation[]): string[] {
  return ops.map((op) => op.type);
}

// ---------------------------------------------------------------------------
// Attachment rows

type FromKind = 'cloud' | 'existing' | 'ghost' | 'wrongKind';
type ToKind = 'cloud' | 'existing' | 'created' | 'ghost' | 'wrongKind';

const FROM_KINDS: readonly FromKind[] = ['cloud', 'existing', 'ghost', 'wrongKind'];
const TO_KINDS: readonly ToKind[] = ['cloud', 'existing', 'created', 'ghost', 'wrongKind'];

interface AttachRow {
  readonly end: End;
  readonly from: FromKind;
  readonly to: ToKind;
}

// Every (end, from, to) except the pairs that are not an attachment change:
// cloud -> cloud (the end stays on its own cloud), and a degenerate element to
// the same element. "existing -> existing" moves between two different stocks
// (Stock A -> Stock C); "created" is only a target, since a base view cannot
// contain an element the edit creates.
const ATTACH_ROWS: readonly AttachRow[] = (['source', 'sink'] as const).flatMap((end) =>
  FROM_KINDS.flatMap((from) =>
    TO_KINDS.filter((to) => !(from === to && from !== 'existing')).map((to) => ({ end, from, to })),
  ),
);

const FROM_STOCK = { existing: 'Stock A', ghost: 'Ghost', wrongKind: 'Aux W' } as const;
const FROM_UID = { existing: UID.stockA, ghost: UID.ghostStock, wrongKind: UID.wrongKindStock } as const;
const TO_UID = {
  existing: UID.stockC,
  created: UID.createdStock,
  ghost: UID.ghostStock,
  wrongKind: UID.wrongKindStock,
} as const;
const TO_IDENT = { existing: 'stock_c', created: 'stock_new' } as const;

// Flow K starts cloud -> cloud; the row moves one of its ends.
function attachEdit(row: AttachRow): Edit {
  return {
    setup: (model) => {
      if (row.from === 'cloud') {
        return;
      }
      setEnd(model, UID.flowK, row.end, FROM_UID[row.from]);
      if (row.from === 'existing') {
        // A well-formed committed model lists what the view attaches.
        listOf(model, FROM_STOCK.existing, row.end).push('Flow K');
      }
    },
    next: (model) => {
      if (row.to === 'cloud') {
        setEnd(model, UID.flowK, row.end, { cloud: UID.newCloud });
        return;
      }
      if (row.to === 'created') {
        addStockElement(model, UID.createdStock, 'Stock New');
      }
      setEnd(model, UID.flowK, row.end, TO_UID[row.to]);
    },
  };
}

// The expected stock ops for a row, from its axes alone: the flow leaves an
// existing old stock and joins an existing or created new one; degenerate
// elements and clouds get no op.
function expectedAttachStockOps(row: AttachRow): Map<string, StockLists> {
  const baseLists: Record<string, StockLists> = {
    stock_a: { inflows: ['flow_g'], outflows: ['flow_f'] },
    stock_c: { inflows: [], outflows: [] },
    stock_new: { inflows: [], outflows: [] },
  };
  const listName = row.end === 'source' ? 'outflows' : 'inflows';
  const out = new Map<string, StockLists>();
  if (row.from === 'existing') {
    // The base listed Flow K there; the op removes it and echoes the rest.
    out.set('stock_a', { ...baseLists.stock_a });
  }
  if (row.to === 'existing' || row.to === 'created') {
    const ident = TO_IDENT[row.to];
    const lists = { inflows: [...baseLists[ident].inflows], outflows: [...baseLists[ident].outflows] };
    lists[listName] = [...lists[listName], 'flow_k'].sort();
    out.set(ident, lists);
  }
  return out;
}

function attachRowName(row: AttachRow): string {
  return `${row.end}: ${row.from} -> ${row.to}`;
}

// ---------------------------------------------------------------------------
// Other rows: each carries its own edit and expected ops.

interface ScenarioRow {
  readonly name: string;
  readonly edit: Edit;
  readonly expectOps: (ops: readonly JsonModelOperation[]) => void;
  /**
   * Uids whose M1 violations the committed model carries by construction
   * (the degenerate stock elements), excluded from the kind check.
   */
  readonly degenerateUids?: readonly number[];
  /** Why M2 is not checked, for the one row whose engine semantics the checker predates. */
  readonly skipM2?: string;
}

const DEGENERATE = [UID.ghostStock, UID.wrongKindStock];

const SCENARIO_ROWS: readonly ScenarioRow[] = [
  {
    // Delete stock x flow attached as sink: Stock B with only Flow F (sink) on it.
    name: 'delete a stock a flow ends on (sink)',
    edit: {
      setup: (model) => {
        setEnd(model, UID.flowH, 'source', { cloud: 41 });
        model.stocks.find((s) => s.name === 'Stock B')!.outflows = [];
      },
      next: () => {},
      deleteSelection: [UID.stockB],
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['deleteVariable', 'upsertView']);
      expect(ops[0]).toEqual({ type: 'deleteVariable', payload: { ident: 'stock_b' } });
    },
  },
  {
    name: 'delete a stock a flow starts on (source)',
    edit: {
      setup: (model) => {
        setEnd(model, UID.flowF, 'sink', { cloud: 41 });
        model.stocks.find((s) => s.name === 'Stock B')!.inflows = [];
      },
      next: () => {},
      deleteSelection: [UID.stockB],
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['deleteVariable', 'upsertView']);
    },
  },
  {
    name: 'delete a stock a flow both starts and ends on',
    edit: {
      setup: (model) => {
        setEnd(model, UID.flowK, 'source', UID.stockC);
        setEnd(model, UID.flowK, 'sink', UID.stockC);
        listOf(model, 'Stock C', 'source').push('Flow K');
        listOf(model, 'Stock C', 'sink').push('Flow K');
      },
      next: () => {},
      deleteSelection: [UID.stockC],
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['deleteVariable', 'upsertView']);
    },
  },
  {
    // Flow F runs A -> B; deleting B with F takes F out of A's outflows through
    // the engine's deleteVariable, so no updateStockFlows is emitted at all.
    name: 'delete a stock together with one of its flows',
    edit: { next: () => {}, deleteSelection: [UID.stockB, UID.flowF] },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['deleteVariable', 'deleteVariable', 'upsertView']);
      expect(ops.slice(0, 2).map((op) => (op.payload as { ident: string }).ident)).toEqual(['stock_b', 'flow_f']);
    },
  },
  {
    // Deleting Stock A removes its alias and the links touching A or the alias.
    name: 'delete a stock with an alias and links',
    edit: { next: () => {}, deleteSelection: [UID.stockA] },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['deleteVariable', 'upsertView']);
    },
  },
  {
    // An XMILE import can list one flow as an outflow of two stocks. Moving an
    // unrelated flow onto one of them echoes the stale entry untouched.
    name: 'a flow listed in two stocks stays untouched when another flow attaches there',
    edit: {
      setup: (model) => {
        listOf(model, 'Stock C', 'source').push('Flow F');
      },
      next: (model) => setEnd(model, UID.flowK, 'sink', UID.stockC),
    },
    expectOps: (ops) => {
      expect(stockOpsOf(ops)).toEqual(new Map([['stock_c', { inflows: ['flow_k'], outflows: ['flow_f'] }]]));
    },
  },
  {
    name: 'a flow listed in two stocks: moving its attached end leaves the stale list alone',
    edit: {
      setup: (model) => {
        listOf(model, 'Stock C', 'source').push('Flow F');
      },
      next: (model) => setEnd(model, UID.flowF, 'source', UID.stockB),
    },
    expectOps: (ops) => {
      // Flow F leaves A's outflows and joins B's; C's stale entry gets no op.
      expect(stockOpsOf(ops)).toEqual(
        new Map([
          ['stock_a', { inflows: ['flow_g'], outflows: [] }],
          ['stock_b', { inflows: ['flow_f'], outflows: ['flow_f', 'flow_h'] }],
        ]),
      );
    },
  },
  {
    name: 'rename a flow',
    edit: {
      next: (model) => {
        elementByUid(model, UID.flowF).name = 'Flow Renamed';
      },
    },
    expectOps: (ops) => {
      expect(ops[0]).toEqual({ type: 'renameVariable', payload: { from: 'flow_f', to: 'Flow Renamed' } });
      expect(opTypes(ops)).toEqual(['renameVariable', 'upsertView']);
    },
  },
  {
    name: 'rename a flow and move its sink',
    edit: {
      next: (model) => {
        elementByUid(model, UID.flowF).name = 'Flow Renamed';
        setEnd(model, UID.flowF, 'sink', UID.stockC);
      },
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['renameVariable', 'updateStockFlows', 'updateStockFlows', 'upsertView']);
      // B's inflow is carried through the rename, then removed; C gains the new ident.
      expect(stockOpsOf(ops)).toEqual(
        new Map([
          ['stock_b', { inflows: [], outflows: ['flow_h'] }],
          ['stock_c', { inflows: ['flow_renamed'], outflows: [] }],
        ]),
      );
    },
  },
  {
    // A rename of an attached stock's element changes no list entry.
    name: 'rename a stock',
    edit: {
      next: (model) => {
        elementByUid(model, UID.stockA).name = 'Stock Renamed';
      },
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['renameVariable', 'upsertView']);
    },
  },
  {
    name: 'create a flow between two existing stocks',
    edit: {
      next: (model) => {
        model.views[0].elements.push({
          type: 'flow',
          uid: UID.createdFlow,
          name: 'Flow New',
          x: 200,
          y: 200,
          points: [
            { x: 100, y: 122.5, attachedToUid: UID.stockA },
            { x: 300, y: 277.5, attachedToUid: UID.stockC },
          ],
        });
      },
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['upsertFlow', 'updateStockFlows', 'updateStockFlows', 'upsertView']);
      expect(stockOpsOf(ops)).toEqual(
        new Map([
          ['stock_a', { inflows: ['flow_g'], outflows: ['flow_f', 'flow_new'] }],
          ['stock_c', { inflows: ['flow_new'], outflows: [] }],
        ]),
      );
    },
  },
  {
    name: 'create an aux and a module',
    edit: {
      next: (model) => {
        model.views[0].elements.push({ type: 'aux', uid: UID.createdAux, name: 'Aux New', x: 5, y: 5 });
        model.views[0].elements.push({ type: 'module', uid: UID.createdModule, name: 'Module New', x: 9, y: 9 });
      },
    },
    expectOps: (ops) => {
      expect(ops.slice(0, 2)).toEqual([
        { type: 'upsertAux', payload: { aux: { name: 'Aux New', equation: '' } } },
        { type: 'upsertModule', payload: { module: { name: 'Module New', modelName: '', references: [] } } },
      ]);
    },
  },
  {
    // The model already lists Flow K as an inflow of Stock C while the view
    // shows K between clouds (a divergent import); attaching K there must not
    // list it twice, and needs no op.
    name: 'attach onto a stock that already lists the flow',
    edit: {
      setup: (model) => {
        listOf(model, 'Stock C', 'sink').push('Flow K');
      },
      next: (model) => setEnd(model, UID.flowK, 'sink', UID.stockC),
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['upsertView']);
    },
  },
  {
    // Stock C lists Flow K (not attached). The edit deletes K and attaches Flow
    // G's source onto C: the echoed outflows must not re-add K, which the
    // engine's deleteVariable already removed.
    name: 'a touched stock does not echo an entry for a flow the edit deletes',
    edit: {
      setup: (model) => {
        listOf(model, 'Stock C', 'source').push('Flow K');
      },
      next: (model) => {
        model.views[0].elements = model.views[0].elements.filter(
          (e) =>
            e.uid !== UID.flowK && e.uid !== UID.cloudKSource && e.uid !== UID.cloudKSink && e.uid !== UID.linkAliasToK,
        );
        setEnd(model, UID.flowG, 'source', UID.stockC);
      },
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['deleteVariable', 'updateStockFlows', 'upsertView']);
      expect(stockOpsOf(ops)).toEqual(new Map([['stock_c', { inflows: [], outflows: ['flow_g'] }]]));
    },
    skipM2:
      "the engine's deleteVariable strips the deleted flow from every stock list, which checkStockFlowDelta " +
      'reads as a changed other entry on Stock C; the row pins the payload and the no-duplicate/no-dangling result instead',
  },
  {
    // Removing an alias, a link, or a degenerate stock element deletes no variable.
    name: 'removing an alias, a link and a stock element with no variable deletes nothing',
    edit: { next: () => {}, deleteSelection: [UID.aliasA, UID.linkAtoX, UID.ghostStock] },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['upsertView']);
    },
  },
  {
    name: 'a second primary element keeps its variable alive',
    edit: {
      setup: (model) => {
        model.views[0].elements.push({ type: 'aux', uid: 15, name: 'Aux X', x: 900, y: 20 });
      },
      next: () => {},
      deleteSelection: [UID.auxX],
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['upsertView']);
    },
  },
  {
    // One edit exercising every op class, to pin the order one patch applies them.
    name: 'op order: rename, deletes, creates, stock ops, view',
    edit: {
      next: (model) => {
        elementByUid(model, UID.auxX).name = 'Aux Renamed';
        model.views[0].elements = model.views[0].elements.filter((e) => e.uid !== UID.flowH && e.uid !== UID.cloudH);
        model.views[0].elements.push({ type: 'aux', uid: UID.createdAux, name: 'Aux New', x: 5, y: 5 });
        setEnd(model, UID.flowK, 'sink', UID.stockC);
      },
    },
    expectOps: (ops) => {
      expect(opTypes(ops)).toEqual(['renameVariable', 'deleteVariable', 'upsertAux', 'updateStockFlows', 'upsertView']);
    },
  },
];

// ---------------------------------------------------------------------------

describe('buildEditOps (op payloads)', () => {
  function opsFor(edit: Edit): readonly JsonModelOperation[] {
    const { committed, base, next } = planFrom(authoredProject(edit), edit);
    return buildEditOps(committed, base, next);
  }

  describe('flow end attachment', () => {
    for (const row of ATTACH_ROWS) {
      it(attachRowName(row), () => {
        const ops = opsFor(attachEdit(row));
        expect(stockOpsOf(ops)).toEqual(expectedAttachStockOps(row));
        const creates = ops.filter((op) => op.type === 'upsertStock');
        expect(creates).toHaveLength(row.to === 'created' ? 1 : 0);
        expect(ops[ops.length - 1].type).toBe('upsertView');
      });
    }
  });

  for (const row of SCENARIO_ROWS) {
    it(row.name, () => {
      row.expectOps(opsFor(row.edit));
    });
  }

  it('the upsertView carries the next view as given', () => {
    const edit = attachEdit({ end: 'sink', from: 'cloud', to: 'existing' });
    const { committed, base, next } = planFrom(authoredProject(edit), edit);
    const ops = buildEditOps(committed, base, next);
    const view = ops[ops.length - 1];
    expect(view.type).toBe('upsertView');
    expect((view.payload as { index: number }).index).toBe(0);
    const k = (view.payload as { view: { elements: JsonElement[] } }).view.elements.find((e) => e.uid === UID.flowK);
    expect((k as unknown as { points: Array<{ attachedToUid: number }> }).points[1].attachedToUid).toBe(UID.stockC);
  });

  it('a created element naming an existing variable is a conflict, whatever its kind', () => {
    for (const [type, name] of [
      ['aux', 'Stock A'],
      ['stock', 'Flow F'],
      ['flow', 'aux_x'],
    ] as const) {
      const edit: Edit = {
        next: (model) => {
          model.views[0].elements.push({ type, uid: UID.createdAux, name, x: 1, y: 1, points: [] });
        },
      };
      expect(() => opsFor(edit)).toThrow(EditConflictError);
    }
  });

  it('a rename onto an existing variable is a conflict', () => {
    const edit: Edit = {
      next: (model) => {
        elementByUid(model, UID.auxX).name = 'Stock A';
      },
    };
    expect(() => opsFor(edit)).toThrow(EditConflictError);
  });

  it('a case-only rename is a rename (the engine restamps the display spelling)', () => {
    const ops = opsFor({
      next: (model) => {
        elementByUid(model, UID.auxX).name = 'AUX X';
      },
    });
    expect(ops[0]).toEqual({ type: 'renameVariable', payload: { from: 'aux_x', to: 'AUX X' } });
  });

  it('a view planned on a pending rename resolves idents from element names, not stale idents', () => {
    // The rename handler relabels an element but leaves its ident stale; an
    // edit planned on that view, dequeued after the rename landed, deletes the
    // RENAMED variable.
    const committedJson = authoredProject({
      setup: (model) => {
        model.auxiliaries.find((a) => a.name === 'Aux X')!.name = 'Aux Renamed';
        elementByUid(model, UID.auxX).name = 'Aux Renamed';
      },
      next: () => {},
    });
    const committed = projectFromJson(committedJson).models.get('main')!;
    const committedView = committed.views[0];
    const plannedOn: StockFlowView = {
      ...committedView,
      elements: committedView.elements.map((el) =>
        el.uid === UID.auxX && el.type === 'aux' ? { ...el, ident: 'aux_x' } : el,
      ),
    };
    const next = planDelete(plannedOn, new Set([UID.auxX]));
    const ops = buildEditOps(committed, plannedOn, next);
    expect(ops[0]).toEqual({ type: 'deleteVariable', payload: { ident: 'aux_renamed' } });
  });
});

describeWithEngine('buildEditOps through the engine: M1/M2/M3 on the committed result', () => {
  let engine: EngineModule;

  beforeAll(async () => {
    engine = await loadEngine();
  });

  async function runThroughEngine(edit: Edit, degenerateUids: readonly number[], skipM2?: string): Promise<void> {
    const project = await engine.Project.openJson(JSON.stringify(authoredProject(edit)));
    try {
      const committedJson = JSON.parse(await project.serializeJson(undefined, true)) as JsonProject;
      const { committed, base, next } = planFrom(committedJson, edit);
      const ops = buildEditOps(committed, base, next);
      await project.applyPatch({ models: [{ name: 'main', ops }] }, { allowErrors: true });
      const after = await editorModel(project);
      const baseVV = { view: base, variables: committed.variables };
      const nextVV = { view: after.views[0], variables: after.variables };
      const violations = [
        ...(skipM2 === undefined ? checkStockFlowDelta(baseVV, nextVV) : []),
        ...checkKindAgreement(after.views[0], after.variables).filter(
          (v) => v.uid === undefined || !degenerateUids.includes(v.uid),
        ),
        ...checkCreatedVariables(base, nextVV),
        ...checkReferentialIntegrity(after.views[0]),
        ...checkStockListDuplicates(after.variables),
      ];
      expect(formatViewViolations(violations)).toBe('');
      // Every list entry names an existing flow: nothing dangles.
      for (const variable of after.variables.values()) {
        if (variable.type === 'stock') {
          for (const entry of [...variable.inflows, ...variable.outflows]) {
            expect(after.variables.get(canonicalize(entry))?.type).toBe('flow');
          }
        }
      }
    } finally {
      await project.dispose();
    }
  }

  describe('flow end attachment', () => {
    for (const row of ATTACH_ROWS) {
      it(attachRowName(row), async () => {
        await runThroughEngine(attachEdit(row), DEGENERATE);
      });
    }
  });

  for (const row of SCENARIO_ROWS) {
    it(row.name, async () => {
      await runThroughEngine(row.edit, row.degenerateUids ?? DEGENERATE, row.skipM2);
    });
  }
});
