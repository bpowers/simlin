// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of `offsetSegment` (flow-geometry/offset-segment.ts).
//
// The main table is the cross product of: path length (2, 3, 4, 6 points), the
// dragged segment's position (first, every interior index, last), the source
// and sink terminal kinds (stock or cloud each), the face the path leaves
// through (all four, so both hold axes are exercised), and the drag direction.
// A 2-point path has one segment, which is both first and last; a 3-point path
// has no interior segment. Every row drags out 40px, then back to the base
// coordinate on the committed result, three times over, and asserts: strict
// invariants after each gesture; the dragged segment holds the coordinate the
// plan determines (the request, or the documented clamp or collapse); stock
// endpoints stay on their base faces and clouds follow their endpoints; the
// valve keeps its coordinate along its own segment while that segment survives
// (an independent oracle below); and, unless the out gesture collapsed a corner,
// dragging back restores the base exactly, so stubs never accumulate.
//
// Separate rows pin the face-extent rule on a straight flow between aligned
// stocks (slide within the faces, hold at the extent within MIN_SEGMENT beyond
// it, a bracket past that), the joint resolution of two adjacent risers, the
// nearest valid side of an obstacle, and the valve when its segment is removed.

import { describe, it, expect } from '@rstest/core';

import type { JsonViewElement } from '@simlin/engine';
import type { FlowViewElement, Point, StockFlowView } from '@simlin/core/datamodel';

import { StockHeight, StockWidth } from '../drawing/default';
import { faceOfEndpoint, FACES, flowTerminals, offsetSegment, segmentHold, type Face } from '../flow-geometry';
import {
  applyGeometry,
  arcOf,
  byUidOf,
  cloudJson,
  directions,
  flowJson,
  flowOf,
  fmtFlow,
  loadView,
  pathLength,
  stockJson,
  hausdorff,
  stockOf,
  strictReport,
  type Pt,
} from './support/flow-geometry-fixtures';

const MIN_SEGMENT = 10;
const MIN_SINK_SEGMENT = 15.5;
const VALVE_CLAMP_MARGIN = 10;
const EPS = 1e-6;

const FLOW = 10;
const SOURCE_STOCK = 1;
const SINK_STOCK = 2;
const SOURCE_CLOUD = 11;
const SINK_CLOUD = 12;
const ORIGIN: Pt = { x: 400, y: 400 };

// Canonical paths leave the source rightward from (0,0). Stubs and risers are
// longer than the minima, so no base tail is a re-solvable stub + riser.
const PATHS: Readonly<Record<number, readonly Pt[]>> = {
  2: [
    { x: 0, y: 0 },
    { x: 200, y: 0 },
  ],
  3: [
    { x: 0, y: 0 },
    { x: 100, y: 0 },
    { x: 100, y: 120 },
  ],
  4: [
    { x: 0, y: 0 },
    { x: 80, y: 0 },
    { x: 80, y: 70 },
    { x: 200, y: 70 },
  ],
  6: [
    { x: 0, y: 0 },
    { x: 40, y: 0 },
    { x: 40, y: -60 },
    { x: 160, y: -60 },
    { x: 160, y: 0 },
    { x: 200, y: 0 },
  ],
};
const LENGTHS = [2, 3, 4, 6] as const;
const KINDS = ['stock', 'cloud'] as const;
type Kind = (typeof KINDS)[number];
const DIRECTIONS = [1, -1] as const;
const OUT = 40;

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

