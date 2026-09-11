// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The flow-geometry defects the editing audit found, each as a row through the
// core operation the planner will call for that gesture. Scenes and pointer
// positions are the audit's minimal repros; each row asserts the strict
// invariants on the routed flow and the geometry the plan determines.
//
// Repros that are the planner's to fix, not the core's, and so are not rows
// here: C0 (a click commits nothing: E1), R12 and R14 (a drop on the flow's own
// terminal is an invalid target: E6), R13b's preview/commit disagreement (E2,
// one function by construction), and the zero-delta commit identities T8 (E1).
// The core half of R13b (the new sink lands exactly at the pointer) is a row.

import { describe, it, expect } from '@rstest/core';

import type { CloudViewElement, FlowViewElement, StockFlowView } from '@simlin/core/datamodel';

import {
  faceOfEndpoint,
  flowTerminals,
  freeTerminal,
  offsetSegment,
  route,
  routeEnd,
  slideValve,
  stockTerminal,
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
  hausdorff,
  loadView,
  stockJson,
  stockOf,
  strictReport,
  type Pt,
} from './support/flow-geometry-fixtures';

const pts = (g: { flow: FlowViewElement }): number[][] => g.flow.points.map((p) => [p.x, p.y]);

function clean(view: StockFlowView, flowUid: number, g: { flow: FlowViewElement }): void {
  expect(`${fmtFlow(g.flow)}\n${strictReport(view, [flowUid])}`).toBe(`${fmtFlow(g.flow)}\n`);
}

// S1 at (100,100): right face x = 122.5.
const straightToCloud = (cloudAt: Pt): StockFlowView =>
  loadView([
    stockJson(1, 100, 100),
    cloudJson(3, 10, cloudAt.x, cloudAt.y),
    flowJson(10, { x: 200, y: 100 }, [{ x: 122.5, y: 100 }, cloudAt], { source: 1, sink: 3 }),
  ]);

describe('audit lead A / A2: a sink reattached to a column-aligned stock lands on a face, not at its center', () => {
  const ROWS: ReadonlyArray<{
    readonly name: string;
    readonly cloud: Pt;
    readonly target: Pt;
    readonly want: number[][];
  }> = [
    {
      name: 'A (perpendicular-dominant drag)',
      cloud: { x: 300, y: 100 },
      target: { x: 130, y: 300 },
      want: [
        [122.5, 100],
        [132.5, 100],
        [132.5, 282.5],
      ],
    },
    {
      name: 'A2 (parallel-dominant drag)',
      cloud: { x: 400, y: 100 },
      target: { x: 130, y: 250 },
      want: [
        [122.5, 100],
        [132.5, 100],
        [132.5, 232.5],
      ],
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const view = loadView([
        ...straightToCloud(row.cloud).elements.map(toJson),
        stockJson(2, row.target.x, row.target.y),
      ]);
      const f = flowOf(view, 10);
      const t = flowTerminals(f, byUidOf(view));
      const g = routeEnd(f, 'sink', stockTerminal(stockOf(view, 2)), { fixed: t.source });
      const out = applyGeometry(view, g, [], [3]);
      clean(out, 10, g);
      expect(pts(g)).toEqual(row.want);
      expect(g.flow.points[2].attachedToUid).toBe(2);
      expect(faceOfEndpoint(stockOf(view, 2), g.flow.points[2], g.flow.points[1])).toBe('top');
    });
  }
});

function toJson(el: StockFlowView['elements'][number]): Parameters<typeof loadView>[0][number] {
  switch (el.type) {
    case 'stock':
      return stockJson(el.uid, el.x, el.y);
    case 'cloud':
      return cloudJson(el.uid, el.flowUid, el.x, el.y);
    case 'flow':
      return flowJson(el.uid, el, el.points, {
        source: el.points[0].attachedToUid,
        sink: el.points[el.points.length - 1].attachedToUid,
      });
    default:
      throw new Error(`unexpected ${el.type}`);
  }
}

