// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Scenarios the movement, attachment and pointer-interaction behaviors are
// specified by, as planner rows: the click threshold, label sides, rubber-band
// membership, which clicks open details, how a selection moves, how a flow end
// reattaches and a flow is drawn (with the stock lists buildEditOps derives,
// M2), a pipe drag's latch, and how links follow their moved endpoints. How a
// route bends, where a valve lands and how an offset forms belong to
// flow-geometry/ and are pinned by the flow-geometry-*.test.ts tables and
// repros, so they are not restated here.
//
// Rows are derived from enumerations where one exists: every element type for
// rubber-band membership and single-element moves, both flow ends x {stock,
// cloud} x {another stock, empty space} for reattachment, both creation sources x
// both drop kinds for drawing a flow, and every gesture kind for whether a click
// opens details. Tests are not type-checked, so each table also checks its keys
// against the enumeration at run time.
//
// What this establishes: the planner's decisions for these scenarios, and that
// the committed geometry holds the strict flow invariants and M3. What it does
// not: the Canvas rendering or dispatching them (canvas-gestures-*.test.tsx), or
// the engine applying the edits (editor-gestures-engine.test.ts).

import { describe, it, expect } from '@rstest/core';

import { canonicalize } from '@simlin/core/canonicalize';
import { variableIsArrayed, type FlowViewElement, type LinkViewElement, type UID } from '@simlin/core/datamodel';
import type { JsonViewElement } from '@simlin/engine';

import { beyondThreshold, labelSideForPointer, planGesture, type Gesture, type GesturePlan } from '../gesture-planner';
import { GESTURE_KINDS, type PressGesture } from '../gesture-planner/types';
import { ClickDragThresholdPx } from '../drawing/pointer-utils';
import { buildEditOps } from '../view-model-sync';
import {
  aux,
  cloud,
  committedReport,
  elementOf,
  flow,
  link,
  planInput,
  planned,
  routedFlows,
  scene,
  stock,
  stockToStock,
  type Pt,
  type Scene,
} from './support/gesture-fixtures';

const add = (p: Pt, d: Pt): Pt => ({ x: p.x + d.x, y: p.y + d.y });

function centerOf(s: Scene, uid: UID): Pt {
  const el = s.view.elements.find((e) => e.uid === uid) as { x: number; y: number } | undefined;
  if (el === undefined) {
    throw new Error(`no uid ${uid}`);
  }
  return { x: el.x, y: el.y };
}

function plan(s: Scene, gesture: PressGesture, press: Pt, current: Pt, selection: readonly UID[] = []) {
  return planGesture(planInput(s, gesture, press, current, { selection: new Set(selection) }));
}

/** One element of every view element type. */
function everyKind(): Scene {
  return scene([
    stock(1, 'S', 100, 100),
    cloud(2, 3, 300, 100),
    flow(3, 'F', { x: 200, y: 100 }, [
      [122.5, 100, 1],
      [300, 100, 2],
    ]),
    aux(10, 'a', 100, 300),
    { type: 'module', uid: 20, name: 'm', x: 300, y: 300 } as JsonViewElement,
    { type: 'alias', uid: 30, aliasOfUid: 10, x: 500, y: 300 } as JsonViewElement,
    link(40, 10, 3),
    { type: 'group', uid: 50, name: 'g', x: 500, y: 100, width: 100, height: 80 } as JsonViewElement,
  ]);
}

const ELEMENT_TYPES = ['stock', 'cloud', 'flow', 'aux', 'module', 'alias', 'link', 'group'] as const;

