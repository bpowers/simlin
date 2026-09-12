// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// E3 continuity over the most common real gestures, in 1px frames.
//
// Rows come from a gesture enumeration:
// - a stock dragged in a circle around its straight flow's cloud (radius x the
//   flow's end at the stock);
// - a stock dragged in a line across its flow's axis at several distances;
// - a stock dragged in a circle around the other stock of a stock-stock flow;
// - a stock carrying three flows on one face, dragged in a circle and in a line;
// - a cloud dragged in a circle around its stock (face x the flow's end at the
//   stock x radius), and along and across the flow's axis;
// - a stock end detached and dragged around the stock it left (end x radius);
// - a flow created from a stock and dragged around it (radius).
// Every frame routes as the planner will: routeEnd against the base view for
// the flows on a dragged stock and for a dragged or detached end, route for a
// creation draft.
//
// Each row pins, per flow:
// - the number of route shape transitions (a shape is the faces at both ends
//   plus the segment directions), at the implementation's measured count;
// - that no frame violates the strict invariants;
// - the shape flip-flops, A -> B -> A within FLIP_WINDOW px of pointer travel.
// A row expects no flip-flops unless it lists them. The budgets are measured,
// not derived: a circle visits each face transition twice, so 12 per revolution
// is what the geometry needs, and a count above a row's budget is flicker: a
// shape change no feasibility change forces.
//
// The listed flip-flops are of one kind, which the last describe pins over a
// band of radii. The route partitions pointer positions into regions whose edges
// are G3 minima, and a region two minima bound has a corner. A circle whose
// radius is just past that corner's distance clips it: it passes a few px
// through the region and returns to the region it came from. A pure function
// with no previous frame (so no hysteresis) cannot avoid that on such a path.

import { describe, it, expect } from '@rstest/core';

import type { CloudViewElement, FlowViewElement, StockFlowView, UID } from '@simlin/core/datamodel';

import {
  faceOfEndpoint,
  FACES,
  flowTerminals,
  freeTerminal,
  route,
  routeEnd,
  stockTerminal,
  type Face,
  type FlowEnd,
} from '../flow-geometry';
import {
  applyGeometry,
  byUidOf,
  cloudJson,
  directions,
  flowJson,
  flowOf,
  loadView,
  stockJson,
  stockOf,
  strictReport,
  type Pt,
} from './support/flow-geometry-fixtures';

const FLIP_WINDOW = 20;

interface SweepResult {
  readonly transitions: Map<UID, number>;
  readonly violations: string[];
  readonly flips: string[];
}

/** One frame of a gesture: the planned view and the flows it routed. */
interface Frame {
  readonly view: StockFlowView;
  readonly routed: readonly UID[];
}

function shapeOf(f: FlowViewElement, view: StockFlowView): string {
  const byUid = byUidOf(view);
  const n = f.points.length;
  const face = (i: number, j: number): string => {
    const el = byUid.get(f.points[i].attachedToUid ?? -1);
    return el?.type === 'stock' ? (faceOfEndpoint(el, f.points[i], f.points[j]) ?? '?') : '-';
  };
  return `${face(0, 1)} ${directions(f.points)} ${face(n - 1, n - 2)}`;
}

function sweep(positions: readonly Pt[], frame: (p: Pt) => Frame, check = true): SweepResult {
  const transitions = new Map<UID, number>();
  const history = new Map<UID, Array<{ shape: string; travel: number }>>();
  const violations: string[] = [];
  const flips: string[] = [];
  let travel = 0;
  for (let k = 0; k < positions.length; k++) {
    const p = positions[k];
    if (k > 0) travel += Math.hypot(p.x - positions[k - 1].x, p.y - positions[k - 1].y);
    const { view, routed } = frame(p);
    for (const uid of routed) {
      const f = view.elements.find((e) => e.uid === uid) as FlowViewElement;
      const report = check ? strictReport(view, [uid]) : '';
      if (report !== '' && violations.length < 3) violations.push(`frame ${k}: ${report}`);
      const shape = shapeOf(f, view);
      const h = history.get(uid) ?? [];
      if (h.length === 0 || h[h.length - 1].shape !== shape) {
        if (h.length > 0) transitions.set(uid, (transitions.get(uid) ?? 0) + 1);
        // A -> B -> A: the shape two changes back, re-entered within the window.
        if (h.length >= 2 && h[h.length - 2].shape === shape && travel - h[h.length - 1].travel <= FLIP_WINDOW) {
          flips.push(`flow ${uid} frame ${k}: ${h[h.length - 2].shape} -> ${h[h.length - 1].shape} -> ${shape}`);
        }
        h.push({ shape, travel });
      }
      history.set(uid, h);
    }
  }
  return { transitions, violations, flips };
}

