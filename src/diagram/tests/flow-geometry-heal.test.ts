// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of `heal` (flow-geometry/heal.ts).
//
// Rows are derived from the generator's IMPORT_SHAPES: every corpus shape is
// found across seeds and healed through the production terminals; each shape
// states what heal repairs and what it cannot (an unattached flow has nothing to
// attach to; a flow listed in two stocks is a model-list shape with no geometry).
// Every shape is also healed twice (idempotence), and every flow of strict scenes
// is healed to itself (identity on a valid flow). Separate rows pin the corner
// tie-break, the stocks context (G6's cloud clause), degenerate input, and a
// seeded sweep of junk flows.

import { describe, it, expect } from '@rstest/core';

import type { FlowViewElement, Point, StockFlowView } from '@simlin/core/datamodel';

import {
  faceOfEndpoint,
  flowFault,
  flowTerminals,
  freeTerminal,
  heal,
  offsetSegment,
  route,
  routeEnd,
  slideValve,
  stockTerminal,
} from '../flow-geometry';
import { checkFlowInvariants } from './support/flow-invariants';
import {
  applyGeometry,
  byUidOf,
  cloudJson,
  cloudOf,
  flowJson,
  flowOf,
  fmtFlow,
  loadView,
  stockJson,
  stockOf,
  strictReport,
} from './support/flow-geometry-fixtures';
import { genImportedScene, genScene, IMPORT_SHAPES, Rng, type ImportShape } from './support/scene-generator';

const P = (x: number, y: number, attachedToUid?: number): Point => ({ x, y, attachedToUid });
const CORNER_CLEARANCE = 3;

interface ShapeExpectation {
  /** Strict arms still reported on the healed flow, one per occurrence. */
  readonly remaining: readonly string[];
  /** Whether heal returns the input unchanged. */
  readonly identity: boolean;
  readonly why: string;
}

const EXPECTATIONS: Readonly<Record<ImportShape, ShapeExpectation>> = {
  offFaceAxis: { remaining: [], identity: false, why: 'the endpoint is re-pinned to the nearest valid face point' },
  cornerEndpoint: {
    remaining: [],
    identity: false,
    why: 'the endpoint moves inside the corner clearance along its face',
  },
  cloudOffset: { remaining: [], identity: false, why: 'the cloud moves onto its endpoint' },
  valveOffPath: { remaining: [], identity: false, why: 'the valve is projected onto the path' },
  slightlyDiagonal: { remaining: [], identity: false, why: 'the segment is snapped along its dominant axis' },
  unattachedFlow: {
    remaining: ['G1.unattachedEndpoint', 'G1.unattachedEndpoint'],
    identity: true,
    why: 'no cloud exists to attach to; creating one is the planner`s job',
  },
  listedInTwoStocks: { remaining: [], identity: true, why: 'a model-list shape: the geometry is valid' },
};

const SEEDS = 200;

function stocksOf(view: StockFlowView): Array<{ x: number; y: number }> {
  return view.elements.filter((e) => e.type === 'stock');
}

describe('heal over every import shape', () => {
  for (const shape of IMPORT_SHAPES) {
    const want = EXPECTATIONS[shape];
    it(`${shape}: ${want.why}`, () => {
      let seen = 0;
      for (let seed = 1; seed <= SEEDS; seed++) {
        const scene = genImportedScene(new Rng(seed));
        for (const m of scene.mutations.filter((mm) => mm.shape === shape)) {
          seen++;
          const view = scene.view;
          const base = flowOf(view, m.flowUid);
          const t = flowTerminals(base, byUidOf(view));
          const g = heal(base, t, { stocks: stocksOf(view) });
          const healed = applyGeometry(view, g);
          const context = `seed ${seed} flow ${m.flowUid}\nbase ${fmtFlow(base)}\nhealed ${fmtFlow(g.flow)}`;
          const arms = checkFlowInvariants(healed, { mode: 'strict', routed: new Set([m.flowUid]) })
            .filter((v) => v.uid === m.flowUid)
            .map((v) => v.arm)
            .sort();
          expect(`${context}\n${arms.join(' ')}`).toBe(`${context}\n${[...want.remaining].sort().join(' ')}`);
          expect(`${context}\nidentity ${g.flow === base && g.clouds.length === 0}`).toBe(
            `${context}\nidentity ${want.identity}`,
          );
          const again = heal(g.flow, flowTerminals(g.flow, byUidOf(healed)), { stocks: stocksOf(healed) });
          expect(`${context}\n${again.flow === g.flow} ${again.clouds.length}`).toBe(`${context}\ntrue 0`);
          assertShapeRepair(shape, base, g, view, context);
        }
      }
      expect(`${shape} applied ${seen > 0}`).toBe(`${shape} applied true`);
    });
  }

  it('is the identity on every flow of strict scenes', () => {
    for (let seed = 1; seed <= 60; seed++) {
      const { view } = genScene(new Rng(seed));
      for (const el of view.elements) {
        if (el.type !== 'flow') continue;
        const g = heal(el, flowTerminals(el, byUidOf(view)), { stocks: stocksOf(view) });
        expect(`seed ${seed} flow ${el.uid}: ${g.flow === el} ${g.clouds.length}`).toBe(
          `seed ${seed} flow ${el.uid}: true 0`,
        );
      }
    }
  });
});

