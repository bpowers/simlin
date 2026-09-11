// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Seeded property tests of flow-geometry.ts over generated scenes.
//
// Each seed builds a strict scene (genScene) and drives every operation a
// planner will call through simulated drags: successive pointer positions
// from the press, each frame evaluated against the gesture's base view.
// Operations and their terminals are derived the way production supplies them
// (flowTerminals over the view; a moved stock keeps its base attachment through
// stockTerminal):
//   moveStock   routeEnd on every flow attached to a dragged stock
//   dragCloud   routeEnd with a cloud endpoint following the pointer
//   detach      routeEnd with a stock endpoint dragged into empty space
//   reattach    routeEnd onto every other stock in the view
//   create      route from a stock to the pointer
//   offset      offsetSegment on a random segment, swept perpendicular
//   slide       slideValve, swept by pointer delta
//   translate   both terminals moved together
// Imported scenes (genImportedScene) are healed first, then dragged.
//
// Asserted on every frame: the strict invariants on the routed flow (frames
// with the pointer inside a stock are the planner's target hover and are
// skipped); E4 locality (the result names only this flow and its own clouds,
// and changes no field but the geometry). Asserted across frames: E3
// continuity. slideValve and translate never jump. For the routing operations
// a jump (a frame-to-frame change more than three pointer steps plus 1px) must
// coincide with a change of route shape (segment directions or a stock face),
// which is where the plan's documented transitions happen, except for a small
// residue of same-shape switches where a preserved tail's interior corner moves
// to another candidate because the one it held became infeasible; that residue
// is bounded and reported, not zero.
//
// What this does not establish: behavior after an engine round trip (E4's
// epsilon arm), or anything about the planner's target hit testing.

import { describe, it, expect } from '@rstest/core';

import { performance } from 'node:perf_hooks';

import type {
  CloudViewElement,
  FlowViewElement,
  StockFlowView,
  StockViewElement,
  UID,
  ViewElement,
} from '@simlin/core/datamodel';

import { StockHeight, StockWidth } from '../drawing/default';
import {
  faceOfEndpoint,
  flowTerminals,
  freeTerminal,
  heal,
  offsetSegment,
  route,
  routeEnd,
  segmentHold,
  slideValve,
  stockTerminal,
  translate,
  type FlowGeometry,
  type XY,
} from '../flow-geometry';
import { checkFlowInvariants, formatFlowViolations } from './support/flow-invariants';
import { byUidOf, directions, hausdorff, patchView } from './support/flow-geometry-fixtures';
import { genImportedScene, genScene, Rng } from './support/scene-generator';

const SEEDS = 120;
const FRAMES = 24;

const OPS = [
  'moveStock',
  'dragCloud',
  'detach',
  'reattach',
  'create',
  'offset',
  'slide',
  'translate',
  'imported',
] as const;
type Op = (typeof OPS)[number];

interface Tally {
  frames: number;
  violations: string[];
  locality: string[];
  jumps: number;
  shapeJumps: number;
  sameShapeJumps: string[];
  maxJump: number;
}

function newTally(): Tally {
  return { frames: 0, violations: [], locality: [], jumps: 0, shapeJumps: 0, sameShapeJumps: [], maxJump: 0 };
}

function insideAnyStock(p: XY, view: StockFlowView): boolean {
  return view.elements.some(
    (e) => e.type === 'stock' && Math.abs(p.x - e.x) < StockWidth / 2 && Math.abs(p.y - e.y) < StockHeight / 2,
  );
}

/** The route's shape: segment directions and the faces its stock endpoints use. */
function shapeOf(f: FlowViewElement, view: StockFlowView): string {
  const byUid = byUidOf(view);
  const n = f.points.length;
  const face = (i: number, j: number): string => {
    const el = byUid.get(f.points[i].attachedToUid ?? -1);
    return el?.type === 'stock' ? (faceOfEndpoint(el, f.points[i], f.points[j]) ?? '?') : '-';
  };
  return `${face(0, 1)} ${directions(f.points)} ${face(n - 1, n - 2)}`;
}

function checkFrame(tally: Tally, view: StockFlowView, routed: readonly UID[], context: string): void {
  tally.frames++;
  const violations = checkFlowInvariants(view, { mode: 'strict', routed: new Set(routed) }).filter((v) =>
    routed.includes(v.uid),
  );
  if (violations.length > 0 && tally.violations.length < 5) {
    tally.violations.push(`${context}\n${formatFlowViolations(violations)}`);
  }
}