describe('beyondThreshold: a click wobbles, a drag moves', () => {
  const T = ClickDragThresholdPx;
  const rows: ReadonlyArray<{ name: string; delta: Pt; zoom: number; want: boolean }> = [
    { name: 'no movement is a click', delta: { x: 0, y: 0 }, zoom: 1, want: false },
    { name: 'sub-threshold jitter is a click', delta: { x: 1, y: 1 }, zoom: 1, want: false },
    { name: 'just under the threshold is a click', delta: { x: T - 0.5, y: 0 }, zoom: 1, want: false },
    { name: 'exactly the threshold is a drag', delta: { x: T, y: 0 }, zoom: 1, want: true },
    { name: 'well past the threshold is a drag', delta: { x: 50, y: 0 }, zoom: 1, want: true },
    { name: 'screen pixels: 3 model px at zoom 4 is a drag', delta: { x: 3, y: 0 }, zoom: 4, want: true },
    { name: 'screen pixels: 3 model px at zoom 0.5 is a click', delta: { x: 3, y: 0 }, zoom: 0.5, want: false },
    {
      name: 'Euclidean: under the threshold on each axis, past it diagonally',
      delta: { x: T / Math.SQRT2 + 0.01, y: T / Math.SQRT2 + 0.01 },
      zoom: 1,
      want: true,
    },
  ];
  for (const row of rows) {
    it(row.name, () => {
      const press = { x: 100, y: 100 };
      expect(beyondThreshold(press, add(press, row.delta), row.zoom)).toBe(row.want);
    });
  }
});

describe('labelSideForPointer: the label goes to the side the pointer is on', () => {
  const rows: ReadonlyArray<{ name: string; pointer: Pt; want: string }> = [
    { name: 'pointer to the left', pointer: { x: -10, y: 0 }, want: 'left' },
    { name: 'pointer to the right', pointer: { x: 10, y: 0 }, want: 'right' },
    { name: 'pointer above', pointer: { x: 0, y: -10 }, want: 'top' },
    { name: 'pointer below', pointer: { x: 0, y: 10 }, want: 'bottom' },
    { name: 'the upper-left diagonal belongs to left', pointer: { x: -10, y: -10 }, want: 'left' },
    { name: 'the upper-right diagonal belongs to top', pointer: { x: 10, y: -10 }, want: 'top' },
    { name: 'the lower-left diagonal belongs to bottom', pointer: { x: -10, y: 10 }, want: 'bottom' },
    { name: 'the lower-right diagonal belongs to right', pointer: { x: 10, y: 10 }, want: 'right' },
  ];
  for (const row of rows) {
    it(row.name, () => {
      expect(labelSideForPointer({ x: 0, y: 0 }, row.pointer)).toBe(row.want);
    });
  }
});

describe('rubberBand membership, by element type', () => {
  type Rule = 'center' | 'centerOrCorner' | 'never';
  const MEMBERSHIP: Readonly<Record<(typeof ELEMENT_TYPES)[number], { uid: UID; rule: Rule }>> = {
    stock: { uid: 1, rule: 'center' },
    cloud: { uid: 2, rule: 'center' },
    flow: { uid: 3, rule: 'center' },
    aux: { uid: 10, rule: 'centerOrCorner' },
    module: { uid: 20, rule: 'center' },
    alias: { uid: 30, rule: 'center' },
    link: { uid: 40, rule: 'never' },
    group: { uid: 50, rule: 'never' },
  };

  function band(s: Scene, press: Pt, current: Pt): ReadonlySet<UID> {
    const p = planGesture(planInput(s, { kind: 'rubberBand' }, press, current, { clickSelection: new Set() }));
    expect(p.commit).toBe('select');
    return p.selection;
  }

  it('the table and the scene cover every element type', () => {
    expect(Object.keys(MEMBERSHIP).sort()).toEqual([...ELEMENT_TYPES].sort());
    expect([...new Set(everyKind().view.elements.map((el) => el.type))].sort()).toEqual([...ELEMENT_TYPES].sort());
  });

  for (const [type, { uid, rule }] of Object.entries(MEMBERSHIP)) {
    if (rule === 'never') {
      it(`${type}: never selected, even by a band covering everything`, () => {
        expect(band(everyKind(), { x: -1000, y: -1000 }, { x: 2000, y: 2000 }).has(uid)).toBe(false);
      });
      continue;
    }
    it(`${type}: selected by a band around its center`, () => {
      const s = everyKind();
      const c = centerOf(s, uid);
      expect(band(s, add(c, { x: -15, y: -15 }), add(c, { x: 15, y: 15 })).has(uid)).toBe(true);
    });
    if (rule === 'center') {
      it(`${type}: a band overlapping it but not its center does not select it`, () => {
        const s = everyKind();
        const c = centerOf(s, uid);
        expect(band(s, add(c, { x: 3, y: -15 }), add(c, { x: 40, y: 15 })).has(uid)).toBe(false);
      });
    } else {
      it(`${type}: a band whose corner lies within the circle selects it, one just outside does not`, () => {
        const s = everyKind();
        const c = centerOf(s, uid);
        expect(band(s, add(c, { x: 5, y: 5 }), add(c, { x: 60, y: 60 })).has(uid)).toBe(true);
        expect(band(s, add(c, { x: 8, y: 8 }), add(c, { x: 60, y: 60 })).has(uid)).toBe(false);
      });
    }
  }

  it('a click (a band within the threshold) selects nothing', () => {
    expect([...band(everyKind(), { x: 100, y: 100 }, { x: 101, y: 101 })]).toEqual([]);
  });
});