function assertShapeRepair(
  shape: ImportShape,
  base: FlowViewElement,
  g: { readonly flow: FlowViewElement; readonly clouds: readonly { uid: number; x: number; y: number }[] },
  view: StockFlowView,
  context: string,
): void {
  const healed = g.flow;
  const n = base.points.length;
  const byUid = byUidOf(view);
  const stockEnd = [0, n - 1].find((i) => byUid.get(base.points[i].attachedToUid ?? -1)?.type === 'stock');
  switch (shape) {
    case 'offFaceAxis': {
      // The endpoint moved back along its stub onto the face: the along-face coordinate is kept.
      const i = [0, n - 1].find((index) => {
        const el = byUid.get(base.points[index].attachedToUid ?? -1);
        return el?.type === 'stock' && faceOfEndpoint(el, base.points[index]) === undefined;
      })!;
      const moved = healed.points[i === 0 ? 0 : healed.points.length - 1];
      const before = base.points[i];
      expect(`${context}\n${Math.abs(moved.x - before.x) <= 1e-9 || Math.abs(moved.y - before.y) <= 1e-9}`).toBe(
        `${context}\ntrue`,
      );
      break;
    }
    case 'cornerEndpoint': {
      // The endpoint slides CORNER_CLEARANCE along the face its stub leaves
      // perpendicular to; the straight pipe and its cloud follow, so it stays straight.
      const i = stockEnd!;
      const stock = byUid.get(base.points[i].attachedToUid!)!;
      const adjacent = base.points[i === 0 ? 1 : n - 2];
      const face = faceOfEndpoint(stock, base.points[i], adjacent);
      const moved = healed.points[i === 0 ? 0 : healed.points.length - 1];
      const shift = Math.hypot(moved.x - base.points[i].x, moved.y - base.points[i].y);
      expect(
        `${context}\n${healed.points.length} ${shift.toFixed(9)} ${faceOfEndpoint(stock, moved, healed.points[i === 0 ? 1 : healed.points.length - 2])}`,
      ).toBe(`${context}\n2 ${CORNER_CLEARANCE.toFixed(9)} ${face}`);
      expect(g.clouds.length).toBe(1);
      break;
    }
    case 'cloudOffset': {
      // The pipe keeps its geometry; the cloud comes to the endpoint.
      expect(`${context}\n${JSON.stringify(healed.points)}`).toBe(`${context}\n${JSON.stringify(base.points)}`);
      expect(g.clouds.length).toBe(1);
      const cloud = g.clouds[0];
      const end = healed.points.find((p) => p.attachedToUid === cloud.uid)!;
      expect([cloud.x, cloud.y]).toEqual([end.x, end.y]);
      break;
    }
    case 'valveOffPath':
      expect(`${context}\n${JSON.stringify(healed.points)}`).toBe(`${context}\n${JSON.stringify(base.points)}`);
      break;
    case 'slightlyDiagonal':
      // The source endpoint is kept; the sink (a cloud) snaps onto the source's axis.
      expect([healed.points[0].x, healed.points[0].y]).toEqual([base.points[0].x, base.points[0].y]);
      expect(healed.points.length).toBe(2);
      break;
    case 'unattachedFlow':
    case 'listedInTwoStocks':
      break;
  }
}