describe('audit lead B: a three-point flow reattached onto a stock takes the face its last segment reaches', () => {
  const ROWS: ReadonlyArray<{
    readonly name: string;
    readonly points: Pt[];
    readonly stock: Pt;
    readonly want: number[][];
    readonly face: string;
  }> = [
    {
      name: 'B: stock to the right of the corner: the left face',
      points: [
        { x: 0, y: 0 },
        { x: 0, y: 100 },
        { x: 200, y: 100 },
      ],
      stock: { x: 300, y: 100 },
      want: [
        [0, 0],
        [0, 100],
        [277.5, 100],
      ],
      face: 'left',
    },
    {
      name: 'B-mirror: stock below the corner: the top face',
      points: [
        { x: 0, y: 0 },
        { x: 100, y: 0 },
        { x: 100, y: 200 },
      ],
      stock: { x: 100, y: 300 },
      want: [
        [0, 0],
        [100, 0],
        [100, 282.5],
      ],
      face: 'top',
    },
    {
      name: 'B-contrast: stock to the left of the corner: the right face',
      points: [
        { x: 0, y: 0 },
        { x: 0, y: 100 },
        { x: 200, y: 100 },
      ],
      stock: { x: -100, y: 100 },
      want: [
        [0, 0],
        [0, 100],
        [-77.5, 100],
      ],
      face: 'right',
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const last = row.points[2];
      const view = loadView([
        stockJson(2, row.stock.x, row.stock.y),
        cloudJson(3, 10, 0, 0),
        cloudJson(4, 10, last.x, last.y),
        flowJson(10, row.points[1], row.points, { source: 3, sink: 4 }),
      ]);
      const f = flowOf(view, 10);
      const t = flowTerminals(f, byUidOf(view));
      const g = routeEnd(f, 'sink', stockTerminal(stockOf(view, 2)), { fixed: t.source });
      clean(applyGeometry(view, g, [], [4]), 10, g);
      expect(pts(g)).toEqual(row.want);
      expect(faceOfEndpoint(stockOf(view, 2), g.flow.points[2], g.flow.points[1])).toBe(row.face);
    });
  }
});

describe('audit lead C: a source detached to empty space puts the cloud under the pointer', () => {
  it('press at x = 132 (in the source hit area), release at x = 60: the endpoint moves 72px, the cloud is at the endpoint', () => {
    const view = straightToCloud({ x: 300, y: 100 });
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const newCloud: CloudViewElement = {
      type: 'cloud',
      uid: 20,
      flowUid: 10,
      x: 0,
      y: 0,
      isZeroRadius: false,
      ident: undefined,
    };
    // The endpoint keeps its grab offset: base endpoint + (release - press).
    const pointer = { x: 122.5 + (60 - 132), y: 100 };
    const g = routeEnd(f, 'source', freeTerminal(pointer, newCloud), { fixed: t.sink });
    clean(applyGeometry(view, g), 10, g);
    expect(pts(g)).toEqual([
      [50.5, 100],
      [300, 100],
    ]);
    expect(g.clouds).toEqual([{ ...newCloud, x: 50.5, y: 100 }]);
  });
});

