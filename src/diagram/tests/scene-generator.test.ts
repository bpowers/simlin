// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of the seeded scene generator (tests/support/scene-generator.ts).
//
// What these establish: generated strict scenes hold every strict flow arm, M1,
// M3, duplicate-free stock lists and list agreement over many seeds; the view
// realizes every route the generator records (the route's shape classified from
// the points' directions, its hub end, and the face its far stock is entered
// through), and the recorded routes cover every shape x start x end combination,
// so "passes over many seeds" is not vacuous; imported scenes pass tolerant mode
// while strict mode reports exactly the arms each applied corpus shape records;
// a seed determines its scene; and a sample of both kinds of scene keeps its
// strict arms through an engine round trip.
//
// What they do not establish: that the generated distribution resembles real
// models beyond the enumerated shapes, or anything about gestures (the gesture
// generator arrives with the planner API).

import { describe, it, expect, beforeAll } from '@rstest/core';

import type { JsonProject } from '@simlin/engine';
import {
  modelToJson,
  type FlowViewElement,
  type Point,
  type StockFlowView,
  type StockViewElement,
  type UID,
  type ViewElement,
} from '@simlin/core/datamodel';

import { StockHeight, StockWidth } from '../drawing/default';
import { describeWithEngine, editorModel, loadEngine, type EngineModule } from './support/engine';
import { checkFlowInvariants, formatFlowViolations, PIPE_SPACING } from './support/flow-invariants';
import {
  FACES,
  FLOW_SHAPES,
  genImportedScene,
  genScene,
  IMPORT_SHAPES,
  Rng,
  ROUTE_STARTS,
  type Face,
  type RouteRecord,
  type Scene,
} from './support/scene-generator';
import {
  checkKindAgreement,
  checkReferentialIntegrity,
  checkStockFlowAgreement,
  checkStockListDuplicates,
  formatViewViolations,
} from './support/view-invariants';

const SEEDS = 300;

function sceneFingerprint(scene: Scene): string {
  return JSON.stringify({ view: scene.view, variables: [...scene.model.variables.entries()] });
}

/**
 * The route shape as the points' segment directions show it: straight is one
 * segment; an L turns once; a Z turns and turns back to its first direction; a
 * bracket is stub, riser, run in the first direction, the riser reversed, and a
 * stub in the first direction. Reversing a route preserves its class.
 */
function classifyRoute(points: readonly Point[]): string {
  const dirs = points.slice(1).map((p, i) => ({ x: Math.sign(p.x - points[i].x), y: Math.sign(p.y - points[i].y) }));
  const same = (a: Point, b: Point): boolean => a.x === b.x && a.y === b.y;
  const reversed = (a: Point, b: Point): boolean => a.x === -b.x && a.y === -b.y;
  const perpendicular = (a: Point, b: Point): boolean => a.x * b.x + a.y * b.y === 0;
  if (dirs.length === 1) return 'straight';
  if (dirs.length === 2 && perpendicular(dirs[0], dirs[1])) return 'L';
  if (dirs.length === 3 && perpendicular(dirs[0], dirs[1]) && same(dirs[0], dirs[2])) return 'Z';
  if (
    dirs.length === 5 &&
    perpendicular(dirs[0], dirs[1]) &&
    same(dirs[0], dirs[2]) &&
    reversed(dirs[1], dirs[3]) &&
    same(dirs[0], dirs[4])
  ) {
    return 'bracket';
  }
  return `unclassified(${dirs.map((d) => `${d.x},${d.y}`).join(' ')})`;
}

function faceOf(p: Point, stock: StockViewElement): Face | 'none' {
  const eq = (a: number, b: number): boolean => Math.abs(a - b) < 1e-9;
  if (eq(p.x, stock.x - StockWidth / 2)) return 'left';
  if (eq(p.x, stock.x + StockWidth / 2)) return 'right';
  if (eq(p.y, stock.y - StockHeight / 2)) return 'top';
  if (eq(p.y, stock.y + StockHeight / 2)) return 'bottom';
  return 'none';
}

function recordedRoute(route: RouteRecord): string {
  return `${route.shape} ${route.start} ${route.far === undefined ? 'cloud' : `stock ${route.far.stockUid}@${route.far.face}`}`;
}