describe('which clicks open details, by gesture kind', () => {
  const detailsScene = (): Scene =>
    scene([
      stock(1, 'S', 100, 100),
      cloud(2, 3, 300, 100),
      flow(3, 'F', { x: 200, y: 100 }, [
        [122.5, 100, 1],
        [300, 100, 2],
      ]),
      aux(10, 'a', 100, 300),
      aux(11, 'b', 300, 300),
      link(13, 10, 11, 20),
    ]);
  // A click on an element's body (or its valve, pipe or link body) opens details;
  // a click on an end grip, a label, a tool press or empty canvas does not.
  const DETAILS: Readonly<
    Record<Gesture['kind'], { gesture: Gesture; selection: UID[]; press: Pt; details: boolean }>
  > = {
    moveSelection: { gesture: { kind: 'moveSelection' }, selection: [1], press: { x: 100, y: 100 }, details: true },
    slideValve: { gesture: { kind: 'slideValve', flow: 3 }, selection: [3], press: { x: 200, y: 100 }, details: true },
    offsetSegment: {
      gesture: { kind: 'offsetSegment', flow: 3, segmentIndex: 0 },
      selection: [3],
      press: { x: 250, y: 100 },
      details: true,
    },
    linkArc: { gesture: { kind: 'linkArc', link: 13 }, selection: [13], press: { x: 200, y: 280 }, details: true },
    flowEndpoint: {
      gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
      selection: [3],
      press: { x: 300, y: 100 },
      details: false,
    },
    linkEndpoint: {
      gesture: { kind: 'linkEndpoint', link: 13 },
      selection: [13],
      press: { x: 291, y: 300 },
      details: false,
    },
    createFlow: {
      gesture: { kind: 'createFlow', from: 'empty' },
      selection: [],
      press: { x: 500, y: 500 },
      details: false,
    },
    createLink: { gesture: { kind: 'createLink', from: 10 }, selection: [], press: { x: 100, y: 300 }, details: false },
    createElement: {
      gesture: { kind: 'createElement', type: 'aux' },
      selection: [],
      press: { x: 500, y: 500 },
      details: false,
    },
    label: { gesture: { kind: 'label', uid: 10 }, selection: [10], press: { x: 100, y: 300 }, details: false },
    rubberBand: { gesture: { kind: 'rubberBand' }, selection: [], press: { x: 500, y: 500 }, details: false },
    pan: { gesture: { kind: 'pan' }, selection: [], press: { x: 500, y: 500 }, details: false },
  };

  it('has a row for every gesture kind', () => {
    expect(Object.keys(DETAILS).sort()).toEqual([...GESTURE_KINDS].sort());
  });

  for (const [kind, row] of Object.entries(DETAILS)) {
    it(`${kind}: a click ${row.details ? 'opens' : 'does not open'} details`, () => {
      const p = plan(detailsScene(), row.gesture, row.press, add(row.press, { x: 1, y: 1 }), row.selection);
      expect(!!p.details).toBe(row.details);
    });
    if (row.details) {
      it(`${kind}: a drag never opens details`, () => {
        const p = plan(detailsScene(), row.gesture, row.press, add(row.press, { x: 40, y: 25 }), row.selection);
        expect(!!p.details).toBe(false);
      });
    }
  }

  it('an unlatched pipe press: a click opens details', () => {
    const p = plan(
      detailsScene(),
      { kind: 'pipe', flow: 3, segmentIndex: 0 },
      { x: 250, y: 100 },
      { x: 251, y: 101 },
      [3],
    );
    expect(!!p.details).toBe(true);
  });
});