describe('audit lead D and #819: offsetting a flow between two stocks forms a bracket with stubs, not risers along the faces', () => {
  const view = loadView([
    stockJson(1, 100, 100),
    stockJson(2, 300, 100),
    flowJson(
      10,
      { x: 200, y: 100 },
      [
        { x: 122.5, y: 100 },
        { x: 277.5, y: 100 },
      ],
      { source: 1, sink: 2 },
    ),
  ]);
  const f = flowOf(view, 10);
  const t = flowTerminals(f, byUidOf(view));

  it('D: the valve dragged down 40', () => {
    const g = offsetSegment(f, 0, 140, t);
    clean(applyGeometry(view, g), 10, g);
    expect(pts(g)).toEqual([
      [122.5, 114.5],
      [132.5, 114.5],
      [132.5, 140],
      [262, 140],
      [262, 114.5],
      [277.5, 114.5],
    ]);
  });

  it('#819 follow-up: nudging a stock keeps the bracket and the endpoint on its face', () => {
    const bracket = offsetSegment(f, 0, 140, t);
    const bview = applyGeometry(view, bracket);
    const bf = bracket.flow;
    const A = stockOf(bview, 1);
    const moved = { ...A, x: 80 };
    const g = routeEnd(bf, 'source', stockTerminal(moved, bf.points[0], bf.points[1], A), {
      fixed: flowTerminals(bf, byUidOf(bview)).sink,
    });
    clean(applyGeometry(bview, g, [moved]), 10, g);
    expect(directions(g.flow.points)).toBe(directions(bf.points));
    expect(faceOfEndpoint(moved, g.flow.points[0], g.flow.points[1])).toBe('right');
    expect(g.flow.points.slice(2)).toEqual(bf.points.slice(2));
  });
});

describe('audit L1-L5: a stock -> cloud flow with its valve or cloud dragged off axis', () => {
  const view = straightToCloud({ x: 300, y: 100 });
  const f = flowOf(view, 10);
  const t = flowTerminals(f, byUidOf(view));

  it('L1: the valve dragged down 30 offsets the pipe: a stub and a riser, never a riser along the face', () => {
    const g = offsetSegment(f, 0, 130, t);
    clean(applyGeometry(view, g), 10, g);
    expect(pts(g)).toEqual([
      [122.5, 114.5],
      [132.5, 114.5],
      [132.5, 130],
      [300, 130],
    ]);
    expect(g.clouds.map((c) => [c.uid, c.x, c.y])).toEqual([[3, 300, 130]]);
  });

  const DRAGS: ReadonlyArray<{ readonly name: string; readonly to: Pt; readonly want: number[][] }> = [
    {
      name: 'L2/L3: the cloud dragged down 30: an L turning at the cloud',
      to: { x: 300, y: 130 },
      want: [
        [122.5, 100],
        [300, 100],
        [300, 130],
      ],
    },
    {
      name: 'L4: the cloud moved (+5,+1): the endpoint slides, no diagonal',
      to: { x: 305, y: 101 },
      want: [
        [122.5, 101],
        [305, 101],
      ],
    },
    {
      name: 'L5: the cloud moved (+50,+3): no 3px riser',
      to: { x: 350, y: 103 },
      want: [
        [122.5, 103],
        [350, 103],
      ],
    },
  ];
  for (const row of DRAGS) {
    it(row.name, () => {
      const g = routeEnd(f, 'sink', freeTerminal(row.to, cloudOf(view, 3)), { fixed: t.source });
      clean(applyGeometry(view, g), 10, g);
      expect(pts(g)).toEqual(row.want);
    });
  }
});