/** The same description, read from the view alone. */
function observedRoute(view: StockFlowView, hubUid: UID, f: FlowViewElement): string {
  const byUid = new Map(view.elements.map((e) => [e.uid, e]));
  const first = f.points[0];
  const last = f.points[f.points.length - 1];
  const kindAt = (p: Point): string => byUid.get(p.attachedToUid ?? -1)?.type ?? 'none';
  const start =
    first.attachedToUid === hubUid
      ? 'hubSource'
      : last.attachedToUid === hubUid
        ? 'hubSink'
        : kindAt(first) === 'cloud'
          ? 'freeCloud'
          : 'unknown';
  const farPoint = start === 'hubSink' ? first : last;
  const far = byUid.get(farPoint.attachedToUid ?? -1);
  const end =
    far?.type === 'stock' ? `stock ${far.uid}@${faceOf(farPoint, far)}` : far?.type === 'cloud' ? 'cloud' : 'none';
  return `${classifyRoute(f.points)} ${start} ${end}`;
}

describe('genScene', () => {
  it('is deterministic per seed and varies across seeds', () => {
    expect(sceneFingerprint(genScene(new Rng(42)))).toBe(sceneFingerprint(genScene(new Rng(42))));
    expect(sceneFingerprint(genScene(new Rng(42)))).not.toBe(sceneFingerprint(genScene(new Rng(43))));
  });

  it(`holds strict G1-G8, M1, M3, duplicate-free lists and list agreement over ${SEEDS} seeds`, () => {
    for (let seed = 1; seed <= SEEDS; seed++) {
      const { view, model } = genScene(new Rng(seed));
      const flowViolations = checkFlowInvariants(view, { mode: 'strict' });
      const viewViolations = [
        ...checkKindAgreement(view, model.variables),
        ...checkReferentialIntegrity(view),
        ...checkStockListDuplicates(model.variables),
        ...checkStockFlowAgreement(view, model.variables),
      ];
      expect(`seed ${seed}\n${formatFlowViolations(flowViolations)}${formatViewViolations(viewViolations)}`).toBe(
        `seed ${seed}\n`,
      );
    }
  });

  it('realizes every recorded route and covers every shape x start x end', () => {
    const combos = new Set<string>();
    for (let seed = 1; seed <= SEEDS; seed++) {
      const { view, hubUid, routes } = genScene(new Rng(seed));
      const flows = view.elements.filter((e): e is FlowViewElement => e.type === 'flow');
      expect(`seed ${seed}: ${routes.map((r) => r.flowUid).sort((a, b) => a - b)}`).toBe(
        `seed ${seed}: ${flows.map((f) => f.uid).sort((a, b) => a - b)}`,
      );
      for (const route of routes) {
        const f = flows.find((el) => el.uid === route.flowUid)!;
        expect(`seed ${seed} flow ${f.uid}: ${observedRoute(view, hubUid, f)}`).toBe(
          `seed ${seed} flow ${f.uid}: ${recordedRoute(route)}`,
        );
        combos.add(`${route.shape} ${route.start} ${route.far?.face ?? 'cloud'}`);
      }
    }
    const expected = FLOW_SHAPES.flatMap((shape) =>
      ROUTE_STARTS.flatMap((start) => [...FACES, 'cloud'].map((end) => `${shape} ${start} ${end}`)),
    );
    expect([...combos].sort()).toEqual(expected.sort());
  });

  it('shares stock faces at PIPE_SPACING, and produces links, aliases, modules and auxes', () => {
    let sharedFaces = 0;
    let links = 0;
    let aliases = 0;
    let modules = 0;
    let auxes = 0;
    for (let seed = 1; seed <= SEEDS; seed++) {
      const { view } = genScene(new Rng(seed));
      const flows = view.elements.filter((e): e is FlowViewElement => e.type === 'flow');
      sharedFaces += countSharedFaces(view.elements, flows);
      const count = (type: ViewElement['type']): number => view.elements.filter((e) => e.type === type).length;
      links += count('link');
      aliases += count('alias');
      modules += count('module');
      auxes += count('aux');
    }
    expect(sharedFaces).toBeGreaterThan(20);
    expect(links).toBeGreaterThan(SEEDS);
    expect(aliases).toBeGreaterThan(SEEDS / 3);
    expect(modules).toBeGreaterThan(SEEDS / 10);
    expect(auxes).toBeGreaterThan(SEEDS);
  });
});

/**
 * Faces carrying two or more endpoints, with the nearest pair asserted at least
 * PIPE_SPACING apart (the generator's slot rule, which the routing preference
 * shares).
 */