/** Drag a stock: every flow on it is re-routed with routeEnd against the base view. */
function stockSweep(view0: StockFlowView, stockUid: UID, positions: readonly Pt[]): SweepResult {
  const base = stockOf(view0, stockUid);
  const byUid = byUidOf(view0);
  const flows = view0.elements.filter(
    (e): e is FlowViewElement =>
      e.type === 'flow' &&
      (e.points[0].attachedToUid === stockUid || e.points[e.points.length - 1].attachedToUid === stockUid),
  );
  return sweep(positions, (p) => {
    const moved = { ...base, x: p.x, y: p.y };
    const routed = flows.map((f) => {
      const n = f.points.length;
      const t = flowTerminals(f, byUid);
      const sourceOn = f.points[0].attachedToUid === stockUid;
      const endpoint = sourceOn ? f.points[0] : f.points[n - 1];
      const adjacent = sourceOn ? f.points[1] : f.points[n - 2];
      return routeEnd(f, sourceOn ? 'source' : 'sink', stockTerminal(moved, endpoint, adjacent, base), {
        fixed: sourceOn ? t.sink : t.source,
      }).flow;
    });
    const view = applyGeometry(view0, { flow: routed[0], clouds: [] }, [moved, ...routed.slice(1)]);
    return { view, routed: flows.map((f) => f.uid) };
  });
}

/** Drag a flow's cloud end: routeEnd against the base view with the other end fixed. */
function cloudSweep(view0: StockFlowView, flowUid: UID, positions: readonly Pt[], check = true): SweepResult {
  const f = flowOf(view0, flowUid);
  const t = flowTerminals(f, byUidOf(view0));
  const end: FlowEnd = t.sink.kind === 'free' ? 'sink' : 'source';
  const own = end === 'sink' ? t.sink : t.source;
  const cloud = own.kind === 'free' ? own.cloud : undefined;
  const fixed = end === 'sink' ? t.source : t.sink;
  return sweep(
    positions,
    (p) => {
      const g = routeEnd(f, end, freeTerminal(p, cloud), { fixed });
      return { view: applyGeometry(view0, g), routed: [flowUid] };
    },
    check,
  );
}

function circle(center: Pt, r: number, startAngle: number): Pt[] {
  const count = Math.ceil(2 * Math.PI * r);
  return Array.from({ length: count + 1 }, (_, i) => {
    const th = startAngle + (2 * Math.PI * i) / count;
    return { x: center.x + r * Math.cos(th), y: center.y + r * Math.sin(th) };
  });
}

function line(from: Pt, to: Pt): Pt[] {
  const count = Math.ceil(Math.hypot(to.x - from.x, to.y - from.y));
  return Array.from({ length: count + 1 }, (_, i) => ({
    x: from.x + ((to.x - from.x) * i) / count,
    y: from.y + ((to.y - from.y) * i) / count,
  }));
}

/** Detach a flow's stock end: routeEnd to a new cloud at the pointer, the other end fixed. */
function detachSweep(view0: StockFlowView, flowUid: UID, end: FlowEnd, positions: readonly Pt[]): SweepResult {
  const f = flowOf(view0, flowUid);
  const t = flowTerminals(f, byUidOf(view0));
  const detached: CloudViewElement = {
    type: 'cloud',
    uid: 20,
    flowUid,
    x: 0,
    y: 0,
    isZeroRadius: false,
    ident: undefined,
  };
  const fixed = end === 'source' ? t.sink : t.source;
  return sweep(positions, (p) => {
    const g = routeEnd(f, end, freeTerminal(p, detached), { fixed });
    const added = g.clouds.some((c) => c.uid === detached.uid) ? [] : [{ ...detached, x: p.x, y: p.y }];
    return { view: applyGeometry(view0, g, added), routed: [flowUid] };
  });
}

