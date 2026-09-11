// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of paths and the valve (flow-geometry/path.ts): normalize, the valve
// policy (arc position from an end, clamped, margin applied once), slideValve
// and translate.

import { describe, it, expect } from '@rstest/core';

import type { FlowViewElement, Point } from '@simlin/core/datamodel';

import {
  flowTerminals,
  normalize,
  placeValve,
  routeEnd,
  slideValve,
  stockTerminal,
  translate,
  valveDistance,
} from '../flow-geometry';
import {
  arcOf,
  byUidOf,
  cloudJson,
  flowJson,
  flowOf,
  loadView,
  pathLength,
  stockJson,
  stockOf,
  type Pt,
} from './support/flow-geometry-fixtures';

const P = (x: number, y: number, attachedToUid?: number): Point => ({ x, y, attachedToUid });

describe('normalize', () => {
  const ROWS: ReadonlyArray<{ readonly name: string; readonly input: Point[]; readonly want: Pt[] }> = [
    {
      name: 'two points untouched',
      input: [P(0, 0, 1), P(0, 0, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 0, y: 0 },
      ],
    },
    {
      name: 'valid L untouched',
      input: [P(0, 0, 1), P(10, 0), P(10, 10, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
        { x: 10, y: 10 },
      ],
    },
    {
      name: 'collinear interior removed',
      input: [P(0, 0, 1), P(5, 0), P(10, 0, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
      ],
    },
    {
      name: 'zero-length interior removed',
      input: [P(0, 0, 1), P(0, 0), P(10, 0), P(10, 10, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
        { x: 10, y: 10 },
      ],
    },
    {
      name: 'zero-length at the end removed',
      input: [P(0, 0, 1), P(10, 0), P(10, 0, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
      ],
    },
    {
      name: 'removal that exposes collinearity repeats to a fixed point',
      input: [P(0, 0, 1), P(10, 0), P(10, 0), P(10, 10), P(10, 10), P(20, 10), P(30, 10, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
        { x: 10, y: 10 },
        { x: 30, y: 10 },
      ],
    },
    {
      name: 'a collapsed riser merges its neighbors',
      input: [P(0, 0, 1), P(10, 0), P(10, 0), P(20, 0, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 20, y: 0 },
      ],
    },
    {
      name: 'within GEOMETRY_EPSILON counts as equal',
      input: [P(0, 0, 1), P(10, 1e-7), P(20, 0, 2)],
      want: [
        { x: 0, y: 0 },
        { x: 20, y: 0 },
      ],
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const out = normalize(row.input);
      expect(out.map((p) => ({ x: p.x, y: p.y }))).toEqual(row.want);
      expect(out[0].attachedToUid).toBe(row.input[0].attachedToUid);
      expect(out[out.length - 1].attachedToUid).toBe(row.input[row.input.length - 1].attachedToUid);
      expect(normalize(out)).toEqual(out);
    });
  }
});

describe('placeValve and valveDistance', () => {
  const L: Pt[] = [
    { x: 0, y: 0 },
    { x: 100, y: 0 },
    { x: 100, y: 50 },
  ];
  const ROWS: ReadonlyArray<{
    readonly name: string;
    readonly points: Pt[];
    readonly from: 'source' | 'sink';
    readonly d: number | undefined;
    readonly want: Pt;
  }> = [
    { name: 'from the source', points: L, from: 'source', d: 40, want: { x: 40, y: 0 } },
    { name: 'from the sink, crossing the corner', points: L, from: 'sink', d: 70, want: { x: 80, y: 0 } },
    { name: 'clamped to the path, then the margin', points: L, from: 'source', d: 1000, want: { x: 100, y: 40 } },
    { name: 'inside the margin at the start', points: L, from: 'source', d: 3, want: { x: 10, y: 0 } },
    { name: 'no base distance: the midpoint', points: L, from: 'source', d: undefined, want: { x: 75, y: 0 } },
    {
      name: 'a path shorter than two margins: the midpoint',
      points: [
        { x: 0, y: 0 },
        { x: 18, y: 0 },
      ],
      from: 'source',
      d: 2,
      want: { x: 9, y: 0 },
    },
    {
      name: 'exactly two margins long: the single valid position',
      points: [
        { x: 0, y: 0 },
        { x: 20, y: 0 },
      ],
      from: 'sink',
      d: 0,
      want: { x: 10, y: 0 },
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const got = placeValve(row.points, row.from, row.d);
      expect(got.x).toBeCloseTo(row.want.x, 9);
      expect(got.y).toBeCloseTo(row.want.y, 9);
    });
  }

  it('valveDistance measures from either end, never applies the margin, and is undefined with no length', () => {
    expect(valveDistance(L, { x: 3, y: 1 }, 'source')).toBeCloseTo(3, 9);
    expect(valveDistance(L, { x: 40, y: 3 }, 'sink')).toBeCloseTo(110, 9);
    expect(
      valveDistance(
        [
          { x: 5, y: 5 },
          { x: 5, y: 5 },
        ],
        { x: 5, y: 5 },
        'source',
      ),
    ).toBeUndefined();
    expect(valveDistance(L, { x: NaN, y: 0 }, 'source')).toBeUndefined();
  });

  it('the margin is applied once: a base valve inside the margin is measured raw, so growing the far end does not push it inward', () => {
    // An imported valve 3px from the source endpoint; the source stock moves 5px
    // away, the sink is fixed. From the sink the valve is 174.5 along; on the new
    // 182.5 path that is 8 from the source, clamped to the margin: 10. Measuring
    // with the margin would place it 15 from the source.
    const view = loadView([
      stockJson(1, 0, 0),
      cloudJson(3, 10, 200, 0),
      flowJson(
        10,
        { x: 25.5, y: 0 },
        [
          { x: 22.5, y: 0 },
          { x: 200, y: 0 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const A = stockOf(view, 1);
    const moved = { ...A, x: -5 };
    const g = routeEnd(f, 'source', stockTerminal(moved, f.points[0], f.points[1], A), { fixed: t.sink });
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [17.5, 0],
      [200, 0],
    ]);
    expect(g.flow.x).toBeCloseTo(27.5, 9);
  });
});

describe('slideValve', () => {
  const base: FlowViewElement = flowOf(
    loadView([
      cloudJson(11, 10, 0, 0),
      cloudJson(12, 10, 100, 100),
      flowJson(
        10,
        { x: 50, y: 0 },
        [
          { x: 0, y: 0 },
          { x: 100, y: 0 },
          { x: 100, y: 100 },
        ],
        { source: 11, sink: 12 },
      ),
    ]),
    10,
  );
  const ROWS: ReadonlyArray<{ readonly name: string; readonly delta: Pt; readonly want: Pt }> = [
    { name: 'along its segment', delta: { x: 20, y: 0 }, want: { x: 70, y: 0 } },
    { name: 'backwards along its segment', delta: { x: -20, y: 0 }, want: { x: 30, y: 0 } },
    { name: 'perpendicular to its segment: no movement', delta: { x: 0, y: 30 }, want: { x: 50, y: 0 } },
    { name: 'past the corner, continuing down the next segment', delta: { x: 80, y: 80 }, want: { x: 100, y: 30 } },
    {
      name: 'into the corner with the next segment pointing back: rests at the corner',
      delta: { x: 80, y: -80 },
      want: { x: 100, y: 0 },
    },
    { name: 'clamped to the margin at the end of the path', delta: { x: 500, y: 500 }, want: { x: 100, y: 90 } },
    { name: 'clamped to the margin at the start of the path', delta: { x: -500, y: 0 }, want: { x: 10, y: 0 } },
    { name: 'a non-finite delta: unchanged', delta: { x: NaN, y: 0 }, want: { x: 50, y: 0 } },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const got = slideValve(base, row.delta);
      expect(got.points).toBe(base.points);
      expect(got.x).toBeCloseTo(row.want.x, 9);
      expect(got.y).toBeCloseTo(row.want.y, 9);
    });
  }

  it('starting on a corner, it takes the segment the delta moves it along', () => {
    const onCorner = { ...base, x: 100, y: 0 };
    expect([slideValve(onCorner, { x: 0, y: 25 }).x, slideValve(onCorner, { x: 0, y: 25 }).y]).toEqual([100, 25]);
    expect([slideValve(onCorner, { x: -25, y: 0 }).x, slideValve(onCorner, { x: -25, y: 0 }).y]).toEqual([75, 0]);
  });

  it('is continuous in the delta (the time left, not the vector left, carries past a corner)', () => {
    let worst = 0;
    for (let dx = -120; dx <= 120; dx += 1) {
      let prev: FlowViewElement | undefined;
      for (let dy = -120; dy <= 120; dy += 1) {
        const got = slideValve(base, { x: dx, y: dy });
        if (prev !== undefined) {
          worst = Math.max(worst, Math.abs(arcOf(base.points, got) - arcOf(base.points, prev)));
        }
        prev = got;
      }
    }
    expect(worst).toBeLessThanOrEqual(1 + 1e-9);
  });
});

describe('translate', () => {
  const f = flowOf(
    loadView([
      cloudJson(11, 10, 0, 0),
      cloudJson(12, 10, 100, 0),
      flowJson(
        10,
        { x: 50, y: 0 },
        [
          { x: 0, y: 0 },
          { x: 100, y: 0 },
        ],
        { source: 11, sink: 12 },
      ),
    ]),
    10,
  );
  it('moves every point and the valve, keeping attachments', () => {
    const g = translate(f, { x: 7, y: -3 });
    expect([g.x, g.y]).toEqual([57, -3]);
    expect(g.points).toEqual([P(7, -3, 11), P(107, -3, 12)]);
    expect(pathLength(g.points)).toBe(pathLength(f.points));
  });
  it('a non-finite delta returns the flow unchanged', () => {
    expect(translate(f, { x: Infinity, y: 0 })).toBe(f);
  });
});