function checkLocality(
  tally: Tally,
  base: FlowViewElement,
  g: FlowGeometry,
  allowedClouds: readonly UID[],
  context: string,
): void {
  const problems: string[] = [];
  const identity = (f: FlowViewElement): string => JSON.stringify({ ...f, x: 0, y: 0, points: [] });
  if (identity(g.flow) !== identity(base)) problems.push('non-geometry flow fields changed');
  for (const c of g.clouds) {
    if (!allowedClouds.includes(c.uid) || c.flowUid !== base.uid)
      problems.push(`cloud ${c.uid} is not this flow's terminal`);
  }
  if (problems.length > 0 && tally.locality.length < 5) {
    tally.locality.push(`${context}: ${problems.join('; ')}`);
  }
}

function continuity(
  tally: Tally,
  prev: { flow: FlowViewElement; shape: string } | undefined,
  cur: FlowViewElement,
  shape: string,
  step: number,
  context: string,
): void {
  if (prev === undefined) return;
  const d = Math.max(hausdorff(prev.flow.points, cur.points, 2), Math.hypot(prev.flow.x - cur.x, prev.flow.y - cur.y));
  if (d <= 3 * step + 1) return;
  tally.jumps++;
  tally.maxJump = Math.max(tally.maxJump, d);
  if (prev.shape !== shape) {
    tally.shapeJumps++;
  } else if (tally.sameShapeJumps.length < 1000) {
    tally.sameShapeJumps.push(`${context} d=${d.toFixed(2)} ${shape}`);
  }
}

function attachedFlows(view: StockFlowView, stockUid: UID): FlowViewElement[] {
  return view.elements.filter(
    (e): e is FlowViewElement =>
      e.type === 'flow' &&
      e.points.length >= 2 &&
      (e.points[0].attachedToUid === stockUid || e.points[e.points.length - 1].attachedToUid === stockUid),
  );
}

function randomDelta(rng: Rng): XY {
  const angle = rng.float(0, 2 * Math.PI);
  const magnitude = rng.float(5, 250);
  return { x: magnitude * Math.cos(angle), y: magnitude * Math.sin(angle) };
}