/** Create a flow from stock 1 of `view0` to a new cloud at the pointer. */
function createSweep(view0: StockFlowView, positions: readonly Pt[]): SweepResult {
  const draft = flowOf(loadView([flowJson(10, { x: 0, y: 0 }, [], {})]), 10);
  const sink: CloudViewElement = {
    type: 'cloud',
    uid: 20,
    flowUid: 10,
    x: 0,
    y: 0,
    isZeroRadius: false,
    ident: undefined,
  };
  const source = stockTerminal(stockOf(view0, 1));
  return sweep(positions, (p) => ({
    view: applyGeometry(view0, route(source, freeTerminal(p, sink), { flow: draft })),
    routed: [10],
  }));
}

/** A stock at `stock` with one straight flow out of (source) or into (sink) its right face from a cloud at `cloud`. */
function straightScene(stock: Pt, cloud: Pt, end: FlowEnd): StockFlowView {
  const face = { x: stock.x + 22.5, y: stock.y };
  const points = end === 'source' ? [face, cloud] : [cloud, face];
  return loadView([
    stockJson(1, stock.x, stock.y),
    cloudJson(3, 10, cloud.x, cloud.y),
    flowJson(
      10,
      { x: (face.x + cloud.x) / 2, y: cloud.y },
      points,
      end === 'source' ? { source: 1, sink: 3 } : { source: 3, sink: 1 },
    ),
  ]);
}

const STOCK = { x: 500, y: 500 };
const OUTWARD: Record<Face, Pt> = {
  left: { x: -1, y: 0 },
  right: { x: 1, y: 0 },
  top: { x: 0, y: -1 },
  bottom: { x: 0, y: 1 },
};

/** Stock 1 at STOCK with one straight flow out of (source) or into (sink) the middle of `face`, from a cloud r from its center. */
function faceScene(face: Face, end: FlowEnd, r: number): StockFlowView {
  const o = OUTWARD[face];
  const endpoint = { x: STOCK.x + o.x * 22.5, y: STOCK.y + o.y * 17.5 };
  const cloud = { x: STOCK.x + o.x * r, y: STOCK.y + o.y * r };
  return loadView([
    stockJson(1, STOCK.x, STOCK.y),
    cloudJson(3, 10, cloud.x, cloud.y),
    flowJson(
      10,
      { x: (endpoint.x + cloud.x) / 2, y: (endpoint.y + cloud.y) / 2 },
      end === 'source' ? [endpoint, cloud] : [cloud, endpoint],
      end === 'source' ? { source: 1, sink: 3 } : { source: 3, sink: 1 },
    ),
  ]);
}

function cloudCircle(face: Face, end: FlowEnd, r: number, check = true): SweepResult {
  const o = OUTWARD[face];
  return cloudSweep(faceScene(face, end, r), 10, circle(STOCK, r, Math.atan2(o.y, o.x)), check);
}

/**
 * The measured count for a cloud circling its stock. With the flow as source
 * (the cloud is its sink) the pinned Z between the sliding endpoint and the L
 * (routeEnd k = 0 in -route) adds two transitions per revolution once the
 * circle reaches the Z's region; at r = 45 the top and bottom faces clip its
 * corner instead, which is two more (the listed flip-flops).
 */
function cloudCircleBudget(face: Face, end: FlowEnd, r: number): number {
  if (end === 'sink') return 12;
  if (r > 45) return 14;
  return face === 'top' || face === 'bottom' ? 16 : 12;
}

/** A cloud circling its stock's flip-flops where the circle clips a corner (see the last describe). */
const CLIPPED_CORNERS: Readonly<Record<string, readonly string[]>> = {
  'top source 45': [
    'flow 10 frame 14: top U - -> top URU - -> top U -',
    'flow 10 frame 273: top U - -> top ULU - -> top U -',
  ],
  'bottom source 45': [
    'flow 10 frame 14: bottom D - -> bottom DLD - -> bottom D -',
    'flow 10 frame 273: bottom D - -> bottom DRD - -> bottom D -',
  ],
};