describe('moveSelection', () => {
  // A lone flow slides its valve, a lone cloud drags its flow's end and a link
  // curves; those are rows below, not translations.
  const POSITIONED: Readonly<Record<'stock' | 'aux' | 'module' | 'alias' | 'group', UID>> = {
    stock: 1,
    aux: 10,
    module: 20,
    alias: 30,
    group: 50,
  };

  for (const [type, uid] of Object.entries(POSITIONED)) {
    it(`a selected ${type} translates by exactly the pointer delta`, () => {
      const s = everyKind();
      const c = centerOf(s, uid);
      const p = plan(s, { kind: 'moveSelection' }, c, add(c, { x: 40, y: 30 }), [uid]);
      expect(p.commit).toBe('edit');
      expect(elementOf(p, uid)).toMatchObject(add(c, { x: 40, y: 30 }));
      expect(committedReport(s, p)).toBe('');
    });
  }

  it('a chain whose every element is selected translates rigidly, flow points and valve included', () => {
    const s = stockToStock();
    const d = { x: 30, y: 50 };
    const p = plan(s, { kind: 'moveSelection' }, { x: 100, y: 100 }, add({ x: 100, y: 100 }, d), [1, 2, 3]);
    const before = s.view.elements.find((e) => e.uid === 3) as FlowViewElement;
    const after = elementOf(p, 3) as FlowViewElement;
    expect(after.points.map((pt) => ({ x: pt.x, y: pt.y }))).toEqual(before.points.map((pt) => add(pt, d)));
    expect({ x: after.x, y: after.y }).toEqual(add(before, d));
    expect(elementOf(p, 1)).toMatchObject({ x: 130, y: 150 });
    expect(elementOf(p, 2)).toMatchObject({ x: 430, y: 150 });
  });

  it('a stock and its flow`s cloud selected together translate the flow rigidly', () => {
    const s = everyKind();
    const d = { x: 20, y: 60 };
    const p = plan(s, { kind: 'moveSelection' }, { x: 100, y: 100 }, add({ x: 100, y: 100 }, d), [1, 2]);
    const before = s.view.elements.find((e) => e.uid === 3) as FlowViewElement;
    const after = elementOf(p, 3) as FlowViewElement;
    expect(after.points.map((pt) => ({ x: pt.x, y: pt.y }))).toEqual(before.points.map((pt) => add(pt, d)));
    expect(committedReport(s, p)).toBe('');
  });

  it('a selected flow whose ends do not move keeps its path and slides only its valve', () => {
    const s = everyKind();
    const p = plan(s, { kind: 'moveSelection' }, { x: 200, y: 100 }, { x: 240, y: 100 }, [3, 10]);
    const before = s.view.elements.find((e) => e.uid === 3) as FlowViewElement;
    const after = elementOf(p, 3) as FlowViewElement;
    expect(after.points).toEqual(before.points);
    expect(after.x).toBeCloseTo(240);
    expect(elementOf(p, 10)).toMatchObject({ x: 140, y: 300 });
  });

  it('a stock moved without its flows routes every attached flow onto its moved faces', () => {
    const s = scene([
      stock(1, 'S', 200, 200),
      cloud(11, 21, 400, 200),
      cloud(12, 22, 200, 400),
      cloud(13, 23, 200, 0),
      flow(21, 'F1', { x: 300, y: 200 }, [
        [222.5, 200, 1],
        [400, 200, 11],
      ]),
      flow(22, 'F2', { x: 200, y: 300 }, [
        [200, 217.5, 1],
        [200, 400, 12],
      ]),
      flow(23, 'F3', { x: 200, y: 100 }, [
        [200, 182.5, 1],
        [200, 0, 13],
      ]),
    ]);
    const p = plan(s, { kind: 'moveSelection' }, { x: 200, y: 200 }, { x: 230, y: 230 }, [1]);
    expect([...routedFlows(s, p)].sort()).toEqual([21, 22, 23]);
    for (const uid of [21, 22, 23]) {
      expect((elementOf(p, uid) as FlowViewElement).points[0].attachedToUid).toBe(1);
    }
    expect(committedReport(s, p)).toBe('');
  });

  it('a cloud moved without its flow drags that end of the flow, and the cloud stays at the endpoint', () => {
    const s = everyKind();
    const p = plan(s, { kind: 'moveSelection' }, { x: 300, y: 100 }, { x: 300, y: 160 }, [2]);
    expect(p.commit).toBe('edit');
    expect(routedFlows(s, p).has(3)).toBe(true);
    expect(committedReport(s, p)).toBe('');
  });
});