describe('audit R8: moving a stock up keeps the valve continuous on an L', () => {
  it('the valve moves at most one frame step per 1px of stock travel', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 40, 300),
      flowJson(
        10,
        { x: 60, y: 100 },
        [
          { x: 77.5, y: 100 },
          { x: 40, y: 100 },
          { x: 40, y: 300 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const A = stockOf(view, 1);
    let prev: FlowViewElement | undefined;
    let worst = 0;
    for (let d = 0; d <= 30; d++) {
      const moved = { ...A, y: 100 - d };
      const g = routeEnd(f, 'source', stockTerminal(moved, f.points[0], f.points[1], A), { fixed: t.sink });
      clean(applyGeometry(view, g, [moved]), 10, g);
      if (prev !== undefined) {
        worst = Math.max(worst, Math.hypot(g.flow.x - prev.x, g.flow.y - prev.y));
      }
      prev = g.flow;
    }
    // One px of travel moves the corner one px; a valve on the run moves with it (sqrt 2).
    expect(worst).toBeLessThanOrEqual(Math.SQRT2 + 1e-9);
  });
});

describe('audit R9 and R16: a route never crosses the fixed end`s stock', () => {
  it('R9: B moved left under A', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      stockJson(2, 300, 200),
      flowJson(
        10,
        { x: 200, y: 100 },
        [
          { x: 300, y: 182.5 },
          { x: 300, y: 100 },
          { x: 122.5, y: 100 },
        ],
        { source: 2, sink: 1 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const B = stockOf(view, 2);
    for (let x = 300; x >= 110; x -= 5) {
      const moved = { ...B, x };
      const g = routeEnd(f, 'source', stockTerminal(moved, f.points[0], f.points[1], B), { fixed: t.sink });
      clean(applyGeometry(view, g, [moved]), 10, g);
    }
  });

  it('R16: an L flow`s source cloud dragged up 40', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 250, 150),
      flowJson(
        10,
        { x: 170, y: 150 },
        [
          { x: 250, y: 150 },
          { x: 90, y: 150 },
          { x: 90, y: 117.5 },
        ],
        { source: 3, sink: 1 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const g = routeEnd(f, 'source', freeTerminal({ x: 250, y: 110 }, cloudOf(view, 3)), { fixed: t.sink });
    clean(applyGeometry(view, g), 10, g);
    // Pinned to the bottom face it would need a U turn; released, it reaches the right face straight.
    expect(pts(g)).toEqual([
      [250, 110],
      [122.5, 110],
    ]);
  });
});

describe('audit R10 and lead probe F: an unmoved terminal changes nothing (no re-spread, no snap to center)', () => {
  it('R10: an off-center L endpoint', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 200, 300),
      flowJson(
        10,
        { x: 200, y: 200 },
        [
          { x: 122.5, y: 95 },
          { x: 200, y: 95 },
          { x: 200, y: 300 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const A = stockOf(view, 1);
    const g = routeEnd(f, 'source', stockTerminal(A, f.points[0], f.points[1], A), { fixed: t.sink });
    expect(pts(g)).toEqual(f.points.map((p) => [p.x, p.y]));
    expect(g.flow.x).toBeCloseTo(f.x, 9);
    expect(g.flow.y).toBeCloseTo(f.y, 9);
    // R10b: a real drag keeps the offset instead of snapping the endpoint to the face center.
    const moved = { ...A, x: 106 };
    const nudged = routeEnd(f, 'source', stockTerminal(moved, f.points[0], f.points[1], A), { fixed: t.sink });
    expect(nudged.flow.points[0]).toEqual({ x: 128.5, y: 95, attachedToUid: 1 });
  });

  it('probe F: two endpoints 5px apart on one face are not re-spread', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 300, 90),
      cloudJson(4, 11, 300, 95),
      flowJson(
        10,
        { x: 200, y: 90 },
        [
          { x: 122.5, y: 90 },
          { x: 300, y: 90 },
        ],
        { source: 1, sink: 3 },
      ),
      flowJson(
        11,
        { x: 200, y: 95 },
        [
          { x: 122.5, y: 95 },
          { x: 300, y: 95 },
        ],
        { source: 1, sink: 4 },
      ),
    ]);
    const A = stockOf(view, 1);
    for (const uid of [10, 11]) {
      const f = flowOf(view, uid);
      const t = flowTerminals(f, byUidOf(view));
      const g = routeEnd(f, 'source', stockTerminal(A, f.points[0], f.points[1], A), { fixed: t.sink });
      expect(pts(g)).toEqual(f.points.map((p) => [p.x, p.y]));
    }
  });
});

describe('audit R11: a stock nudged past a straight flow`s column leaves no sub-pixel stub', () => {
  it('stock nudged right 5 under a vertical flow into its top face', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 82, 0),
      flowJson(
        10,
        { x: 82, y: 40 },
        [
          { x: 82, y: 0 },
          { x: 82, y: 82.5 },
        ],
        { source: 3, sink: 1 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const A = stockOf(view, 1);
    for (const dx of [1, 2, 3, 4, 5, 6, 10]) {
      const moved = { ...A, x: 100 + dx };
      const g = routeEnd(f, 'sink', stockTerminal(moved, f.points[1], f.points[0], A), { fixed: t.source });
      clean(applyGeometry(view, g, [moved]), 10, g);
    }
  });
});