interface Row {
  readonly name: string;
  readonly run: () => SweepResult;
  /** Transitions per flow uid, pinned at the measured count: more is flicker, fewer an improvement to re-pin. */
  readonly budget: Readonly<Record<number, number>>;
  /** The flip-flops the row shows, each a clipped corner (see the last describe). */
  readonly flips?: readonly string[];
}

const CLOUD = { x: 500, y: 500 };

const ROWS: Row[] = [
  ...[45, 100, 160, 250].flatMap((r) =>
    (['source', 'sink'] as const).map((end) => ({
      name: `a stock circling its cloud, r = ${r}, flow as ${end}`,
      run: () => stockSweep(straightScene({ x: CLOUD.x - r, y: CLOUD.y }, CLOUD, end), 1, circle(CLOUD, r, Math.PI)),
      budget: { 10: 12 },
    })),
  ),
  ...[40, 160].map((dx) => ({
    name: `a stock dragged down across its flow's axis, ${dx}px from the cloud`,
    run: () => {
      const s = { x: CLOUD.x - dx, y: CLOUD.y };
      return stockSweep(straightScene(s, CLOUD, 'source'), 1, line(s, { x: s.x, y: s.y + 200 }));
    },
    budget: { 10: 1 },
  })),
  ...[200, 100].map((r) => ({
    name: `a stock circling the other stock of a stock-stock flow, r = ${r}`,
    run: () =>
      stockSweep(
        loadView([
          stockJson(1, 500 - r, 400),
          stockJson(2, 500, 400),
          flowJson(
            10,
            { x: 500 - r / 2, y: 400 },
            [
              { x: 522.5 - r, y: 400 },
              { x: 477.5, y: 400 },
            ],
            { source: 1, sink: 2 },
          ),
        ]),
        1,
        circle({ x: 500, y: 400 }, r, Math.PI),
      ),
    budget: { 10: 14 },
  })),
  ...(['circle', 'line'] as const).map((kind) => ({
    name: `three flows on one face, the stock dragged in a ${kind}`,
    run: () =>
      stockSweep(
        loadView([
          stockJson(1, 400, 400),
          cloudJson(3, 10, 560, 390),
          cloudJson(4, 11, 520, 300),
          cloudJson(5, 12, 540, 480),
          flowJson(
            10,
            { x: 480, y: 390 },
            [
              { x: 422.5, y: 390 },
              { x: 560, y: 390 },
            ],
            { source: 1, sink: 3 },
          ),
          flowJson(
            11,
            { x: 470, y: 400 },
            [
              { x: 422.5, y: 400 },
              { x: 520, y: 400 },
              { x: 520, y: 300 },
            ],
            { source: 1, sink: 4 },
          ),
          flowJson(
            12,
            { x: 470, y: 410 },
            [
              { x: 422.5, y: 410 },
              { x: 540, y: 410 },
              { x: 540, y: 480 },
            ],
            { source: 1, sink: 5 },
          ),
        ]),
        1,
        kind === 'circle' ? circle({ x: 500, y: 400 }, 100, Math.PI) : line({ x: 400, y: 250 }, { x: 400, y: 550 }),
      ),
    budget: kind === 'circle' ? { 10: 12, 11: 8, 12: 11 } : { 10: 2, 11: 2, 12: 2 },
  })),
  ...FACES.flatMap((face) =>
    (['source', 'sink'] as const).flatMap((end) =>
      [45, 100, 160, 250].map((r) => ({
        name: `a cloud circling its stock, ${face} face, flow as ${end}, r = ${r}`,
        run: () => cloudCircle(face, end, r),
        budget: { 10: cloudCircleBudget(face, end, r) },
        flips: CLIPPED_CORNERS[`${face} ${end} ${r}`],
      })),
    ),
  ),
  ...(
    [
      ['along its axis, out and back in', line({ x: 400, y: 0 }, { x: 40, y: 0 }), 0],
      ['across its axis at the cloud, 200px from the stock', line({ x: 200, y: -150 }, { x: 200, y: 150 }), 4],
      ['across its axis 60px from the stock', line({ x: 60, y: -150 }, { x: 60, y: 150 }), 4],
      ['across its axis behind the stock', line({ x: -80, y: -150 }, { x: -80, y: 150 }), 2],
    ] as const
  ).map(([what, positions, budget]) => ({
    name: `a straight flow's cloud dragged ${what}`,
    run: () => cloudSweep(straightScene({ x: 0, y: 0 }, { x: 200, y: 0 }, 'source'), 10, positions),
    budget: { 10: budget },
  })),
  ...(['source', 'sink'] as const).flatMap((end) =>
    [45, 100, 160].map((r) => ({
      name: `a stock end detached and dragged around its stock, flow as ${end}, r = ${r}`,
      run: () =>
        detachSweep(straightScene({ x: 0, y: 0 }, { x: 200, y: 0 }, end), 10, end, circle({ x: 0, y: 0 }, r, 0)),
      budget: { 10: r === 160 ? 12 : 11 },
    })),
  ),
  ...[45, 100, 160, 250].map((r) => ({
    name: `a flow created from a stock and dragged around it, r = ${r}`,
    run: () => createSweep(loadView([stockJson(1, 0, 0)]), circle({ x: 0, y: 0 }, r, 0)),
    budget: { 10: 12 },
  })),
];