describe('a pipe drag latches once: perpendicular offsets, along the pipe slides', () => {
  it('perpendicular: the straight flow bends and holds the invariants', () => {
    const s = everyKind();
    const p = plan(s, { kind: 'pipe', flow: 3, segmentIndex: 0 }, { x: 250, y: 100 }, { x: 250, y: 140 }, [3]);
    expect(p.commit).toBe('edit');
    expect((elementOf(p, 3) as FlowViewElement).points.length).toBeGreaterThan(2);
    expect(committedReport(s, p)).toBe('');
  });

  it('along the pipe: the path stays and the valve slides', () => {
    const s = everyKind();
    const before = s.view.elements.find((e) => e.uid === 3) as FlowViewElement;
    const p = plan(s, { kind: 'pipe', flow: 3, segmentIndex: 0 }, { x: 200, y: 100 }, { x: 240, y: 100 }, [3]);
    const after = elementOf(p, 3) as FlowViewElement;
    expect(after.points).toEqual(before.points);
    expect(after.x).toBeCloseTo(240);
  });

  it('a perpendicular wobble within the threshold does not bend the flow', () => {
    const s = everyKind();
    const p = plan(s, { kind: 'pipe', flow: 3, segmentIndex: 0 }, { x: 250, y: 100 }, { x: 250, y: 103 }, [3]);
    expect(p.commit).not.toBe('edit');
    expect(p.elements).toBe(s.view.elements);
  });
});

type StockLists = { inflows: string[]; outflows: string[] };

/** The stock lists the plan's edit rewrites, by stock ident: exactly the updateStockFlows ops buildEditOps derives. */
function stockListOps(s: Scene, p: GesturePlan): Map<string, StockLists> {
  const out = new Map<string, StockLists>();
  for (const op of buildEditOps(s.model, s.view, planned(s, p))) {
    if (op.type === 'updateStockFlows') {
      out.set(canonicalize(op.payload.ident), {
        inflows: op.payload.inflows.map(canonicalize).sort(),
        outflows: op.payload.outflows.map(canonicalize).sort(),
      });
    }
  }
  return out;
}

/** A (1) -> B (2) through f (3); cloud c6 (6) -> cloud c7 (7) through g (5); target stock T (4) below. */
function attachScene(): Scene {
  return scene([
    stock(1, 'A', 100, 100),
    stock(2, 'B', 400, 100),
    stock(4, 'T', 250, 300),
    flow(3, 'f', { x: 250, y: 100 }, [
      [122.5, 100, 1],
      [377.5, 100, 2],
    ]),
    cloud(6, 5, 100, 450),
    cloud(7, 5, 400, 450),
    flow(5, 'g', { x: 250, y: 450 }, [
      [100, 450, 6],
      [400, 450, 7],
    ]),
  ]);
}

