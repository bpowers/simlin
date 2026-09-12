// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of `route` and `routeEnd` (flow-geometry/route.ts).
//
// The main table is the cross product of enumerations: the moving terminal's
// kind (each of the four stock faces, and a free cloud), the base path's shape
// (straight, L, Z, bracket, and a five-segment path), the drag quadrant, which
// end moves, the fixed terminal's kind, and the drag distance (a nudge every
// preserved prefix survives, and a far drag that releases most of them). Every
// row asserts the strict invariants on the routed flow, the attachments, and the
// valve policy (arc-length distance from the fixed end, clamped, margin once).
// Nudge rows additionally pin what the plan determines: the preserved prefix from
// the fixed end is kept exactly, and a moving stock keeps its base face.
//
// Separate tables pin each ranking arm (validity of off-base faces with no
// hysteresis, stickiness within one bend, axis change, length), the pair-owned
// holds (the straight's split preference, a Z's tip midpoint), U detours, which
// preserved tails routeEnd accepts (not crossing, no added bends or U turns), the
// pinned-then-released k = 0 rule, and totality with overlapping stocks.
//
// Not covered here: offsetSegment (flow-geometry-offset.test.ts), heal
// (flow-geometry-heal.test.ts), the primitives (-terminal, -valve, -validity),
// the audit repros (-repros), continuity sweeps (-sweeps), and seeded scenes
// (-fuzz).

import { describe, it, expect } from '@rstest/core';

import type { JsonViewElement } from '@simlin/engine';
import type { FlowViewElement, StockFlowView, StockViewElement } from '@simlin/core/datamodel';

import { StockHeight, StockWidth } from '../drawing/default';
import {
  FACES,
  faceOfEndpoint,
  flowTerminals,
  freeTerminal,
  route,
  routeEnd,
  stockTerminal,
  type Face,
  type FlowEnd,
  type Terminal,
} from '../flow-geometry';
import {
  applyGeometry,
  arcOf,
  byUidOf,
  cloudJson,
  cloudOf,
  directions,
  flowJson,
  flowOf,
  fmtFlow,
  loadView,
  pathLength,
  stockJson,
  stockOf,
  strictReport,
  type Pt,
} from './support/flow-geometry-fixtures';

// Pinned literal, independent of the core's export (drift in either is caught).
const VALVE_CLAMP_MARGIN = 10;

const MOVING = 1;
const FIXED = 2;
const FLOW = 10;
const MOVING_CLOUD = 11;
const FIXED_CLOUD = 12;
const CENTER: Pt = { x: 400, y: 400 };

// Canonical templates: the moving end is the source, at a stock centered at the
// origin whose right face is x = 22.5, leaving rightward. Stubs are at least 20
// so a nudge keeps every preserved prefix feasible.
const SHAPES = {
  straight: [
    { x: 22.5, y: 0 },
    { x: 200, y: 0 },
  ],
  L: [
    { x: 22.5, y: 0 },
    { x: 150, y: 0 },
    { x: 150, y: 150 },
  ],
  Z: [
    { x: 22.5, y: 0 },
    { x: 100, y: 0 },
    { x: 100, y: 80 },
    { x: 250, y: 80 },
  ],
  bracket: [
    { x: 22.5, y: 0 },
    { x: 52.5, y: 0 },
    { x: 52.5, y: -70 },
    { x: 180, y: -70 },
    { x: 180, y: 0 },
    { x: 260, y: 0 },
  ],
  prefix3: [
    { x: 22.5, y: 0 },
    { x: 70, y: 0 },
    { x: 70, y: 90 },
    { x: 170, y: 90 },
    { x: 170, y: -60 },
    { x: 280, y: -60 },
  ],
} as const;
type ShapeName = keyof typeof SHAPES;
const SHAPE_NAMES = Object.keys(SHAPES) as ShapeName[];

const MOVING_KINDS = [...FACES, 'free'] as const;
type MovingKind = (typeof MOVING_KINDS)[number];
const QUADRANTS: ReadonlyArray<readonly [number, number]> = [
  [1, 1],
  [1, -1],
  [-1, 1],
  [-1, -1],
];
const ENDS: readonly FlowEnd[] = ['source', 'sink'];
const FIXED_KINDS = ['cloud', 'stock'] as const;
const DISTANCES = { nudge: 3, far: 140 } as const;
type DistanceName = keyof typeof DISTANCES;

/** Rotate the canonical +x exit onto `face`'s outward direction. */
function rotate(p: Pt, face: Face): Pt {
  switch (face) {
    case 'right':
      return { x: p.x, y: p.y };
    case 'bottom':
      return { x: -p.y, y: p.x };
    case 'left':
      return { x: -p.x, y: -p.y };
    case 'top':
      return { x: p.y, y: -p.x };
  }
}