describe('audit R13: flows created from a stock', () => {
  it('R13: the pointer still inside the source stock: a route exists and holds G1-G5', () => {
    const view = loadView([stockJson(1, 445, 479)]);
    const draft = { ...flowOf(loadView([flowJson(10, { x: 445, y: 479 }, [], {})]), 10) };
    const sink: CloudViewElement = {
      type: 'cloud',
      uid: 3,
      flowUid: 10,
      x: 445,
      y: 484,
      isZeroRadius: false,
      ident: undefined,
    };
    const g = route(stockTerminal(stockOf(view, 1)), freeTerminal({ x: 445, y: 484 }, sink), { flow: draft });
    clean(applyGeometry(view, g, [sink]), 10, g);
  });

  it('R13b (core half): the sink lands exactly at the pointer', () => {
    const view = loadView([stockJson(1, 200, 200)]);
    const draft = flowOf(loadView([flowJson(10, { x: 200, y: 200 }, [], {})]), 10);
    const sink: CloudViewElement = {
      type: 'cloud',
      uid: 3,
      flowUid: 10,
      x: 315,
      y: 210,
      isZeroRadius: false,
      ident: undefined,
    };
    const g = route(stockTerminal(stockOf(view, 1)), freeTerminal({ x: 315, y: 210 }, sink), { flow: draft });
    clean(applyGeometry(view, g, [sink]), 10, g);
    expect(pts(g)).toEqual([
      [222.5, 210],
      [315, 210],
    ]);
  });
});

describe('audit R17: dragging a Z middle past the stock face stops at the stub minimum', () => {
  it('the pipe drag is clamped so the stub keeps MIN_SEGMENT', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 250, 300),
      flowJson(
        10,
        { x: 175, y: 200 },
        [
          { x: 100, y: 117.5 },
          { x: 100, y: 200 },
          { x: 250, y: 200 },
          { x: 250, y: 300 },
        ],
        {
          source: 1,
          sink: 3,
        },
      ),
    ]);
    const f = flowOf(view, 10);
    const g = offsetSegment(f, 1, 105, flowTerminals(f, byUidOf(view)));
    clean(applyGeometry(view, g), 10, g);
    expect(pts(g)).toEqual([
      [100, 117.5],
      [100, 127.5],
      [250, 127.5],
      [250, 300],
    ]);
  });
});