function countSharedFaces(elements: readonly ViewElement[], flows: readonly FlowViewElement[]): number {
  let shared = 0;
  for (const s of elements) {
    if (s.type !== 'stock') continue;
    const byFace = new Map<string, number[]>();
    for (const f of flows) {
      for (const p of [f.points[0], f.points[f.points.length - 1]]) {
        if (p.attachedToUid !== s.uid) continue;
        const face = faceOf(p, s);
        const along = face === 'left' || face === 'right' ? p.y : p.x;
        byFace.set(face, [...(byFace.get(face) ?? []), along]);
      }
    }
    for (const positions of byFace.values()) {
      if (positions.length < 2) continue;
      shared++;
      positions.sort((a, b) => a - b);
      for (let i = 1; i < positions.length; i++) {
        expect(positions[i] - positions[i - 1]).toBeGreaterThanOrEqual(PIPE_SPACING);
      }
    }
  }
  return shared;
}

function strictArms(view: StockFlowView): string[] {
  return checkFlowInvariants(view, { mode: 'strict' })
    .map((v) => `${v.uid}:${v.arm}`)
    .sort();
}

describe('genImportedScene', () => {
  it('is deterministic per seed', () => {
    const a = genImportedScene(new Rng(7));
    const b = genImportedScene(new Rng(7));
    expect(sceneFingerprint(a)).toBe(sceneFingerprint(b));
    expect(a.mutations).toEqual(b.mutations);
  });

  it(`passes tolerant mode, M1 and M3, while strict mode reports exactly the recorded arms, over ${SEEDS} seeds`, () => {
    for (let seed = 1; seed <= SEEDS; seed++) {
      const { view, model, mutations } = genImportedScene(new Rng(seed));
      const tolerant = checkFlowInvariants(view, { mode: 'tolerant' });
      const structural = [
        ...checkKindAgreement(view, model.variables),
        ...checkReferentialIntegrity(view),
        ...checkStockListDuplicates(model.variables),
      ];
      expect(`seed ${seed}\n${formatFlowViolations(tolerant)}${formatViewViolations(structural)}`).toBe(
        `seed ${seed}\n`,
      );

      const expectedStrict = mutations.flatMap((m) => m.flowArms.map((arm) => `${m.flowUid}:${arm}`)).sort();
      expect(`seed ${seed} ${strictArms(view).join(' ')}`).toBe(`seed ${seed} ${expectedStrict.join(' ')}`);

      const agreement = checkStockFlowAgreement(view, model.variables)
        .map((v) => v.arm)
        .sort();
      const expectedAgreement = mutations.flatMap((m) => m.viewArms).sort();
      expect(`seed ${seed} ${agreement.join(' ')}`).toBe(`seed ${seed} ${expectedAgreement.join(' ')}`);
    }
  });

  it('applies every corpus shape across seeds', () => {
    const seen = new Map<string, number>();
    for (let seed = 1; seed <= SEEDS; seed++) {
      for (const m of genImportedScene(new Rng(seed)).mutations) {
        seen.set(m.shape, (seen.get(m.shape) ?? 0) + 1);
      }
    }
    for (const shape of IMPORT_SHAPES) {
      expect(`${shape}: ${seen.get(shape) ?? 0}`).not.toBe(`${shape}: 0`);
    }
  });
});

// A committed edit reaches the editor through an engine round trip, which is not
// value-exact (one-ULP float drift, re-derived fields); these scenes must keep
// their strict arms through it or the checker would misjudge committed views.
const ROUND_TRIP_SEEDS = 30;

describeWithEngine('generated scenes through an engine round trip', () => {
  let engine: EngineModule;

  beforeAll(async () => {
    engine = await loadEngine();
  });

  async function roundTrip(scene: Scene): Promise<StockFlowView> {
    const projectJson: JsonProject = {
      name: 'scene',
      simSpecs: { startTime: 0, endTime: 1, dt: '1' },
      models: [modelToJson(scene.model)],
    };
    const project = await engine.Project.openJson(JSON.stringify(projectJson));
    try {
      return (await editorModel(project)).views[0];
    } finally {
      await project.dispose();
    }
  }

  it(`keeps the strict arm multiset of ${ROUND_TRIP_SEEDS} strict and ${ROUND_TRIP_SEEDS} imported scenes`, async () => {
    for (let seed = 1; seed <= ROUND_TRIP_SEEDS; seed++) {
      for (const [kind, scene] of [
        ['strict', genScene(new Rng(seed))],
        ['imported', genImportedScene(new Rng(seed))],
      ] as const) {
        const view = await roundTrip(scene);
        expect(`${kind} seed ${seed} ${strictArms(view).join(' ')}`).toBe(
          `${kind} seed ${seed} ${strictArms(scene.view).join(' ')}`,
        );
      }
    }
  });
});