interface Fixture {
  readonly view: StockFlowView;
  readonly flow: FlowViewElement;
  readonly movingEnd: FlowEnd;
  readonly movingKind: MovingKind;
}

function buildFixture(kind: MovingKind, shape: ShapeName, movingEnd: FlowEnd, fixedKind: 'cloud' | 'stock'): Fixture {
  const face: Face = kind === 'free' ? 'right' : kind;
  const canonical = SHAPES[shape].map((p) => rotate(p, face));
  const pts = canonical.map((p) => ({ x: CENTER.x + p.x, y: CENTER.y + p.y }));
  const elements: JsonViewElement[] = [];
  if (kind === 'free') {
    elements.push(cloudJson(MOVING_CLOUD, FLOW, pts[0].x, pts[0].y));
  } else {
    elements.push(stockJson(MOVING, CENTER.x, CENTER.y));
  }
  const last = pts[pts.length - 1];
  const before = pts[pts.length - 2];
  if (fixedKind === 'cloud') {
    elements.push(cloudJson(FIXED_CLOUD, FLOW, last.x, last.y));
  } else {
    const dx = Math.sign(last.x - before.x);
    const dy = Math.sign(last.y - before.y);
    elements.push(stockJson(FIXED, last.x + dx * (StockWidth / 2), last.y + dy * (StockHeight / 2)));
  }
  const movingUid = kind === 'free' ? MOVING_CLOUD : MOVING;
  const fixedUid = fixedKind === 'cloud' ? FIXED_CLOUD : FIXED;
  const ordered = movingEnd === 'source' ? pts : [...pts].reverse();
  const valve = pointAt(ordered, pathLength(ordered) * 0.4);
  elements.push(
    flowJson(
      FLOW,
      valve,
      ordered,
      movingEnd === 'source' ? { source: movingUid, sink: fixedUid } : { source: fixedUid, sink: movingUid },
    ),
  );
  const view = loadView(elements);
  return { view, flow: flowOf(view, FLOW), movingEnd, movingKind: kind };
}

function pointAt(pts: readonly Pt[], s: number): Pt {
  let remaining = s;
  for (let i = 0; i < pts.length - 1; i++) {
    const length = Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
    if (remaining <= length) {
      const t = remaining / length;
      return { x: pts[i].x + (pts[i + 1].x - pts[i].x) * t, y: pts[i].y + (pts[i + 1].y - pts[i].y) * t };
    }
    remaining -= length;
  }
  return pts[pts.length - 1];
}

/** Move the moving terminal by `delta` the way a planner does: the moved stock keeps the base face and offset. */
function movingTerminal(fx: Fixture, delta: Pt): { terminal: Terminal; moved: StockViewElement | undefined } {
  const f = fx.flow;
  const n = f.points.length;
  const endpoint = fx.movingEnd === 'source' ? f.points[0] : f.points[n - 1];
  const adjacent = fx.movingEnd === 'source' ? f.points[1] : f.points[n - 2];
  if (fx.movingKind === 'free') {
    const cloud = cloudOf(fx.view, MOVING_CLOUD);
    const point = { x: cloud.x + delta.x, y: cloud.y + delta.y };
    return { terminal: freeTerminal(point, cloud), moved: undefined };
  }
  const base = stockOf(fx.view, MOVING);
  const stock = { ...base, x: base.x + delta.x, y: base.y + delta.y };
  return { terminal: stockTerminal(stock, endpoint, adjacent, base), moved: stock };
}

interface Row {
  readonly name: string;
  readonly kind: MovingKind;
  readonly shape: ShapeName;
  readonly quadrant: readonly [number, number];
  readonly end: FlowEnd;
  readonly fixedKind: 'cloud' | 'stock';
  readonly distance: DistanceName;
}

const ROWS: Row[] = MOVING_KINDS.flatMap((kind) =>
  SHAPE_NAMES.flatMap((shape) =>
    QUADRANTS.flatMap((quadrant) =>
      ENDS.flatMap((end) =>
        FIXED_KINDS.flatMap((fixedKind) =>
          (Object.keys(DISTANCES) as DistanceName[]).map((distance) => ({
            name: `${kind} ${shape} q(${quadrant.join(',')}) ${end} moves, fixed ${fixedKind}, ${distance}`,
            kind,
            shape,
            quadrant,
            end,
            fixedKind,
            distance,
          })),
        ),
      ),
    ),
  ),
);