describe('flowEndpoint: every end x attachment x drop kind, with the stock lists it implies (M2)', () => {
  const ENDS = ['source', 'sink'] as const;
  const FROM = ['stock', 'cloud'] as const;
  const TO = ['stock', 'empty'] as const;

  for (const end of ENDS) {
    for (const from of FROM) {
      for (const to of TO) {
        it(`${end} on a ${from}, dropped on ${to === 'stock' ? 'another stock' : 'empty space'}`, () => {
          const s = attachScene();
          const flowUid = from === 'stock' ? 3 : 5;
          const name = from === 'stock' ? 'f' : 'g';
          const base = s.view.elements.find((e) => e.uid === flowUid) as FlowViewElement;
          const baseEnd = end === 'source' ? base.points[0] : base.points[base.points.length - 1];
          const press = { x: baseEnd.x, y: baseEnd.y };
          const current = to === 'stock' ? { x: 250, y: 300 } : add(press, { x: 0, y: 70 });

          const p = plan(s, { kind: 'flowEndpoint', flow: flowUid, end }, press, current, [flowUid]);
          expect(p.commit).toBe('edit');
          expect(committedReport(s, p)).toBe('');
          const routed = elementOf(p, flowUid) as FlowViewElement;
          const routedEnd = end === 'source' ? routed.points[0] : routed.points[routed.points.length - 1];

          const want = new Map<string, StockLists>();
          const listed = end === 'source' ? 'outflows' : 'inflows';
          if (from === 'stock') {
            // The old stock's list loses the flow; the stock at the other end is untouched.
            want.set(end === 'source' ? 'a' : 'b', { inflows: [], outflows: [] });
          }
          if (to === 'stock') {
            expect(routedEnd.attachedToUid).toBe(4);
            want.set('t', { inflows: [], outflows: [], [listed]: [name] } as StockLists);
          } else {
            const endCloud = p.elements.find((e) => e.uid === routedEnd.attachedToUid);
            expect(endCloud?.type === 'cloud' && endCloud.flowUid === flowUid).toBe(true);
            if (from === 'cloud') {
              // The same cloud moves with the end; no new cloud is made.
              expect(routedEnd.attachedToUid).toBe(baseEnd.attachedToUid);
            }
          }
          if (from === 'cloud' && to === 'stock') {
            // The detached cloud is removed with the drop.
            expect(p.elements.some((e) => e.uid === baseEnd.attachedToUid)).toBe(false);
          }
          expect(stockListOps(s, p)).toEqual(want);
        });
      }
    }
  }
});

describe('createFlow: every source kind x drop kind, with the model it implies (M1/M2)', () => {
  const FROM = ['empty', 'stock'] as const;
  const TO = ['empty', 'stock'] as const;

  for (const from of FROM) {
    for (const to of TO) {
      it(`from ${from === 'stock' ? 'a stock' : 'empty space'} to ${to === 'stock' ? 'a stock' : 'empty space'}`, () => {
        const s = attachScene();
        const gesture: Gesture = { kind: 'createFlow', from: from === 'stock' ? { stock: 1 } : 'empty' };
        const press = from === 'stock' ? { x: 100, y: 100 } : { x: 600, y: 300 };
        const current = to === 'stock' ? { x: 250, y: 300 } : add(press, { x: 0, y: 120 });

        const p = plan(s, gesture, press, current);
        expect(p.commit).toBe('edit');
        expect(committedReport(s, p)).toBe('');
        const created = p.elements.find((e): e is FlowViewElement => e.type === 'flow' && e.uid === s.view.nextUid);
        expect(created).toBeDefined();
        expect([...p.selection]).toEqual([created!.uid]);
        expect(p.handoff?.editName).toBe(created!.uid);
        expect(p.nextUid).toBeGreaterThan(created!.uid);

        const source = created!.points[0];
        const sink = created!.points[created!.points.length - 1];
        const cloudOf = (uid: UID | undefined) => p.elements.find((e) => e.uid === uid);
        expect(from === 'stock' ? source.attachedToUid === 1 : cloudOf(source.attachedToUid)?.type === 'cloud').toBe(
          true,
        );
        expect(to === 'stock' ? sink.attachedToUid === 4 : cloudOf(sink.attachedToUid)?.type === 'cloud').toBe(true);

        const ops = buildEditOps(s.model, s.view, planned(s, p));
        expect(ops.some((op) => op.type === 'upsertFlow')).toBe(true);
        const want = new Map<string, StockLists>();
        if (from === 'stock') {
          want.set('a', { inflows: [], outflows: ['f', 'new_flow'] });
        }
        if (to === 'stock') {
          want.set('t', { inflows: ['new_flow'], outflows: [] });
        }
        expect(stockListOps(s, p)).toEqual(want);
      });
    }
  }

  // A drawn flow ends exactly at the pointer (its sink cloud is there), so a drag
  // off an axis is an L whose long leg -- along the dominant axis -- runs into
  // the sink, where the arrowhead needs room; a drag along an axis is straight.
  const DIRECTIONS = [
    { name: 'mostly down', delta: { x: 10, y: 80 }, sinkAxis: 'vertical', points: 3 },
    { name: 'mostly up', delta: { x: 10, y: -80 }, sinkAxis: 'vertical', points: 3 },
    { name: 'mostly right', delta: { x: 80, y: 10 }, sinkAxis: 'horizontal', points: 3 },
    { name: 'mostly left', delta: { x: -80, y: 10 }, sinkAxis: 'horizontal', points: 3 },
    { name: 'straight down', delta: { x: 0, y: 80 }, sinkAxis: 'vertical', points: 2 },
    { name: 'straight right', delta: { x: 80, y: 0 }, sinkAxis: 'horizontal', points: 2 },
  ] as const;
  for (const row of DIRECTIONS) {
    it(`drawn from empty space ${row.name}: the sink cloud is at the pointer and the flow enters it ${row.sinkAxis}ly`, () => {
      const s = attachScene();
      const press = { x: 700, y: 600 };
      const current = add(press, row.delta);
      const p = plan(s, { kind: 'createFlow', from: 'empty' }, press, current);
      expect(committedReport(s, p)).toBe('');
      const created = p.elements.find((e): e is FlowViewElement => e.type === 'flow' && e.uid === s.view.nextUid)!;
      expect(created.points).toHaveLength(row.points);
      const sink = created.points[created.points.length - 1];
      const beforeSink = created.points[created.points.length - 2];
      expect(p.elements.find((e) => e.uid === sink.attachedToUid)).toMatchObject({ type: 'cloud', ...current });
      expect(row.sinkAxis === 'vertical' ? beforeSink.x === sink.x : beforeSink.y === sink.y).toBe(true);
    });
  }
});