describe('audit R18: a valve dragged diagonally off an L slides continuously, never hopping segments', () => {
  it('sweeping the pointer diagonally moves the valve at most 1px of arc per 1px step', () => {
    const view = loadView([
      stockJson(1, 100, 100),
      cloudJson(3, 10, 300, 300),
      flowJson(
        10,
        { x: 200, y: 100 },
        [
          { x: 122.5, y: 100 },
          { x: 300, y: 100 },
          { x: 300, y: 300 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const f = flowOf(view, 10);
    let prev = arcOf(f.points, f);
    for (let k = 1; k <= 60; k++) {
      const g = slideValve(f, { x: k, y: k });
      const arc = arcOf(f.points, g);
      expect(Math.abs(arc - prev)).toBeLessThanOrEqual(1 + 1e-9);
      prev = arc;
    }
    expect([slideValve(f, { x: 60, y: 60 }).x, slideValve(f, { x: 60, y: 60 }).y]).toEqual([260, 100]);
  });
});

describe('audit R19: stocks touching or overlapping still route (totality)', () => {
  for (const [name, x] of [
    ['R19a: faces meeting', 155],
    ['R19b: A moved into B', 170],
  ] as const) {
    it(name, () => {
      const view = loadView([
        stockJson(1, 100, 100),
        stockJson(2, 200, 100),
        flowJson(
          10,
          { x: 150, y: 100 },
          [
            { x: 122.5, y: 100 },
            { x: 177.5, y: 100 },
          ],
          { source: 1, sink: 2 },
        ),
      ]);
      const f = flowOf(view, 10);
      const t = flowTerminals(f, byUidOf(view));
      const A = stockOf(view, 1);
      const moved = { ...A, x };
      const g = routeEnd(f, 'source', stockTerminal(moved, f.points[0], f.points[1], A), { fixed: t.sink });
      clean(applyGeometry(view, g, [moved]), 10, g);
    });
  }
});

describe('#53: a cloud dragged perpendicular keeps the valve continuous except at the documented transitions', () => {
  it('sweeping the sink cloud down 200px in 1px frames', () => {
    const view = loadView([
      stockJson(1, 100, 200),
      cloudJson(3, 10, 300, 200),
      flowJson(
        10,
        { x: 200, y: 200 },
        [
          { x: 122.5, y: 200 },
          { x: 300, y: 200 },
        ],
        { source: 1, sink: 3 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const valveJumps: number[] = [];
    const pathJumps: number[] = [];
    let prev: FlowViewElement | undefined;
    for (let dy = 0; dy <= 200; dy++) {
      const g = routeEnd(f, 'sink', freeTerminal({ x: 300, y: 200 + dy }, cloudOf(view, 3)), { fixed: t.source });
      clean(applyGeometry(view, g), 10, g);
      if (prev !== undefined) {
        if (Math.hypot(g.flow.x - prev.x, g.flow.y - prev.y) > 1 + 1e-9) valveJumps.push(dy);
        if (hausdorff(prev.points, g.flow.points) > 1.5) pathJumps.push(dy);
      }
      prev = g.flow;
    }
    // Under MIN_SEGMENT the endpoint slides (valve follows); at 10 the pinned Z
    // becomes valid and the endpoint returns to its base offset (the valve jumps
    // back 9px); at 16 (MIN_SINK_SEGMENT past) the Z's riser moves onto the
    // cloud as the L becomes valid. Nothing else jumps.
    expect(valveJumps).toEqual([10]);
    expect(pathJumps).toEqual([10, 16]);
  });
});

describe('#818: a non-finite valve on the base flow routes to finite geometry', () => {
  it('routeEnd and route place the valve at the midpoint', () => {
    const view = straightToCloud({ x: 300, y: 100 });
    const f = { ...flowOf(view, 10), x: NaN };
    const t = flowTerminals(f, byUidOf(view));
    const moved = { ...stockOf(view, 1), y: 90 };
    const a = routeEnd(f, 'source', stockTerminal(moved, f.points[0], f.points[1], stockOf(view, 1)), {
      fixed: t.sink,
    });
    const b = route(t.source, t.sink, { flow: f });
    for (const g of [a, b]) {
      expect([g.flow.x, g.flow.y].every(Number.isFinite)).toBe(true);
    }
  });
});

describe('#832: grabbing a source cloud without moving leaves an off-center valve where it is', () => {
  it('zero-delta source cloud grab', () => {
    const view = loadView([
      stockJson(1, 300, 200),
      cloudJson(2, 10, 100, 200),
      flowJson(
        10,
        { x: 160, y: 200 },
        [
          { x: 100, y: 200 },
          { x: 277.5, y: 200 },
        ],
        { source: 2, sink: 1 },
      ),
    ]);
    const f = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const g = routeEnd(f, 'source', freeTerminal({ x: 100, y: 200 }, cloudOf(view, 2)), { fixed: t.sink });
    expect(pts(g)).toEqual(f.points.map((p) => [p.x, p.y]));
    expect(g.flow.x).toBeCloseTo(160, 9);
    expect(g.flow.y).toBe(200);
    expect(g.clouds).toEqual([]);
  });
});