function build(length: number, face: Face, sourceKind: Kind, sinkKind: Kind): StockFlowView {
  const pts = PATHS[length].map((p) => {
    const r = rotate(p, face);
    return { x: ORIGIN.x + r.x, y: ORIGIN.y + r.y };
  });
  const n = pts.length;
  const elements: JsonViewElement[] = [];
  const out0 = rotate({ x: 1, y: 0 }, face);
  if (sourceKind === 'stock') {
    elements.push(stockJson(SOURCE_STOCK, pts[0].x - out0.x * (StockWidth / 2), pts[0].y - out0.y * (StockHeight / 2)));
  } else {
    elements.push(cloudJson(SOURCE_CLOUD, FLOW, pts[0].x, pts[0].y));
  }
  const dx = Math.sign(pts[n - 1].x - pts[n - 2].x);
  const dy = Math.sign(pts[n - 1].y - pts[n - 2].y);
  if (sinkKind === 'stock') {
    elements.push(stockJson(SINK_STOCK, pts[n - 1].x + dx * (StockWidth / 2), pts[n - 1].y + dy * (StockHeight / 2)));
  } else {
    elements.push(cloudJson(SINK_CLOUD, FLOW, pts[n - 1].x, pts[n - 1].y));
  }
  const valve = pointAt(pts, pathLength(pts) * 0.3);
  elements.push(
    flowJson(FLOW, valve, pts, {
      source: sourceKind === 'stock' ? SOURCE_STOCK : SOURCE_CLOUD,
      sink: sinkKind === 'stock' ? SINK_STOCK : SINK_CLOUD,
    }),
  );
  return loadView(elements);
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

const axisOf = (a: Pt, b: Pt): 'x' | 'y' => (Math.abs(a.y - b.y) <= EPS ? 'x' : 'y');
const holdOf = (a: Pt, b: Pt): number => (axisOf(a, b) === 'x' ? a.y : a.x);

/** The segment of `pts` on `axis` holding `hold` whose span covers `along`. */
function findSegment(pts: readonly Pt[], axis: 'x' | 'y', hold: number, along: number): number {
  for (let i = 0; i < pts.length - 1; i++) {
    const a = pts[i];
    const b = pts[i + 1];
    if (axisOf(a, b) !== axis || Math.abs(holdOf(a, b) - hold) > EPS) continue;
    const lo = Math.min(axis === 'x' ? a.x : a.y, axis === 'x' ? b.x : b.y);
    const hi = Math.max(axis === 'x' ? a.x : a.y, axis === 'x' ? b.x : b.y);
    if (along >= lo - EPS && along <= hi + EPS) return i;
  }
  return -1;
}

interface Row {
  readonly name: string;
  readonly length: number;
  readonly index: number;
  readonly position: 'first' | 'interior' | 'last';
  readonly face: Face;
  readonly sourceKind: Kind;
  readonly sinkKind: Kind;
  readonly direction: 1 | -1;
}

const ROWS: Row[] = LENGTHS.flatMap((length) => {
  const segments = length - 1;
  const indexes: Array<[number, Row['position']]> = [[0, 'first']];
  for (let i = 1; i < segments - 1; i++) indexes.push([i, 'interior']);
  if (segments > 1) indexes.push([segments - 1, 'last']);
  return indexes.flatMap(([index, position]) =>
    FACES.flatMap((face) =>
      KINDS.flatMap((sourceKind) =>
        KINDS.flatMap((sinkKind) =>
          DIRECTIONS.map((direction) => ({
            name: `${length} points, ${position} segment ${index}, leaving ${face}, ${sourceKind} -> ${sinkKind}, ${direction > 0 ? '+' : '-'}`,
            length,
            index,
            position,
            face,
            sourceKind,
            sinkKind,
            direction,
          })),
        ),
      ),
    ),
  );
});

/**
 * The hold the plan determines for the out gesture, in canonical coordinates
 * (before rotation): the request, unless an adjacent stock stub would shrink
 * under its minimum (clamped there) or an adjacent cloud segment would shrink
 * to zero (the corner collapses at the cloud's coordinate).
 */
function expectedOutHold(row: Row): { hold: number; collapses: boolean } {
  const canon = PATHS[row.length];
  const i = row.index;
  const baseHold = holdOf(canon[i], canon[i + 1]);
  const requested = baseHold + row.direction * OUT;
  const last = canon.length - 2;
  if (i === 1 && last > 1) {
    // The adjacent first segment leaves the source at canonical x = 0.
    if (row.sourceKind === 'stock') {
      return { hold: Math.max(requested, MIN_SEGMENT), collapses: false };
    }
    return { hold: requested, collapses: Math.abs(requested) <= EPS };
  }
  if (i === last - 1 && i > 0) {
    const sinkCoordinate = canon[canon.length - 1].x;
    const sign = Math.sign(canon[canon.length - 1].x - canon[canon.length - 2].x);
    if (row.sinkKind === 'stock') {
      const bound = sinkCoordinate - sign * MIN_SINK_SEGMENT;
      return { hold: sign > 0 ? Math.min(requested, bound) : Math.max(requested, bound), collapses: false };
    }
    return { hold: requested, collapses: Math.abs(requested - sinkCoordinate) <= EPS };
  }
  return { hold: requested, collapses: false };
}

/**
 * The valve's expected arc position after an offset (an independent oracle for
 * the plan's rule): the valve keeps its coordinate along its own segment while a
 * segment on the same axis holding the same coordinate (the dragged segment's new
 * hold, when the valve rode it) overlaps that segment's span, clamped into the new
 * span; with its segment gone it goes to the nearest point of the new path. The
 * margin is applied last.
 */
function valveExpectation(base: FlowViewElement, i: number, result: FlowViewElement, newHold: number): number {
  const pts = base.points;
  const s0 = arcOf(pts, base);
  let start = 0;
  let j = 0;
  for (; j < pts.length - 2; j++) {
    const length = Math.hypot(pts[j + 1].x - pts[j].x, pts[j + 1].y - pts[j].y);
    if (s0 <= start + length) break;
    start += length;
  }
  const axis = axisOf(pts[j], pts[j + 1]);
  const hold = j === i ? newHold : holdOf(pts[j], pts[j + 1]);
  const along = axis === 'x' ? base.x : base.y;
  const at = (p: Pt): number => (axis === 'x' ? p.x : p.y);
  const spanLo = Math.min(at(pts[j]), at(pts[j + 1]));
  const spanHi = Math.max(at(pts[j]), at(pts[j + 1]));
  const out = result.points;
  let target: Pt = { x: base.x, y: base.y };
  let bestGap = Infinity;
  for (let k = 0; k < out.length - 1; k++) {
    if (axisOf(out[k], out[k + 1]) !== axis || Math.abs(holdOf(out[k], out[k + 1]) - hold) > EPS) continue;
    const lo = Math.min(at(out[k]), at(out[k + 1]));
    const hi = Math.max(at(out[k]), at(out[k + 1]));
    if (Math.min(hi, spanHi) - Math.max(lo, spanLo) < -EPS) continue;
    const v = Math.max(lo, Math.min(hi, along));
    if (Math.abs(v - along) < bestGap) {
      bestGap = Math.abs(v - along);
      target = axis === 'x' ? { x: v, y: hold } : { x: hold, y: v };
    }
  }
  const length = pathLength(out);
  const s = arcOf(out, target);
  return length < 2 * VALVE_CLAMP_MARGIN
    ? length / 2
    : Math.max(VALVE_CLAMP_MARGIN, Math.min(length - VALVE_CLAMP_MARGIN, s));
}

describe('offsetSegment over length x position x face x terminal kinds x direction, dragged out and back', () => {
  it('covers the full cross product', () => {
    // Positions: 1 (2 points) + 2 (3 points) + 3 (4 points) + 5 (6 points).
    expect(ROWS.length).toBe(11 * FACES.length * 4 * 2);
  });

  for (const row of ROWS) {
    it(row.name, () => {
      const view0 = build(row.length, row.face, row.sourceKind, row.sinkKind);
      const base = flowOf(view0, FLOW);
      const { axis, hold: baseHold } = segmentHold(base.points, row.index);

      let view = view0;
      let flow = base;
      let index = row.index;
      const baseAlong =
        axis === 'x'
          ? (base.points[row.index].x + base.points[row.index + 1].x) / 2
          : (base.points[row.index].y + base.points[row.index + 1].y) / 2;
      const expected = expectedOutHold(row);
      const maxPoints = base.points.length + 4;

      for (let cycle = 0; cycle < 3; cycle++) {
        // Out: the hold moves `direction * OUT` in canonical terms, rotated with the path.
        const canonicalHold =
          holdOf(PATHS[row.length][row.index], PATHS[row.length][row.index + 1]) + row.direction * OUT;
        const outHold = realHold(axis, row.face, canonicalHold);
        const terminals = flowTerminals(flow, byUidOf(view));
        const out = offsetSegment(flow, index, outHold, terminals, {});
        view = applyGeometry(view, out);
        const context = `${row.name} cycle ${cycle} out\nbase ${fmtFlow(flow)}\nresult ${fmtFlow(out.flow)}`;
        expect(`${context}\n${strictReport(view, [FLOW])}`).toBe(`${context}\n`);
        expect(out.flow.points.length).toBeLessThanOrEqual(maxPoints);

        const wantHold = realHold(axis, row.face, expected.hold);
        const newIndex = findSegment(out.flow.points, axis, wantHold, baseAlong);
        expect(`${context}\nsegment holding ${wantHold}: ${newIndex >= 0}`).toBe(
          `${context}\nsegment holding ${wantHold}: true`,
        );

        assertTerminals(view0, out.flow, row, context);
        if (cycle === 0) {
          const got = arcOf(out.flow.points, out.flow);
          const want = valveExpectation(flow, index, out.flow, wantHold);
          expect(`${context}\nvalve arc ${got.toFixed(6)}`).toBe(`${context}\nvalve arc ${want.toFixed(6)}`);
        }

        // Back to the base coordinate.
        const back = offsetSegment(out.flow, newIndex, baseHold, flowTerminals(out.flow, byUidOf(view)), {});
        view = applyGeometry(view, back);
        const backContext = `${row.name} cycle ${cycle} back\nfrom ${fmtFlow(out.flow)}\nresult ${fmtFlow(back.flow)}`;
        expect(`${backContext}\n${strictReport(view, [FLOW])}`).toBe(`${backContext}\n`);
        assertTerminals(view0, back.flow, row, backContext);
        if (!expected.collapses) {
          expect(`${backContext}\n${JSON.stringify(back.flow.points.map((p) => [p.x, p.y]))}`).toBe(
            `${backContext}\n${JSON.stringify(base.points.map((p) => [p.x, p.y]))}`,
          );
          flow = back.flow;
          index = findSegment(back.flow.points, axis, baseHold, baseAlong);
        } else {
          // The out gesture removed a corner, so there is nothing to restore;
          // repeated cycles still must not grow the path.
          flow = back.flow;
          index = findSegment(back.flow.points, axis, baseHold, baseAlong);
          if (index < 0) break;
        }
      }
    });
  }
});

/**
 * The real hold coordinate for a canonical hold. A canonical segment is
 * horizontal (holding y) exactly when its real axis matches the face's normal
 * axis after rotation, so the canonical hold is placed on that canonical axis
 * and rotated with the path.
 */
function realHold(axis: 'x' | 'y', face: Face, canonical: number): number {
  const faceNormalIsX = face === 'right' || face === 'left';
  const canonicalHorizontal = (axis === 'x') === faceNormalIsX;
  const p = rotate(canonicalHorizontal ? { x: 0, y: canonical } : { x: canonical, y: 0 }, face);
  return axis === 'x' ? ORIGIN.y + p.y : ORIGIN.x + p.x;
}

function assertTerminals(view0: StockFlowView, f: FlowViewElement, row: Row, context: string): void {
  const n = f.points.length;
  const byUid = byUidOf(view0);
  if (row.sourceKind === 'stock') {
    const stock = byUid.get(SOURCE_STOCK)!;
    expect(`${context}\nsource face ${faceOfEndpoint(stock as never, f.points[0], f.points[1])}`).toBe(
      `${context}\nsource face ${row.face}`,
    );
  }
  if (row.sinkKind === 'stock') {
    const stock = byUid.get(SINK_STOCK)!;
    const baseFlow = flowOf(view0, FLOW);
    const bn = baseFlow.points.length;
    const baseFace = faceOfEndpoint(stock as never, baseFlow.points[bn - 1], baseFlow.points[bn - 2]);
    expect(`${context}\nsink face ${faceOfEndpoint(stock as never, f.points[n - 1], f.points[n - 2])}`).toBe(
      `${context}\nsink face ${baseFace}`,
    );
  }
  expect(f.points[0].attachedToUid).toBe(row.sourceKind === 'stock' ? SOURCE_STOCK : SOURCE_CLOUD);
  expect(f.points[n - 1].attachedToUid).toBe(row.sinkKind === 'stock' ? SINK_STOCK : SINK_CLOUD);
}

describe('the face-extent rule on a straight flow between aligned stocks', () => {
  // Stocks at (0,0) and (200,0): the right face of A is x = 22.5, the left face
  // of B x = 177.5, and both valid ranges are y in [-14.5, 14.5].
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
  const terminals = flowTerminals(base, byUidOf(view));

  const SWEEP: ReadonlyArray<{ readonly c: number; readonly want: readonly Pt[]; readonly why: string }> = [
    { c: 0, want: base.points, why: 'identity' },
    {
      c: 10,
      want: [
        { x: 22.5, y: 10 },
        { x: 177.5, y: 10 },
      ],
      why: 'slides within both faces',
    },
    {
      c: 14.5,
      want: [
        { x: 22.5, y: 14.5 },
        { x: 177.5, y: 14.5 },
      ],
      why: 'at the extent',
    },
    {
      c: 20,
      want: [
        { x: 22.5, y: 14.5 },
        { x: 177.5, y: 14.5 },
      ],
      why: 'within MIN_SEGMENT beyond: held at the extent',
    },
    {
      c: 24.5,
      want: [
        { x: 22.5, y: 14.5 },
        { x: 177.5, y: 14.5 },
      ],
      why: 'exactly MIN_SEGMENT beyond: still held',
    },
    {
      c: 30,
      want: [
        { x: 22.5, y: 14.5 },
        { x: 22.5 + MIN_SEGMENT, y: 14.5 },
        { x: 22.5 + MIN_SEGMENT, y: 30 },
        { x: 177.5 - MIN_SINK_SEGMENT, y: 30 },
        { x: 177.5 - MIN_SINK_SEGMENT, y: 14.5 },
        { x: 177.5, y: 14.5 },
      ],
      why: 'past it: a bracket, stubs at the minima, risers from the extent',
    },
    {
      c: -30,
      want: [
        { x: 22.5, y: -14.5 },
        { x: 22.5 + MIN_SEGMENT, y: -14.5 },
        { x: 22.5 + MIN_SEGMENT, y: -30 },
        { x: 177.5 - MIN_SINK_SEGMENT, y: -30 },
        { x: 177.5 - MIN_SINK_SEGMENT, y: -14.5 },
        { x: 177.5, y: -14.5 },
      ],
      why: 'the other side',
    },
  ];
  for (const row of SWEEP) {
    it(`c = ${row.c}: ${row.why}`, () => {
      const g = offsetSegment(base, 0, row.c, terminals);
      expect(g.flow.points.map((p: Point) => [p.x, p.y])).toEqual(row.want.map((p) => [p.x, p.y]));
      expect(strictReport(applyGeometry(view, g), [FLOW])).toBe('');
    });
  }

  it('dragging the bracket back collapses it, whatever the bracket was built from', () => {
    const bracket = offsetSegment(base, 0, 60, terminals);
    const run = findSegment(bracket.flow.points, 'x', 60, 100);
    for (const c of [0, 5, -14.5]) {
      const back = offsetSegment(
        bracket.flow,
        run,
        c,
        flowTerminals(bracket.flow, byUidOf(applyGeometry(view, bracket))),
      );
      expect(`${c}: ${directions(back.flow.points)}`).toBe(`${c}: R`);
      expect(back.flow.points[0].y).toBe(c);
    }
  });

  it('a dragged segment next to a stock stub is clamped so the stub keeps MIN_SEGMENT', () => {
    // A Z out of A's right face: dragging the riser left into the stub stops at 32.5.
    const z = loadView([
      stockJson(1, 0, 0),
      stockJson(2, 200, 60),
      flowJson(
        FLOW,
        { x: 100, y: 30 },
        [
          { x: 22.5, y: 0 },
          { x: 100, y: 0 },
          { x: 100, y: 60 },
          { x: 177.5, y: 60 },
        ],
        { source: 1, sink: 2 },
      ),
    ]);
    const f = flowOf(z, FLOW);
    const t = flowTerminals(f, byUidOf(z));
    expect(offsetSegment(f, 1, 0, t).flow.points[1].x).toBe(22.5 + MIN_SEGMENT);
    expect(offsetSegment(f, 1, 1000, t).flow.points[1].x).toBe(177.5 - MIN_SINK_SEGMENT);
  });

  it('an interior riser shorter than MIN_SEGMENT snaps to zero (collapse) or out to the minimum, whichever is nearer', () => {
    // Cloud source at (0,0); an L-then-Z path whose second riser can be dragged onto the first's column.
    const v = loadView([
      cloudJson(11, FLOW, 0, 0),
      cloudJson(12, FLOW, 300, 100),
      flowJson(
        FLOW,
        { x: 150, y: 50 },
        [
          { x: 0, y: 0 },
          { x: 100, y: 0 },
          { x: 100, y: 50 },
          { x: 200, y: 50 },
          { x: 200, y: 100 },
          { x: 300, y: 100 },
        ],
        { source: 11, sink: 12 },
      ),
    ]);
    const f = flowOf(v, FLOW);
    const t = flowTerminals(f, byUidOf(v));
    // Segment 2 holds y = 50 between a riser from y = 0 and one to y = 100. A
    // riser under MIN_SEGMENT / 2 collapses (its corner goes, 4 points); one
    // between that and MIN_SEGMENT is pushed out to MIN_SEGMENT (6 points).
    const ROWS: ReadonlyArray<readonly [number, number, number]> = [
      [4, 0, 4],
      [6, 10, 6],
      [94, 90, 6],
      [96, 100, 4],
    ];
    for (const [c, hold, pointCount] of ROWS) {
      const g = offsetSegment(f, 2, c, t);
      expect(`c ${c}: ${g.flow.points.length}`).toBe(`c ${c}: ${pointCount}`);
      expect(strictReport(applyGeometry(v, g), [FLOW])).toBe('');
      const along = findSegment(g.flow.points, 'x', hold, 150);
      expect(`c ${c}: holds ${hold} ${along >= 0}`).toBe(`c ${c}: holds ${hold} true`);
    }
  });
});

describe('offsetSegment: joint constraints, obstacles, and a removed valve segment', () => {
  it('two adjacent risers are resolved jointly: the nearest coordinate satisfying both', () => {
    // Segment 2 holds y = 50 between a riser from y = 0 and one to y = 15. Asked
    // for y = 6: pushing out from the first riser (to 10) would put the second
    // at 5, under its minimum; the nearest coordinate satisfying both collapses
    // the first riser (y = 0), which also leaves the second 15 long.
    const v = loadView([
      cloudJson(11, FLOW, 0, 0),
      cloudJson(12, FLOW, 160, 15),
      flowJson(
        FLOW,
        { x: 75, y: 50 },
        [
          { x: 0, y: 0 },
          { x: 50, y: 0 },
          { x: 50, y: 50 },
          { x: 100, y: 50 },
          { x: 100, y: 15 },
          { x: 160, y: 15 },
        ],
        { source: 11, sink: 12 },
      ),
    ]);
    const f = flowOf(v, FLOW);
    const g = offsetSegment(f, 2, 6, flowTerminals(f, byUidOf(v)));
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [0, 0],
      [100, 0],
      [100, 15],
      [160, 15],
    ]);
    expect(strictReport(applyGeometry(v, g), [FLOW])).toBe('');
  });

  for (const [request, want, why] of [
    [50, 42.5, 'the near side is nearer: the segment rests at the stock`s top edge'],
    [70, 77.5, 'the far side is nearer: the segment jumps across to the bottom edge'],
  ] as const) {
    it(`a cloud dragged into another stock takes the nearest valid side (${why})`, () => {
      // A cloud -> cloud flow whose source cloud sits above a stock (x 77.5..122.5,
      // y 42.5..77.5); dragging the segment down moves both clouds with it.
      const v = loadView([
        stockJson(1, 100, 60),
        cloudJson(11, FLOW, 100, 0),
        cloudJson(12, FLOW, 300, 0),
        flowJson(
          FLOW,
          { x: 200, y: 0 },
          [
            { x: 100, y: 0 },
            { x: 300, y: 0 },
          ],
          { source: 11, sink: 12 },
        ),
      ]);
      const f = flowOf(v, FLOW);
      const g = offsetSegment(f, 0, request, flowTerminals(f, byUidOf(v)), { stocks: [stockOf(v, 1)] });
      // The valid boundary is GEOMETRY_EPSILON outside the stock edge (a cloud on the edge is not inside).
      expect(g.flow.points[0].y).toBeCloseTo(want, 5);
      expect(g.clouds.map((c) => c.uid).sort()).toEqual([11, 12]);
      expect(strictReport(applyGeometry(v, g), [FLOW])).toBe('');
    });
  }

  it('a bracket dragged back straight moves the valve no further than the path moved', () => {
    // The valve rides the bracket's far riser (x = 180). Dragging the run back to
    // the stub's hold collapses both risers: the valve's segment is gone, so it
    // goes to the nearest point of the straight path instead of keeping an arc
    // distance measured past the removed risers.
    const v = loadView([
      stockJson(1, 0, 0),
      cloudJson(12, FLOW, 260, 0),
      flowJson(
        FLOW,
        { x: 180, y: -20 },
        [
          { x: 22.5, y: 0 },
          { x: 32.5, y: 0 },
          { x: 32.5, y: -40 },
          { x: 180, y: -40 },
          { x: 180, y: 0 },
          { x: 260, y: 0 },
        ],
        { source: 1, sink: 12 },
      ),
    ]);
    const f = flowOf(v, FLOW);
    const g = offsetSegment(f, 2, 0, flowTerminals(f, byUidOf(v)));
    expect(directions(g.flow.points)).toBe('R');
    const valveJump = Math.hypot(g.flow.x - f.x, g.flow.y - f.y);
    expect(valveJump).toBeLessThanOrEqual(hausdorff(f.points, g.flow.points) + 1e-9);
    expect([g.flow.x, g.flow.y]).toEqual([180, 0]);
  });
});