describe('links follow their moved endpoints once, from the final positions', () => {
  function linked(arc: number, arrayed: readonly string[] = []): Scene {
    return scene([aux(10, 'a', 100, 100), aux(11, 'b', 200, 100), link(13, 10, 11, arc)], [], arrayed);
  }
  const arcOf = (p: ReturnType<typeof plan>): number => (elementOf(p, 13) as LinkViewElement).arc!;

  it('both endpoints moved alike keep the arc', () => {
    const p = plan(linked(30), { kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 150, y: 125 }, [10, 11]);
    expect(arcOf(p)).toBe(30);
  });

  it('one endpoint moved along the link`s line keeps the arc', () => {
    const p = plan(linked(30), { kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 150, y: 100 }, [10]);
    expect(arcOf(p)).toBeCloseTo(30, 5);
  });

  it('one endpoint rotating the line turns the arc by the rotation', () => {
    const p = plan(linked(0), { kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 100, y: 200 }, [10]);
    expect(arcOf(p)).toBeCloseTo(-45, 0);
  });

  it('a link selected along with one endpoint is turned once, not twice', () => {
    const p = plan(linked(0), { kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 100, y: 200 }, [10, 13]);
    expect(arcOf(p)).toBeCloseTo(-45, 0);
  });

  // An arrayed element's link anchors at its front shape (offset by 3px), both
  // before and after the move, so the turn matches the visual rotation.
  const ARRAYED = [
    { name: 'the moved source is arrayed', arrayed: ['a'], moved: 10, want: -45 },
    { name: 'both ends are arrayed', arrayed: ['a', 'b'], moved: 10, want: -45 },
    // Old visual line (100,100)->(197,97), new (100,100)->(197,197): the line turns
    // by 45 + atan(3/97) degrees.
    { name: 'the moved target is arrayed', arrayed: ['b'], moved: 11, want: 45 + (Math.atan2(3, 97) * 180) / Math.PI },
  ] as const;
  for (const row of ARRAYED) {
    it(`${row.name}: the arc turns by the visual rotation`, () => {
      const s = linked(0, row.arrayed);
      for (const name of row.arrayed) {
        const el = s.view.elements.find((e) => 'name' in e && e.name === name) as { var?: never };
        expect(el.var !== undefined && variableIsArrayed(el.var)).toBe(true);
      }
      const c = centerOf(s, row.moved);
      const p = plan(s, { kind: 'moveSelection' }, c, add(c, { x: 0, y: 100 }), [row.moved]);
      expect(Math.abs(arcOf(p) - row.want)).toBeLessThan(0.5);
    });
  }
});
