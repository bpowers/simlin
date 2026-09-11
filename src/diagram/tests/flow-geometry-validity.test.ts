// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of path classification (flow-geometry/validity.ts, through flowFault):
// one row per RouteFault arm, and rows at each precondition boundary: G3's
// "terminals leave room" at the exact touching distance and just inside it, and
// G6's "bodies apart" with a crossing path when the bodies overlap.

import { describe, it, expect } from '@rstest/core';

import type { JsonViewElement } from '@simlin/engine';

import { flowFault, flowTerminals, type RouteFault } from '../flow-geometry';
import { byUidOf, cloudJson, flowJson, flowOf, loadView, stockJson, type Pt } from './support/flow-geometry-fixtures';

interface Row {
  readonly name: string;
  readonly elements: JsonViewElement[];
  readonly points: Pt[];
  readonly attach: { readonly source?: number; readonly sink?: number };
  readonly stocks?: Pt[];
  readonly want: RouteFault;
}

const S1 = stockJson(1, 100, 100);
const cloudAt = (p: Pt): JsonViewElement => cloudJson(3, 10, p.x, p.y);

// Stock A at the origin and B to its right at `bx`: A's right face is x = 22.5
// and B's left face x = bx - 22.5. A inflated by MIN_SEGMENT (maxX 32.5) and B
// inflated by MIN_SINK_SEGMENT (minX bx - 38) touch exactly at bx = 70.5.
function zBetween(bx: number): Pick<Row, 'elements' | 'points' | 'attach'> {
  return {
    elements: [stockJson(1, 0, 0), stockJson(2, bx, 5)],
    points: [
      { x: 22.5, y: 0 },
      { x: 40, y: 0 },
      { x: 40, y: 5 },
      { x: bx - 22.5, y: 5 },
    ],
    attach: { source: 1, sink: 2 },
  };
}

const ROWS: readonly Row[] = [
  {
    name: 'none: a straight stock -> cloud flow',
    elements: [S1, cloudAt({ x: 300, y: 100 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 300, y: 100 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'none',
  },
  {
    name: 'crossing: the cloud sits inside another stock of the view',
    elements: [S1, cloudAt({ x: 300, y: 100 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 300, y: 100 },
    ],
    attach: { source: 1, sink: 3 },
    stocks: [{ x: 305, y: 100 }],
    want: 'crossing',
  },
  {
    name: 'crossing: a path through its own terminal stock, bodies apart',
    elements: [S1, cloudAt({ x: 40, y: 160 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 140, y: 100 },
      { x: 140, y: 110 },
      { x: 40, y: 110 },
      { x: 40, y: 160 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'crossing',
  },
  {
    name: 'short: a final segment under MIN_SINK_SEGMENT',
    elements: [S1, cloudAt({ x: 200, y: 110 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 200, y: 100 },
      { x: 200, y: 110 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'short',
  },
  {
    name: 'structure: a diagonal segment',
    elements: [S1, cloudAt({ x: 300, y: 110 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 300, y: 110 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'structure',
  },
  {
    name: 'structure: two consecutive collinear segments',
    elements: [S1, cloudAt({ x: 300, y: 100 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 200, y: 100 },
      { x: 300, y: 100 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'structure',
  },
  {
    name: 'structure: leaving the face inward',
    elements: [S1, cloudAt({ x: 60, y: 100 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 60, y: 100 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'structure',
  },
  {
    name: 'structure: an endpoint inside the corner clearance',
    elements: [S1, cloudAt({ x: 300, y: 116 })],
    points: [
      { x: 122.5, y: 116 },
      { x: 300, y: 116 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'structure',
  },
  {
    name: 'structure: a stock endpoint on no face',
    elements: [S1, cloudAt({ x: 300, y: 100 })],
    points: [
      { x: 130, y: 100 },
      { x: 300, y: 100 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'structure',
  },
  { name: 'G3 room at exactly touching bodies: the minima apply (short)', ...zBetween(70.5), want: 'short' },
  { name: 'G3 room just inside touching: the minima are excused (none)', ...zBetween(70.49), want: 'none' },
  {
    name: 'G6 with overlapping bodies: a crossing path is best effort, not a fault (none)',
    // A cloud inside its own flow's stock: the bodies overlap, so neither the
    // crossing nor the G3 minima are faults.
    elements: [S1, cloudAt({ x: 105, y: 100 })],
    points: [
      { x: 122.5, y: 100 },
      { x: 140, y: 100 },
      { x: 140, y: 120 },
      { x: 105, y: 120 },
      { x: 105, y: 100 },
    ],
    attach: { source: 1, sink: 3 },
    want: 'none',
  },
];

describe('flowFault', () => {
  for (const row of ROWS) {
    it(row.name, () => {
      const valve = row.points[0];
      const view = loadView([...row.elements, flowJson(10, valve, row.points, row.attach)]);
      const f = flowOf(view, 10);
      expect(flowFault(f, flowTerminals(f, byUidOf(view)), row.stocks)).toBe(row.want);
    });
  }

  it('covers every RouteFault', () => {
    expect([...new Set(ROWS.map((r) => r.want))].sort()).toEqual(['crossing', 'none', 'short', 'structure']);
  });
});