describe('E3 continuity sweeps in 1px frames', () => {
  for (const row of ROWS) {
    it(row.name, () => {
      const result = row.run();
      expect(result.violations.join('\n')).toBe('');
      expect(result.flips.join('\n')).toBe((row.flips ?? []).join('\n'));
      for (const [uid, budget] of Object.entries(row.budget)) {
        expect(`flow ${uid}: ${result.transitions.get(Number(uid)) ?? 0} transitions`).toBe(
          `flow ${uid}: ${budget} transitions`,
        );
      }
    });
  }
});

describe('flip-flops only where a circle clips the corner of a region two G3 minima bound', () => {
  // Pinned literals, independent of the core: MIN_SEGMENT, MIN_SINK_SEGMENT,
  // CORNER_CLEARANCE and the stock's half extents (bottom and right mirror top
  // and left).
  //
  // In face coordinates (along the face from the endpoint, out from the face),
  // the pinned routes routeEnd accepts occupy regions bounded by minima. With the
  // flow as source (the cloud is its sink) the L needs along >= MIN_SINK_SEGMENT
  // and out >= MIN_SEGMENT, and the Z along >= MIN_SEGMENT and out >= both
  // minima. With the flow as sink the L needs along >= MIN_SEGMENT and out >=
  // MIN_SINK_SEGMENT, and no Z shows, since the L is valid wherever it would be.
  // A circle flip-flops when its radius is at least a corner's distance from the
  // stock center and it leaves the region through the out edge while the
  // released straight is still there: along <= the face's slide range (and, for
  // the Z, before the L's edge).
  const m = 10;
  const s = 15.5;
  const clearance = 3;
  function bands(face: 'top' | 'left', end: FlowEnd): Array<readonly [number, number]> {
    const toFace = face === 'top' ? 17.5 : 22.5;
    const slide = (face === 'top' ? 22.5 : 17.5) - clearance;
    const band = (along: number, out: number, until: number): readonly [number, number] => [
      Math.hypot(toFace + out, along),
      Math.hypot(toFace + out, until),
    ];
    const all = end === 'source' ? [band(s, m, slide), band(m, m + s, Math.min(s, slide))] : [band(m, s, slide)];
    return all.filter(([lo, hi]) => hi > lo);
  }
  for (const face of ['top', 'left'] as const) {
    for (const end of ['source', 'sink'] as const) {
      it(`a cloud circling its stock, ${face} face, flow as ${end}, r in [28, 52]`, () => {
        const flipping: number[] = [];
        for (let r = 28; r <= 52; r += 0.25) {
          if (cloudCircle(face, end, r, false).flips.length > 0) flipping.push(r);
        }
        const expected = bands(face, end);
        const outside = flipping.filter((r) => !expected.some(([lo, hi]) => r >= lo && r < hi));
        const empty = expected.filter(([lo, hi]) => !flipping.some((r) => r >= lo && r < hi));
        const fmt = ([lo, hi]: readonly [number, number]): string => `[${lo.toFixed(2)}, ${hi.toFixed(2)})`;
        expect(
          `flipping outside the bands: ${outside.join(' ')}; bands with no flip: ${empty.map(fmt).join(' ')}`,
        ).toBe('flipping outside the bands: ; bands with no flip: ');
      });
    }
  }
});