describe('routeEnd over terminal kind x shape x quadrant x end x fixed kind x distance', () => {
  it('covers the full cross product', () => {
    expect(ROWS.length).toBe(MOVING_KINDS.length * SHAPE_NAMES.length * 4 * 2 * 2 * 2);
  });

  for (const row of ROWS) {
    it(row.name, () => {
      const fx = buildFixture(row.kind, row.shape, row.end, row.fixedKind);
      const base = fx.flow;
      const d = DISTANCES[row.distance];
      const delta = { x: row.quadrant[0] * d, y: row.quadrant[1] * d };
      const terminals = flowTerminals(base, byUidOf(fx.view));
      const fixedEnd: FlowEnd = row.end === 'source' ? 'sink' : 'source';
      const fixed = fixedEnd === 'source' ? terminals.source : terminals.sink;
      const { terminal, moved } = movingTerminal(fx, delta);

      const g = routeEnd(base, row.end, terminal, { fixed });
      const view = applyGeometry(fx.view, g, moved === undefined ? [] : [moved]);
      const f = g.flow;
      const n = f.points.length;
      const context = `${row.name}\nbase ${fmtFlow(base)}\nresult ${fmtFlow(f)}`;

      expect(`${context}\n${strictReport(view, [FLOW])}`).toBe(`${context}\n`);

      // Attachments: the moving end to the moving terminal, the fixed end to the fixed one.
      const movingUid = row.kind === 'free' ? MOVING_CLOUD : MOVING;
      const fixedUid = row.fixedKind === 'cloud' ? FIXED_CLOUD : FIXED;
      const movingPoint = row.end === 'source' ? f.points[0] : f.points[n - 1];
      const fixedPoint = row.end === 'source' ? f.points[n - 1] : f.points[0];
      expect(`${context}\n${movingPoint.attachedToUid} ${fixedPoint.attachedToUid}`).toBe(
        `${context}\n${movingUid} ${fixedUid}`,
      );
      // The only cloud a routeEnd moves is the moving terminal's (E4).
      expect(g.clouds.every((c) => c.uid === MOVING_CLOUD)).toBe(true);
      if (row.kind === 'free') {
        expect(movingPoint.x).toBeCloseTo(cloudOf(fx.view, MOVING_CLOUD).x + delta.x, 9);
        expect(movingPoint.y).toBeCloseTo(cloudOf(fx.view, MOVING_CLOUD).y + delta.y, 9);
      }

      // Valve: arc distance from the fixed end preserved, clamped, margin once.
      const baseLength = pathLength(base.points);
      const baseFromFixed = fixedEnd === 'source' ? arcOf(base.points, base) : baseLength - arcOf(base.points, base);
      const length = pathLength(f.points);
      const wantFromFixed =
        length < 2 * VALVE_CLAMP_MARGIN
          ? length / 2
          : Math.max(VALVE_CLAMP_MARGIN, Math.min(length - VALVE_CLAMP_MARGIN, Math.min(baseFromFixed, length)));
      const gotFromFixed = fixedEnd === 'source' ? arcOf(f.points, f) : length - arcOf(f.points, f);
      expect(`${context}\nvalve ${gotFromFixed.toFixed(6)}`).toBe(`${context}\nvalve ${wantFromFixed.toFixed(6)}`);

      if (row.distance !== 'nudge') {
        return;
      }
      // A nudge keeps the preserved prefix from the fixed end exactly: K all but
      // the corner adjacent to the moving end, and at K = 0 the fixed endpoint.
      const K = Math.max(0, base.points.length - 3);
      const fromFixed = (pts: readonly Pt[]): Pt[] => (fixedEnd === 'source' ? [...pts] : [...pts].reverse());
      const keptBase = fromFixed(base.points).slice(0, K + 1);
      const keptResult = fromFixed(f.points).slice(0, K + 1);
      if (!(row.kind === 'free' && row.shape === 'straight')) {
        expect(`${context}\n${JSON.stringify(keptResult.map((p) => [p.x, p.y]))}`).toBe(
          `${context}\n${JSON.stringify(keptBase.map((p) => [p.x, p.y]))}`,
        );
      }
      if (row.kind !== 'free') {
        // Stickiness: a nudged stock keeps its base face and the path its shape.
        const stock = moved!;
        const movingAdjacent = row.end === 'source' ? f.points[1] : f.points[n - 2];
        expect(`${context}\n${faceOfEndpoint(stock, movingPoint, movingAdjacent)}`).toBe(`${context}\n${row.kind}`);
        expect(`${context}\n${directions(f.points)}`).toBe(`${context}\n${directions(base.points)}`);
      } else if (row.shape === 'straight') {
        // A free end nudged off a straight flow's axis: pinned, nothing valid is
        // straight, L or Z (the offset is under MIN_SEGMENT), so the flow is
        // released. A fixed stock slides its endpoint along its face and stays
        // straight; a fixed cloud cannot slide, and the detour is a U turn.
        if (row.fixedKind === 'stock') {
          expect(`${context}\n${n}`).toBe(`${context}\n2`);
        } else {
          expect(`${context}\n${n}`).toBe(`${context}\n4`);
        }
      }
    });
  }
});