describe('heal: a stock endpoint on a corner', () => {
  // Stock S at the origin: corners (+-22.5, +-17.5), valid range x in [-19.5, 19.5] and y in [-14.5, 14.5].
  const ROWS: ReadonlyArray<{
    readonly name: string;
    readonly endpoint: [number, number];
    readonly cloud: [number, number];
    readonly want: number[][];
  }> = [
    {
      name: 'bottom-right corner, stub down: along the bottom face',
      endpoint: [22.5, 17.5],
      cloud: [22.5, 80],
      want: [
        [19.5, 17.5],
        [19.5, 80],
      ],
    },
    {
      name: 'bottom-right corner, stub right: along the right face',
      endpoint: [22.5, 17.5],
      cloud: [100, 17.5],
      want: [
        [22.5, 14.5],
        [100, 14.5],
      ],
    },
    {
      name: 'top-left corner, stub up: along the top face',
      endpoint: [-22.5, -17.5],
      cloud: [-22.5, -80],
      want: [
        [-19.5, -17.5],
        [-19.5, -80],
      ],
    },
    {
      name: 'top-left corner, stub left: along the left face',
      endpoint: [-22.5, -17.5],
      cloud: [-100, -17.5],
      want: [
        [-22.5, -14.5],
        [-100, -14.5],
      ],
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const view = loadView([
        stockJson(1, 0, 0),
        cloudJson(3, 10, row.cloud[0], row.cloud[1]),
        flowJson(
          10,
          { x: (row.endpoint[0] + row.cloud[0]) / 2, y: (row.endpoint[1] + row.cloud[1]) / 2 },
          [
            { x: row.endpoint[0], y: row.endpoint[1] },
            { x: row.cloud[0], y: row.cloud[1] },
          ],
          { source: 1, sink: 3 },
        ),
      ]);
      const f = flowOf(view, 10);
      const g = heal(f, flowTerminals(f, byUidOf(view)));
      expect(g.flow.points.map((p) => [p.x, p.y])).toEqual(row.want);
      expect(g.clouds.map((c) => [c.x, c.y])).toEqual([row.want[1]]);
      expect(strictReport(applyGeometry(view, g), [10])).toBe('');
    });
  }
});

describe('heal: the stocks context (G6`s cloud clause)', () => {
  // A valid stock -> cloud flow whose cloud sits inside a second, unrelated stock.
  const view = loadView([
    stockJson(1, 0, 0),
    stockJson(2, 310, 0),
    cloudJson(3, 10, 300, 0),
    flowJson(
      10,
      { x: 160, y: 0 },
      [
        { x: 22.5, y: 0 },
        { x: 300, y: 0 },
      ],
      { source: 1, sink: 3 },
    ),
  ]);
  const f = flowOf(view, 10);
  const t = flowTerminals(f, byUidOf(view));

  it('without stocks the flow is healthy and left alone', () => {
    const g = heal(f, t);
    expect([g.flow === f, g.clouds.length]).toEqual([true, 0]);
  });

  it('with stocks the cloud moves along its segment to the nearer edge of the stock it sits in', () => {
    const stocks = [stockOf(view, 1), stockOf(view, 2)];
    expect(flowFault(f, t, stocks)).toBe('crossing');
    const g = heal(f, t, { stocks });
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [22.5, 0],
      [287.5, 0],
    ]);
    expect(g.clouds).toEqual([{ ...cloudOf(view, 3), x: 287.5, y: 0 }]);
    const healed = applyGeometry(view, g);
    expect(flowFault(g.flow, flowTerminals(g.flow, byUidOf(healed)), stocks)).toBe('none');
    expect(strictReport(healed, [10])).toBe('');
  });

  it('a source cloud inside a stock moves along its segment too', () => {
    // The mirror image: cloud -> stock, the cloud inside a stock left of the flow.
    const mirror = loadView([
      stockJson(1, 0, 0),
      stockJson(2, -310, 0),
      cloudJson(3, 10, -300, 0),
      flowJson(
        10,
        { x: -160, y: 0 },
        [
          { x: -300, y: 0 },
          { x: -22.5, y: 0 },
        ],
        { source: 3, sink: 1 },
      ),
    ]);
    const mf = flowOf(mirror, 10);
    const mt = flowTerminals(mf, byUidOf(mirror));
    const stocks = [stockOf(mirror, 1), stockOf(mirror, 2)];
    expect(flowFault(mf, mt, stocks)).toBe('crossing');
    const g = heal(mf, mt, { stocks });
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [-287.5, 0],
      [-22.5, 0],
    ]);
    expect(g.clouds).toEqual([{ ...cloudOf(mirror, 3), x: -287.5, y: 0 }]);
    const healed = applyGeometry(mirror, g);
    expect(flowFault(g.flow, flowTerminals(g.flow, byUidOf(healed)), stocks)).toBe('none');
    expect(strictReport(healed, [10])).toBe('');
  });

  it('over seeded junk flows, heal with stocks leaves every flow flowFault(stocks) accepts, and is idempotent', () => {
    const rng = new Rng(20260911);
    let checked = 0;
    for (let i = 0; i < 400; i++) {
      const sx = rng.float(-40, 40);
      const other = { x: rng.float(-250, 250), y: rng.float(-250, 250) };
      const count = rng.int(2, 5);
      const pts: Array<{ x: number; y: number }> = [{ x: 22.5, y: rng.float(-25, 25) }];
      for (let k = 1; k < count; k++) pts.push({ x: rng.float(-200, 200), y: rng.float(-200, 200) });
      const cloud = pts[count - 1];
      const junk = loadView([
        stockJson(1, sx, 0),
        stockJson(2, other.x, other.y),
        cloudJson(3, 10, cloud.x + rng.float(-5, 5), cloud.y + rng.float(-5, 5)),
        flowJson(10, { x: rng.float(-100, 100), y: rng.float(-100, 100) }, pts, { source: 1, sink: 3 }),
      ]);
      const jf = flowOf(junk, 10);
      const stocks = stocksOf(junk);
      const g = heal(jf, flowTerminals(jf, byUidOf(junk)), { stocks });
      const healed = applyGeometry(junk, g);
      const ht = flowTerminals(g.flow, byUidOf(healed));
      const again = heal(g.flow, ht, { stocks });
      expect(`junk ${i} ${fmtFlow(g.flow)}: ${again.flow === g.flow} ${again.clouds.length}`).toBe(
        `junk ${i} ${fmtFlow(g.flow)}: true 0`,
      );
      if (flowFault(g.flow, ht, stocks) === 'none') checked++;
    }
    // Most junk heals to a valid flow; the rest are overlapping-body scenes where G6 is best effort.
    expect(checked).toBeGreaterThan(300);
  });
});