describe('flow-geometry over generated scenes', () => {
  const tallies = new Map<Op, Tally>(OPS.map((op) => [op, newTally()]));
  let routeEndCalls = 0;
  let routeEndMs = 0;
  let routeCalls = 0;
  let routeMs = 0;

  const moveStock = (view: StockFlowView, stock: StockViewElement, delta: XY, tally: Tally, context: string): void => {
    const byUid = byUidOf(view);
    const flows = attachedFlows(view, stock.uid);
    const prev = new Map<UID, { flow: FlowViewElement; shape: string }>();
    const step = Math.hypot(delta.x, delta.y) / FRAMES;
    for (let k = 0; k <= FRAMES; k++) {
      const moved = { ...stock, x: stock.x + (delta.x * k) / FRAMES, y: stock.y + (delta.y * k) / FRAMES };
      const changed: ViewElement[] = [moved];
      const routed: UID[] = [];
      for (const f of flows) {
        const n = f.points.length;
        const t = flowTerminals(f, byUid);
        const sourceOn = f.points[0].attachedToUid === stock.uid;
        const sinkOn = f.points[n - 1].attachedToUid === stock.uid;
        let g: FlowGeometry;
        if (sourceOn && sinkOn) {
          g = { flow: translate(f, { x: moved.x - stock.x, y: moved.y - stock.y }), clouds: [] };
        } else {
          const end = sourceOn ? 'source' : 'sink';
          const endpoint = sourceOn ? f.points[0] : f.points[n - 1];
          const adjacent = sourceOn ? f.points[1] : f.points[n - 2];
          const started = performance.now();
          g = routeEnd(f, end, stockTerminal(moved, endpoint, adjacent, stock), {
            fixed: sourceOn ? t.sink : t.source,
          });
          routeEndMs += performance.now() - started;
          routeEndCalls++;
        }
        checkLocality(tally, f, g, [], `${context} k ${k} flow ${f.uid}`);
        changed.push(g.flow, ...g.clouds);
        routed.push(f.uid);
      }
      const view2 = patchView(view, changed);
      checkFrame(tally, view2, routed, `${context} k ${k}`);
      for (const uid of routed) {
        const f = view2.elements.find((e) => e.uid === uid) as FlowViewElement;
        const shape = shapeOf(f, view2);
        continuity(tally, prev.get(uid), f, shape, step, `${context} flow ${uid} k ${k}`);
        prev.set(uid, { flow: f, shape });
      }
    }
  };

  for (let seed = 1; seed <= SEEDS; seed++) {
    const scene = genScene(new Rng(seed));
    const rng = new Rng(seed * 7919 + 1);
    const view = scene.view;
    const byUid = byUidOf(view);
    const stocks = view.elements.filter((e): e is StockViewElement => e.type === 'stock');
    const flows = view.elements.filter((e): e is FlowViewElement => e.type === 'flow');

    // moveStock
    moveStock(view, rng.pick(stocks), randomDelta(rng), tallies.get('moveStock')!, `seed ${seed}`);

    // dragCloud / detach: an endpoint follows the pointer
    for (const op of ['dragCloud', 'detach'] as const) {
      const tally = tallies.get(op)!;
      const candidates: Array<{ f: FlowViewElement; end: 'source' | 'sink' }> = [];
      for (const f of flows) {
        const t = flowTerminals(f, byUid);
        const want = op === 'dragCloud' ? 'free' : 'stock';
        if (t.source.kind === want) candidates.push({ f, end: 'source' });
        if (t.sink.kind === want) candidates.push({ f, end: 'sink' });
      }
      if (candidates.length === 0) continue;
      const { f, end } = rng.pick(candidates);
      const t = flowTerminals(f, byUid);
      const endpoint = end === 'source' ? f.points[0] : f.points[f.points.length - 1];
      const own = end === 'source' ? t.source : t.sink;
      const cloud: CloudViewElement =
        own.kind === 'free' && own.cloud !== undefined
          ? own.cloud
          : {
              type: 'cloud',
              uid: view.nextUid,
              flowUid: f.uid,
              x: endpoint.x,
              y: endpoint.y,
              isZeroRadius: false,
              ident: undefined,
            };
      const delta = randomDelta(rng);
      const step = Math.hypot(delta.x, delta.y) / FRAMES;
      let prev: { flow: FlowViewElement; shape: string } | undefined;
      for (let k = 1; k <= FRAMES; k++) {
        const p = { x: endpoint.x + (delta.x * k) / FRAMES, y: endpoint.y + (delta.y * k) / FRAMES };
        if (insideAnyStock(p, view)) {
          prev = undefined;
          continue;
        }
        const g = routeEnd(f, end, freeTerminal(p, cloud), { fixed: end === 'source' ? t.sink : t.source });
        checkLocality(tally, f, g, [cloud.uid], `seed ${seed} k ${k}`);
        // A detached end's new cloud is added by the planner; the core reports it moved onto the endpoint.
        const withCloud = patchView(view, [{ ...cloud, x: p.x, y: p.y }, g.flow, ...g.clouds]);
        checkFrame(tally, withCloud, [f.uid], `seed ${seed} ${op} flow ${f.uid} k ${k}`);
        const shape = shapeOf(g.flow, withCloud);
        continuity(tally, prev, g.flow, shape, step, `seed ${seed} flow ${f.uid} k ${k}`);
        prev = { flow: g.flow, shape };
      }
    }

    // reattach: a cloud end onto every other stock
    {
      const tally = tallies.get('reattach')!;
      for (const f of flows) {
        const t = flowTerminals(f, byUid);
        for (const end of ['source', 'sink'] as const) {
          const own = end === 'source' ? t.source : t.sink;
          const fixed = end === 'source' ? t.sink : t.source;
          if (own.kind !== 'free' || own.cloud === undefined) continue;
          for (const s of stocks) {
            if (fixed.kind === 'stock' && fixed.stock.uid === s.uid) continue;
            const g = routeEnd(f, end, stockTerminal(s), { fixed });
            checkLocality(tally, f, g, [], `seed ${seed} flow ${f.uid} ${end} -> S${s.uid}`);
            checkFrame(
              tally,
              patchView(view, [g.flow], [own.cloud.uid]),
              [f.uid],
              `seed ${seed} flow ${f.uid} ${end} -> S${s.uid}`,
            );
          }
        }
      }
    }

    // create: from a stock to the pointer
    {
      const tally = tallies.get('create')!;
      const s = rng.pick(stocks);
      const delta = randomDelta(rng);
      const draft: FlowViewElement = { ...flows[0], uid: view.nextUid + 1, points: [], x: s.x, y: s.y };
      const sinkCloud: CloudViewElement = {
        type: 'cloud',
        uid: view.nextUid + 2,
        flowUid: draft.uid,
        x: s.x,
        y: s.y,
        isZeroRadius: false,
        ident: undefined,
      };
      const step = Math.hypot(delta.x, delta.y) / FRAMES;
      let prev: { flow: FlowViewElement; shape: string } | undefined;
      for (let k = 1; k <= FRAMES; k++) {
        const p = { x: s.x + (delta.x * k) / FRAMES, y: s.y + (delta.y * k) / FRAMES };
        if (insideAnyStock(p, view)) {
          prev = undefined;
          continue;
        }
        const started = performance.now();
        const g = route(stockTerminal(s), freeTerminal(p, sinkCloud), { flow: draft });
        routeMs += performance.now() - started;
        routeCalls++;
        const view2 = patchView(view, [{ ...sinkCloud, x: p.x, y: p.y }, g.flow, ...g.clouds]);
        checkFrame(tally, view2, [draft.uid], `seed ${seed} create k ${k}`);
        const shape = shapeOf(g.flow, view2);
        continuity(tally, prev, g.flow, shape, step, `seed ${seed} create k ${k}`);
        prev = { flow: g.flow, shape };
      }
    }

    // offset
    {
      const tally = tallies.get('offset')!;
      const f = rng.pick(flows);
      const i = rng.int(0, f.points.length - 2);
      const { hold } = segmentHold(f.points, i);
      const amount = rng.float(-150, 150);
      const t = flowTerminals(f, byUid);
      const allowed = [t.source, t.sink].flatMap((term) =>
        term.kind === 'free' && term.cloud !== undefined ? [term.cloud.uid] : [],
      );
      const step = Math.abs(amount) / FRAMES;
      let prev: { flow: FlowViewElement; shape: string } | undefined;
      for (let k = 0; k <= FRAMES; k++) {
        const g = offsetSegment(f, i, hold + (amount * k) / FRAMES, t, { stocks });
        checkLocality(tally, f, g, allowed, `seed ${seed} seg ${i} k ${k}`);
        const view2 = patchView(view, [g.flow, ...g.clouds]);
        checkFrame(tally, view2, [f.uid], `seed ${seed} offset flow ${f.uid} seg ${i} k ${k}`);
        const shape = shapeOf(g.flow, view2);
        continuity(tally, prev, g.flow, shape, step, `seed ${seed} flow ${f.uid} seg ${i} k ${k}`);
        prev = { flow: g.flow, shape };
      }
    }

    // slide
    {
      const tally = tallies.get('slide')!;
      const f = rng.pick(flows);
      const delta = randomDelta(rng);
      const step = Math.hypot(delta.x, delta.y) / FRAMES;
      let prev: { flow: FlowViewElement; shape: string } | undefined;
      for (let k = 0; k <= FRAMES; k++) {
        const nf = slideValve(f, { x: (delta.x * k) / FRAMES, y: (delta.y * k) / FRAMES });
        const view2 = patchView(view, [nf]);
        checkFrame(tally, view2, [f.uid], `seed ${seed} slide flow ${f.uid} k ${k}`);
        continuity(tally, prev, nf, 'same', step, `seed ${seed} flow ${f.uid} k ${k}`);
        prev = { flow: nf, shape: 'same' };
      }
    }

    // translate: a flow with both terminals selected (stocks and clouds moved together)
    {
      const tally = tallies.get('translate')!;
      const f = rng.pick(flows);
      const delta = randomDelta(rng);
      const t = flowTerminals(f, byUid);
      const step = Math.hypot(delta.x, delta.y) / FRAMES;
      let prev: { flow: FlowViewElement; shape: string } | undefined;
      for (let k = 0; k <= FRAMES; k++) {
        const d = { x: (delta.x * k) / FRAMES, y: (delta.y * k) / FRAMES };
        const moved: ViewElement[] = [];
        let cloudOnStock = false;
        for (const term of [t.source, t.sink]) {
          if (term.kind === 'stock') {
            moved.push({ ...term.stock, x: term.stock.x + d.x, y: term.stock.y + d.y });
          } else if (term.cloud !== undefined) {
            const c = { ...term.cloud, x: term.cloud.x + d.x, y: term.cloud.y + d.y };
            cloudOnStock ||= insideAnyStock(c, view);
            moved.push(c);
          }
        }
        const nf = translate(f, d);
        // Dropping a selection's cloud onto another stock is the user's move (the
        // planner hit-tests it); translate itself changes no shape, so those frames
        // are continuity frames only.
        if (!cloudOnStock) {
          checkFrame(tally, patchView(view, [...moved, nf]), [f.uid], `seed ${seed} translate flow ${f.uid} k ${k}`);
        }
        continuity(tally, prev, nf, 'same', step, `seed ${seed} flow ${f.uid} k ${k}`);
        prev = { flow: nf, shape: 'same' };
      }
    }

    // imported: heal every flow, then drag a stock on the healed view
    {
      const tally = tallies.get('imported')!;
      const imported = genImportedScene(new Rng(seed));
      let healedView = imported.view;
      const changed: ViewElement[] = [];
      const routed: UID[] = [];
      for (const el of imported.view.elements) {
        if (el.type !== 'flow') continue;
        const g = heal(el, flowTerminals(el, byUidOf(imported.view)));
        changed.push(g.flow, ...g.clouds);
        if (!imported.mutations.some((m) => m.flowUid === el.uid && m.shape === 'unattachedFlow')) {
          routed.push(el.uid);
        }
      }
      healedView = patchView(imported.view, changed);
      checkFrame(tally, healedView, routed, `seed ${seed} healed`);
      const istocks = healedView.elements.filter((e): e is StockViewElement => e.type === 'stock');
      moveStock(healedView, rng.pick(istocks), randomDelta(rng), tally, `seed ${seed} imported`);
    }
  }

  for (const op of OPS) {
    it(`${op}: strict invariants on every routed frame, and E4 locality`, () => {
      const tally = tallies.get(op)!;
      expect(tally.frames).toBeGreaterThan(SEEDS);
      expect(tally.violations.join('\n\n')).toBe('');
      expect(tally.locality.join('\n')).toBe('');
    });
  }

  for (const op of ['slide', 'translate'] as const) {
    it(`${op}: E3 continuity with no discrete transitions at all`, () => {
      const tally = tallies.get(op)!;
      expect(`${tally.jumps} jumps; ${tally.sameShapeJumps.slice(0, 5).join('\n')}`).toBe(`0 jumps; `);
    });
  }

  // The same-shape residue, pinned per op at the measured count. It is where a
  // feasibility change keeps the route's shape but moves a hold: an offset riser
  // pushed out at MIN_SEGMENT, a tail whose held corner stopped being feasible.
  // The continuity constraints with real budgets are the 1px sweeps
  // (flow-geometry-sweeps.test.ts); this guards the seeded scenes from new classes.
  const SAME_SHAPE_BUDGET = { moveStock: 0, dragCloud: 0, detach: 1, create: 0, offset: 2, imported: 2 } as const;
  for (const op of ['moveStock', 'dragCloud', 'detach', 'create', 'offset', 'imported'] as const) {
    it(`${op}: E3 continuity: jumps are shape transitions, with at most ${SAME_SHAPE_BUDGET[op]} same-shape jumps`, () => {
      const tally = tallies.get(op)!;
      const report = `${op}: ${tally.sameShapeJumps.length} same-shape jumps over ${tally.frames} frames\n${tally.sameShapeJumps.slice(0, 5).join('\n')}`;
      expect(tally.sameShapeJumps.length <= SAME_SHAPE_BUDGET[op] ? 'within budget' : report).toBe('within budget');
    });
  }

  it('route and routeEnd stay well under a millisecond per call (measured over every frame above)', () => {
    // A generous bound on the mean so a loaded machine does not flake; typical
    // calls take tens of microseconds.
    expect(routeEndCalls).toBeGreaterThan(1000);
    expect(routeCalls).toBeGreaterThan(1000);
    expect(`routeEnd ${(routeEndMs / routeEndCalls).toFixed(4)}ms`).toMatch(/^routeEnd 0\./);
    expect(`route ${(routeMs / routeCalls).toFixed(4)}ms`).toMatch(/^route 0\./);
  });
});