// ---------------------------------------------------------------------------
// Ranking arms

function scene(elements: JsonViewElement[]): StockFlowView {
  return loadView(elements);
}

function faces(view: StockFlowView, f: FlowViewElement): string {
  const byUid = byUidOf(view);
  const n = f.points.length;
  const at = (i: number, j: number): string => {
    const el = byUid.get(f.points[i].attachedToUid ?? -1);
    return el?.type === 'stock' ? (faceOfEndpoint(el, f.points[i], f.points[j]) ?? 'offFace') : 'free';
  };
  return `${at(0, 1)}->${at(n - 1, n - 2)}`;
}

describe('route ranking', () => {
  // Stock A at (0,0) with a base straight out of its right face. Only the base's
  // face, offset and end axes matter to route; the cloud is where the drag put it.
  const A = stockJson(1, 0, 0);
  const baseStraight = (): JsonViewElement =>
    flowJson(
      FLOW,
      { x: 60, y: 0 },
      [
        { x: 22.5, y: 0 },
        { x: 120, y: 0 },
      ],
      { source: 1, sink: 12 },
    );

  interface RankRow {
    readonly name: string;
    readonly cloud: Pt;
    readonly want: string;
    readonly why: string;
  }
  // No hysteresis: a pure function has no previous frame to be sticky against,
  // and stickiness already holds the base face. With the base face (right)
  // invalid, the cloud up and to the left has two off-base one-bend candidates
  // that change one end axis each, and they compete on validity and length only.
  const OFF_BASE: RankRow[] = [
    {
      name: 'an off-base face whose stub clears MIN_SEGMENT wins on length',
      cloud: { x: -40, y: -60 },
      want: 'left->free',
      why: 'left stub 17.5 >= 10; left L is 77.5 long, top L 82.5',
    },
    {
      name: 'an off-base face whose stub would be under MIN_SEGMENT is invalid',
      cloud: { x: -30, y: -60 },
      want: 'top->free',
      why: 'left stub 7.5 < 10',
    },
  ];
  for (const row of OFF_BASE) {
    it(row.name, () => {
      const view = scene([A, cloudJson(12, FLOW, row.cloud.x, row.cloud.y), baseStraight()]);
      const base = flowOf(view, FLOW);
      const t = flowTerminals(base, byUidOf(view));
      const g = route(t.source, freeTerminal(row.cloud, cloudOf(view, 12)), { flow: base });
      const out = applyGeometry(view, g);
      expect(`${row.why}: ${faces(out, g.flow)} ${fmtFlow(g.flow)}`).toBe(`${row.why}: ${row.want} ${fmtFlow(g.flow)}`);
      expect(strictReport(out, [FLOW])).toBe('');
    });
  }

  it('keeps both base faces when they have a valid candidate within one bend of the best (stickiness)', () => {
    // A right -> B left is a Z (two bends); A bottom -> B left is an L (one
    // bend). The base faces are within one bend of the best, so the Z is kept.
    const view = scene([
      A,
      stockJson(2, 100, 60),
      flowJson(
        FLOW,
        { x: 50, y: 30 },
        [
          { x: 22.5, y: 0 },
          { x: 50, y: 0 },
          { x: 50, y: 60 },
          { x: 77.5, y: 60 },
        ],
        { source: 1, sink: 2 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const g = route(t.source, t.sink, { flow: base });
    const out = applyGeometry(view, g);
    expect(`${faces(out, g.flow)} ${directions(g.flow.points)}`).toBe('right->left RDR');
    expect(strictReport(out, [FLOW])).toBe('');
    // Without base faces (a fresh route between the same stocks) an L wins on
    // bends. Two Ls tie on bends, axis change and length here; which one is the
    // generation-order tie break, deliberately not pinned.
    const fresh = route(stockTerminal(stockOf(view, 1)), stockTerminal(stockOf(view, 2)), {
      flow: { ...base, points: [] },
    });
    expect(fresh.flow.points.length).toBe(3);
    expect(strictReport(applyGeometry(view, fresh), [FLOW])).toBe('');
  });

  it('prefers the candidate keeping the base end axes (axis change)', () => {
    // Cloud to cloud, base an L leaving horizontally and arriving vertically.
    // The sink moves: a horizontal-first and a vertical-first L tie on bends and
    // length; only the horizontal-first one keeps both end axes.
    const view = scene([
      cloudJson(11, FLOW, 0, 0),
      cloudJson(12, FLOW, 100, 100),
      flowJson(
        FLOW,
        { x: 50, y: 0 },
        [
          { x: 0, y: 0 },
          { x: 100, y: 0 },
          { x: 100, y: 100 },
        ],
        { source: 11, sink: 12 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const g = routeEnd(base, 'sink', freeTerminal({ x: 160, y: 140 }, cloudOf(view, 12)), { fixed: t.source });
    expect(directions(g.flow.points)).toBe('RD');
    expect(g.flow.points[1]).toEqual({ x: 160, y: 0, attachedToUid: undefined });
  });

  it('prefers a route crossing neither body over sticky faces and axes, even where G6 excuses the crossing', () => {
    // A straight from A's right face into B's left face; B is dragged over A's
    // left side, so the inflated bodies overlap and a crossing is no fault. Out of
    // A's left face into B's left face keeps both base end axes and B's face, but
    // runs through B; over the top of both, out of A's top face into B's, crosses
    // neither and wins.
    const view = loadView([
      stockJson(1, 0, 0),
      stockJson(2, 222.5, 0),
      flowJson(
        FLOW,
        { x: 200, y: 0 },
        [
          { x: 22.5, y: 0 },
          { x: 200, y: 0 },
        ],
        { source: 1, sink: 2 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const B = stockOf(view, 2);
    const moved = { ...B, x: -40, y: 8 };
    const g = routeEnd(base, 'sink', stockTerminal(moved, base.points[1], base.points[0], B), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe('valve(-40,-19.5) (0,-17.5)@1 (0,-19.5) (-40,-19.5) (-40,-9.5)@2');
    expect(strictReport(applyGeometry(view, g, [moved]), [FLOW])).toBe('');
  });

  it('breaks a tie on bends and axis change by length (the shorter U detour)', () => {
    // Clouds 4px off each other's line: nothing straight, L or Z is valid, so a
    // U detour; the one whose riser turns toward the sink's side is shorter.
    const view = scene([
      cloudJson(11, FLOW, 0, 0),
      cloudJson(12, FLOW, 100, 4),
      flowJson(
        FLOW,
        { x: 50, y: 0 },
        [
          { x: 0, y: 0 },
          { x: 100, y: 0 },
        ],
        { source: 11, sink: 12 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const g = route(t.source, t.sink, { flow: base });
    const out = applyGeometry(view, g);
    expect(strictReport(out, [FLOW])).toBe('');
    // Up, across, down into the sink: the run holds 4 - 15.5 = -11.5 (an 11.5px
    // stub, 127 long) rather than 4 + 15.5 = 19.5 below (a 19.5px stub, 135 long).
    expect(directions(g.flow.points)).toBe('URD');
    expect(g.flow.points[1].y).toBeCloseTo(-11.5, 9);
  });
});

describe('route: straights, Zs and U detours', () => {
  const draft = (): FlowViewElement => flowOf(loadView([flowJson(FLOW, { x: 0, y: 0 }, [], {})]), FLOW);

  it('a straight between two faces splits the difference between their preferences', () => {
    // Fresh prefs are the face centers, y = 0 and y = 10; both ranges hold y = 5.
    const view = loadView([stockJson(1, 0, 0), stockJson(2, 200, 10)]);
    const g = route(stockTerminal(stockOf(view, 1)), stockTerminal(stockOf(view, 2)), { flow: draft() });
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [22.5, 5],
      [177.5, 5],
    ]);
  });

  it('a Z with no base corners holds its riser midway between the two stub tips, not the two faces', () => {
    // A straight base from A's right face into B's left face; B moves to (100, 40).
    // No straight fits and the L into B's top face is off-base, so stickiness
    // keeps both faces as a Z. The base has no corner to keep, so the riser sits
    // between A's tip 22.5 + 10 and B's tip 77.5 - 15.5, not between the faces.
    const view = loadView([
      stockJson(1, 0, 0),
      stockJson(2, 200, 0),
      flowJson(
        FLOW,
        { x: 100, y: 0 },
        [
          { x: 22.5, y: 0 },
          { x: 177.5, y: 0 },
        ],
        { source: 1, sink: 2 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const B = stockOf(view, 2);
    const moved = { ...B, x: 100, y: 40 };
    const g = routeEnd(base, 'sink', stockTerminal(moved, base.points[1], base.points[0], B), { fixed: t.source });
    expect(directions(g.flow.points)).toBe('RDR');
    expect(g.flow.points[1].x).toBeCloseTo((32.5 + 62) / 2, 9);
    expect(strictReport(applyGeometry(view, g, [moved]), [FLOW])).toBe('');
  });

  for (const [dy, want, runY] of [
    [4, 'URD', -11.5],
    [-4, 'DRU', 11.5],
  ] as const) {
    it(`clouds ${dy}px off each other's line: the shorter U detour (${want}), whichever side it is on`, () => {
      // Nothing straight, L or Z is valid; the U whose run passes on the sink's
      // side needs the shorter riser into the sink.
      const view = loadView([
        cloudJson(11, FLOW, 0, 0),
        cloudJson(12, FLOW, 100, dy),
        flowJson(
          FLOW,
          { x: 50, y: 0 },
          [
            { x: 0, y: 0 },
            { x: 100, y: 0 },
          ],
          { source: 11, sink: 12 },
        ),
      ]);
      const base = flowOf(view, FLOW);
      const t = flowTerminals(base, byUidOf(view));
      const g = route(t.source, t.sink, { flow: base });
      expect(directions(g.flow.points)).toBe(want);
      expect(g.flow.points[1].y).toBeCloseTo(runY, 9);
      expect(strictReport(applyGeometry(view, g), [FLOW])).toBe('');
    });
  }
});

describe('routeEnd: which preserved tails are accepted', () => {
  it('a tail that would run alongside its stock is not accepted; the flow is released instead', () => {
    // Base: out of the bottom face, down, left, down into a cloud. The cloud is
    // dragged above the stock, just right of its right face (401.5): preserving
    // the first corner would give down, right, up past the body, hugging it.
    const view = loadView([
      stockJson(1, 379, 350),
      cloudJson(3, FLOW, 359, 431.5),
      flowJson(
        FLOW,
        { x: 379, y: 400.5 },
        [
          { x: 379, y: 367.5 },
          { x: 379, y: 410.5 },
          { x: 359, y: 410.5 },
          { x: 359, y: 431.5 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const g = routeEnd(base, 'sink', freeTerminal({ x: 401.9, y: 223.4 }, cloudOf(view, 3)), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe(
      fmtFlow({
        ...g.flow,
        points: [
          { x: 379, y: 332.5, attachedToUid: 1 },
          { x: 379, y: 223.4, attachedToUid: undefined },
          { x: 401.9, y: 223.4, attachedToUid: 3 },
        ],
      }),
    );
    expect(strictReport(applyGeometry(view, g), [FLOW])).toBe('');
  });

  it('a tail that adds a U turn the base did not have is refused', () => {
    // Base: cloud, down, right, down into a stock's top face (no U turn). The sink
    // detaches to a point above the run: preserving two corners would give down,
    // right, UP to the point, a U turn; the released route is right, down.
    const view = loadView([
      stockJson(1, 395.5, 503),
      cloudJson(3, FLOW, 344.5, 361.5),
      flowJson(
        FLOW,
        { x: 344.5, y: 445.3 },
        [
          { x: 344.5, y: 361.5 },
          { x: 344.5, y: 461.5 },
          { x: 395.5, y: 461.5 },
          { x: 395.5, y: 485.5 },
        ],
        { source: 3, sink: 1 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const newCloud = {
      type: 'cloud' as const,
      uid: 21,
      flowUid: FLOW,
      x: 395.5,
      y: 485.5,
      isZeroRadius: false,
      ident: undefined,
    };
    const g = routeEnd(base, 'sink', freeTerminal({ x: 393, y: 444 }, newCloud), { fixed: t.source });
    expect(directions(g.flow.points)).toBe('RD');
  });

  it('a tail that gives the path more bends than the base is refused', () => {
    // Base: a Z from stock 1's right face into stock 5's left face (2 bends).
    // Stock 5 moved down-left under the run: keeping the base's left face would
    // take right, down, left, down, right (4 bends); the released route is an L.
    const view = loadView([
      stockJson(1, 503, 382),
      stockJson(5, 659, 451.5),
      flowJson(
        FLOW,
        { x: 551.5, y: 447 },
        [
          { x: 525.5, y: 383.5 },
          { x: 551.5, y: 383.5 },
          { x: 551.5, y: 451.5 },
          { x: 636.5, y: 451.5 },
        ],
        { source: 1, sink: 5 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const S5 = stockOf(view, 5);
    for (let k = 0; k <= 24; k++) {
      const moved = { ...S5, x: 659 - (184.3 * k) / 24, y: 451.5 + (106.2 * k) / 24 };
      const g = routeEnd(base, 'sink', stockTerminal(moved, base.points[3], base.points[2], S5), { fixed: t.source });
      expect(`k ${k} ${directions(g.flow.points)}`).toMatch(/^k \d+ (RDR|RD|D)$/);
      expect(strictReport(applyGeometry(view, g, [moved]), [FLOW])).toBe('');
    }
  });

  it('a tail crossing its stock is refused even where G6 excuses the crossing', () => {
    // Base: a U out of the right face, down, back left under the stock into a
    // cloud. The cloud is dragged within MIN_SEGMENT of the stock's inflated left
    // side, so G6 excuses crossings: preserving the first corner would run the
    // tail back through the stock at y = 10, a valid path routeEnd still refuses.
    // Released, the endpoint takes the left face straight to the cloud.
    const view = loadView([
      stockJson(1, 0, 0),
      cloudJson(3, FLOW, -60, 80),
      flowJson(
        FLOW,
        { x: 80, y: 0 },
        [
          { x: 22.5, y: 0 },
          { x: 80, y: 0 },
          { x: 80, y: 80 },
          { x: -60, y: 80 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const g = routeEnd(base, 'sink', freeTerminal({ x: -40, y: 10 }, cloudOf(view, 3)), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe('valve(-31.25,10) (-22.5,10)@1 (-40,10)@3');
    expect(strictReport(applyGeometry(view, g), [FLOW])).toBe('');
  });

  it('a tail may take three bends when the bend budget allows', () => {
    // Base: the bracket (four bends) into a cloud, dragged 2.5px left of the first
    // riser. Preserving the first corner, no one- or two-bend tail meets the
    // minima; up, right, up, left (each at its minimum) does, and keeps four bends.
    const view = loadView([
      stockJson(1, 0, 0),
      cloudJson(3, FLOW, 260, 0),
      flowJson(FLOW, { x: 52.5, y: 0 }, [...SHAPES.bracket], { source: 1, sink: 3 }),
    ]);
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const g = routeEnd(base, 'sink', freeTerminal({ x: 50, y: -70 }, cloudOf(view, 3)), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe('valve(52.5,0) (22.5,0)@1 (52.5,0) (52.5,-10) (65.5,-10) (65.5,-70) (50,-70)@3');
    expect(strictReport(applyGeometry(view, g), [FLOW])).toBe('');
  });

  it('a moving stock keeps its base face through a three-bend tail within one bend of an off-base face', () => {
    // Base: a hook (four bends) from stock 1's right face into stock 2's left face.
    // Stock 2 is dragged under the first corner: preserving it, the top face takes
    // a two-bend tail, and the base left face a three-bend tail. Stickiness reaches
    // one bend past the best, so the three-bend tier is generated and the left face
    // kept.
    const view = hook();
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const B = stockOf(view, 2);
    const moved = { ...B, x: 80, y: 20 };
    const g = routeEnd(base, 'sink', stockTerminal(moved, base.points[5], base.points[4], B), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe('valve(60,0) (22.5,0)@1 (60,0) (60,-10) (42,-10) (42,20) (57.5,20)@2');
    expect(strictReport(applyGeometry(view, g, [moved]), [FLOW])).toBe('');
  });

  it('a tail running back alongside its preserved prefix within MIN_SEGMENT is refused', () => {
    // Base: the hook; stock 2 is dragged beside the first riser (x = 60). The
    // terminals leave no room, so G3 excuses short segments. Preserving two
    // corners, the shortest tail steps out 4.5px and runs back up beside the
    // riser: valid, but under the minima, so routeEnd would not accept it and
    // would release to an L. Refusing it inside the search lets the tail that
    // steps out a full MIN_SEGMENT win instead, and that one is accepted.
    const view = hook();
    const base = flowOf(view, FLOW);
    const t = flowTerminals(base, byUidOf(view));
    const B = stockOf(view, 2);
    const moved = { ...B, x: 32, y: 44 };
    const g = routeEnd(base, 'sink', stockTerminal(moved, base.points[5], base.points[4], B), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe('valve(60,0) (22.5,0)@1 (60,0) (60,60) (70,60) (70,44) (54.5,44)@2');
    expect(strictReport(applyGeometry(view, g, [moved]), [FLOW])).toBe('');
  });
});

/** A hook (four bends) from stock 1's right face into stock 2's left face. */
function hook(): StockFlowView {
  const points = [
    { x: 22.5, y: 0 },
    { x: 60, y: 0 },
    { x: 60, y: 60 },
    { x: -60, y: 60 },
    { x: -60, y: -60 },
    { x: 100, y: -60 },
  ];
  return loadView([
    stockJson(1, 0, 0),
    stockJson(2, 122.5, -60),
    flowJson(FLOW, points[1], points, { source: 1, sink: 2 }),
  ]);
}

describe('routeEnd k = 0: pinned, then released', () => {
  const A = stockJson(1, 0, 0);
  const view = loadView([
    A,
    cloudJson(12, FLOW, 200, 0),
    flowJson(
      FLOW,
      { x: 100, y: 0 },
      [
        { x: 22.5, y: 0 },
        { x: 200, y: 0 },
      ],
      { source: 1, sink: 12 },
    ),
  ]);
  const base = flowOf(view, FLOW);
  const t = flowTerminals(base, byUidOf(view));

  const ROWS: ReadonlyArray<{ readonly dy: number; readonly want: string; readonly why: string }> = [
    { dy: 0, want: 'R @0', why: 'identity' },
    { dy: 5, want: 'R @5', why: 'pinned has nothing valid (riser under MIN_SEGMENT); released, the endpoint slides' },
    { dy: 12, want: 'RDR @0', why: 'pinned Z valid: the fixed endpoint stays put, the riser hugs the cloud' },
    { dy: 40, want: 'RD @0', why: 'pinned L valid' },
  ];
  for (const row of ROWS) {
    it(`cloud dragged down ${row.dy}: ${row.why}`, () => {
      const cloud = cloudOf(view, 12);
      const g = routeEnd(base, 'sink', freeTerminal({ x: 200, y: row.dy }, cloud), { fixed: t.source });
      const out = applyGeometry(view, g);
      expect(strictReport(out, [FLOW])).toBe('');
      expect(`${directions(g.flow.points)} @${g.flow.points[0].y}`).toBe(row.want);
    });
  }

  it('the pinned Z riser sits MIN_SINK_SEGMENT short of the cloud', () => {
    const g = routeEnd(base, 'sink', freeTerminal({ x: 200, y: 12 }, cloudOf(view, 12)), { fixed: t.source });
    expect(g.flow.points[1].x).toBeCloseTo(200 - 15.5, 9);
  });

  it('a pinned route under minima that G3 excuses is not accepted; released, the endpoint slides', () => {
    // The cloud 7.5px beyond the face and 10px below the base line: the terminals
    // leave no room, so every minimum is excused and the pinned L (7.5 across, 10
    // down) is valid. Accepting it would flip to the pinned L at the room
    // boundary; released, the least-bend route is the straight.
    const g = routeEnd(base, 'sink', freeTerminal({ x: 30, y: 10 }, cloudOf(view, 12)), { fixed: t.source });
    expect(fmtFlow(g.flow)).toBe('valve(26.25,10) (22.5,10)@1 (30,10)@12');
    expect(strictReport(applyGeometry(view, g), [FLOW])).toBe('');
  });
});

describe('totality with overlapping stocks', () => {
  const OFFSETS = [0, 5, 20, 45, 54.9] as const;
  for (const kind of FACES) {
    for (const dx of OFFSETS) {
      it(`moving stock (base ${kind}) ${dx}px from the fixed stock returns a route holding G1-G5`, () => {
        const face = kind;
        const exit = rotate({ x: 22.5, y: 0 }, face);
        const far = rotate({ x: 200, y: 0 }, face);
        const view = loadView([
          stockJson(1, 0, 0),
          stockJson(2, far.x + rotate({ x: 22.5, y: 0 }, face).x, far.y + rotate({ x: 22.5, y: 0 }, face).y),
          flowJson(FLOW, { x: (exit.x + far.x) / 2, y: (exit.y + far.y) / 2 }, [exit, far], { source: 1, sink: 2 }),
        ]);
        const base = flowOf(view, FLOW);
        const t = flowTerminals(base, byUidOf(view));
        const B = stockOf(view, 2);
        const A = stockOf(view, 1);
        const moved = { ...A, x: B.x + dx, y: B.y };
        const g = routeEnd(base, 'source', stockTerminal(moved, base.points[0], base.points[1], A), {
          fixed: t.sink,
        });
        const out = applyGeometry(view, g, [moved]);
        const all = [g.flow.x, g.flow.y, ...g.flow.points.flatMap((p) => [p.x, p.y])];
        expect(all.every(Number.isFinite)).toBe(true);
        // The bodies, each inflated by MIN_SEGMENT, overlap: G6 and the G3
        // minima are exempt, and everything else must still hold.
        expect(`${fmtFlow(g.flow)}\n${strictReport(out, [FLOW])}`).toBe(`${fmtFlow(g.flow)}\n`);
      });
    }
  }
});