describe('heal on degenerate input', () => {
  const view = loadView([
    stockJson(1, 100, 100),
    cloudJson(3, 10, 300, 100),
    flowJson(
      10,
      { x: 200, y: 100 },
      [
        { x: 122.5, y: 100 },
        { x: 300, y: 100 },
      ],
      { source: 1, sink: 3 },
    ),
  ]);
  const good = flowOf(view, 10);
  const t = flowTerminals(good, byUidOf(view));

  // #818: a non-finite coordinate only exists in memory (the loader repairs
  // NaN), so these rows mutate a loaded flow.
  const DEGENERATE: ReadonlyArray<{ readonly name: string; readonly flow: FlowViewElement }> = [
    { name: 'NaN valve (#818)', flow: { ...good, x: NaN } },
    { name: 'NaN interior point (#818)', flow: { ...good, points: [good.points[0], P(NaN, 5), good.points[1]] } },
    { name: 'Infinity endpoint (#818)', flow: { ...good, points: [P(Infinity, 100, 1), good.points[1]] } },
    { name: 'a single point', flow: { ...good, points: [good.points[0]] } },
    { name: 'no points', flow: { ...good, points: [] } },
  ];
  for (const row of DEGENERATE) {
    it(`${row.name}: routes afresh to finite, valid geometry`, () => {
      const g = heal(row.flow, t);
      const all = [g.flow.x, g.flow.y, ...g.flow.points.flatMap((p) => [p.x, p.y])];
      expect(all.every(Number.isFinite)).toBe(true);
      expect(strictReport(applyGeometry(view, g), [10])).toBe('');
    });
  }

  it('a non-finite terminal (a NaN pointer) returns the flow unchanged from every operation', () => {
    const nan = freeTerminal({ x: NaN, y: 0 }, cloudOf(view, 3));
    const moved = { ...stockOf(view, 1), x: Infinity };
    for (const g of [
      heal(good, { source: t.source, sink: nan }),
      routeEnd(good, 'sink', nan, { fixed: t.source }),
      routeEnd(good, 'source', stockTerminal(moved, good.points[0], good.points[1], stockOf(view, 1)), {
        fixed: t.sink,
      }),
      route(t.source, nan, { flow: good }),
      offsetSegment(good, 0, 120, { source: t.source, sink: nan }),
      offsetSegment(good, 0, NaN, t),
    ]) {
      expect([g.flow === good, g.clouds.length]).toEqual([true, 0]);
    }
  });

  it('does not throw on a self-loop flow, and returns finite geometry (#720)', () => {
    const loop = loadView([
      stockJson(1, 100, 100),
      flowJson(
        10,
        { x: 150, y: 100 },
        [
          { x: 122.5, y: 100 },
          { x: 200, y: 100 },
          { x: 200, y: 82.5 },
          { x: 100, y: 82.5 },
        ],
        { source: 1, sink: 1 },
      ),
    ]);
    const f = flowOf(loop, 10);
    const lt = flowTerminals(f, byUidOf(loop));
    const results = [
      heal(f, lt).flow,
      routeEnd(f, 'sink', lt.sink, { fixed: lt.source }).flow,
      route(lt.source, lt.sink, { flow: f }).flow,
      offsetSegment(f, 1, 250, lt).flow,
      slideValve(f, { x: 30, y: 0 }),
    ];
    for (const r of results) {
      expect(r.points.length).toBeGreaterThanOrEqual(2);
      expect([r.x, r.y, ...r.points.flatMap((p) => [p.x, p.y])].every(Number.isFinite)).toBe(true);
      // Tolerant mode accepts the input and every result (#720 is strict-only).
      const v = { ...loop, elements: loop.elements.map((e) => (e.uid === 10 ? r : e)) };
      expect(checkFlowInvariants(v, { mode: 'tolerant' })).toEqual([]);
    }
  });
});
