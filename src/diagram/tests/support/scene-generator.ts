// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Seeded scene generator for the diagram editing invariant tests.
 *
 * `genScene` produces views that hold the strict flow invariants (G1-G8), M1,
 * M3, and stock-list agreement. `genImportedScene` starts from such a scene and
 * applies the tolerant shapes the corpus measurement found in imported models,
 * recording for each the arms strict mode must report.
 *
 * Scenes are built as the engine's JSON model and deserialized with the
 * production `modelFromJson`, so element fields (idents, stock inflow/outflow
 * uids, labelSide defaults) are exactly what the editor sees on load.
 *
 * The generator's validity rules are its own geometry, never the checker's:
 * a test that generated with the checker and then checked would agree with
 * itself by construction.
 */

import type { JsonModel, JsonViewElement } from '@simlin/engine';
import { modelFromJson, type Model, type StockFlowView, type UID } from '@simlin/core/datamodel';

import { CloudRadius, StockHeight, StockWidth } from '../../drawing/default';
import {
  CORNER_CLEARANCE,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  PIPE_SPACING,
  VALVE_CLAMP_MARGIN,
  type FlowArm,
} from './flow-invariants';
import type { ViewArm } from './view-invariants';

// ---------------------------------------------------------------------------
// PRNG

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export class Rng {
  private readonly next: () => number;

  constructor(seed: number) {
    this.next = mulberry32(seed);
  }

  /** Uniform in [a, b). */
  float(a: number, b: number): number {
    return a + (b - a) * this.next();
  }

  /** Uniform integer in [a, b]. */
  int(a: number, b: number): number {
    return Math.floor(this.float(a, b + 1));
  }

  bool(p = 0.5): boolean {
    return this.next() < p;
  }

  pick<T>(items: readonly T[]): T {
    return items[Math.floor(this.next() * items.length)];
  }
}

// ---------------------------------------------------------------------------
// Scene records

type Pt = { readonly x: number; readonly y: number };
export type Face = 'left' | 'right' | 'top' | 'bottom';
export const FACES: readonly Face[] = ['left', 'right', 'top', 'bottom'];

export const FLOW_SHAPES = ['straight', 'L', 'Z', 'bracket'] as const;
export type FlowShape = (typeof FLOW_SHAPES)[number];

export const ROUTE_STARTS = ['hubSource', 'hubSink', 'freeCloud'] as const;
export type RouteStart = (typeof ROUTE_STARTS)[number];

/**
 * What the generator built for one flow, so a test can check the view realizes
 * it (and so coverage is counted over what was built, not over what a lenient
 * classification would accept).
 */
export interface RouteRecord {
  readonly flowUid: UID;
  readonly shape: FlowShape;
  /** The walk's start: the hub stock at the flow's source or at its sink, or a free cloud (always the source). */
  readonly start: RouteStart;
  /** The new stock the walk ended at and the face it entered through; undefined when the walk ended at a cloud. */
  readonly far: { readonly stockUid: UID; readonly face: Face } | undefined;
}

const HALF_WIDTH = StockWidth / 2;
const HALF_HEIGHT = StockHeight / 2;

// Clouds and other stocks keep this far from stock bodies so that no later
// mutation (a cloud moved by up to CloudRadius) can land inside one.
const CLOUD_STOCK_GAP = CloudRadius + 6;
const STOCK_STOCK_GAP = 20;

interface StockRecord {
  readonly uid: UID;
  readonly name: string;
  readonly center: Pt;
  /** Positions along each face (from the face's top or left corner) already used by endpoints. */
  readonly slots: Map<Face, number[]>;
}

interface CloudRecord {
  readonly uid: UID;
  readonly flowUid: UID;
  center: Pt;
}

type EndRecord =
  | { readonly kind: 'stock'; readonly stock: StockRecord; readonly face: Face }
  | { readonly kind: 'cloud'; readonly cloud: CloudRecord };

interface FlowRecord {
  readonly uid: UID;
  readonly name: string;
  readonly shape: FlowShape;
  points: Pt[];
  readonly source: EndRecord;
  readonly sink: EndRecord;
  valve: Pt;
  /** Import shape: both endpoints carry no attachment and the flow's clouds are gone. */
  detached: boolean;
}

interface NamedRecord {
  readonly uid: UID;
  readonly name: string;
  readonly center: Pt;
}

interface Builder {
  readonly rng: Rng;
  nextUid: number;
  hubUid: UID;
  readonly routes: RouteRecord[];
  readonly stocks: StockRecord[];
  readonly clouds: CloudRecord[];
  readonly flows: FlowRecord[];
  readonly auxes: NamedRecord[];
  readonly modules: NamedRecord[];
  readonly aliases: Array<{ readonly uid: UID; readonly aliasOfUid: UID; readonly center: Pt }>;
  readonly links: Array<{
    readonly uid: UID;
    readonly fromUid: UID;
    readonly toUid: UID;
    readonly arc: number | undefined;
  }>;
  /** Import shape: extra outflow entries by stock uid. */
  readonly extraOutflows: Map<UID, string[]>;
}

export interface Scene {
  readonly model: Model;
  readonly view: StockFlowView;
  readonly hubUid: UID;
  /** One record per flow, describing the route as generated (before any import shape). */
  readonly routes: readonly RouteRecord[];
}

// ---------------------------------------------------------------------------
// Strict scenes

/**
 * A valid scene: a hub stock with several flows on random faces (endpoints on a
 * shared face at least PIPE_SPACING apart), flows starting at free clouds,
 * each flow a straight, L, Z or bracket route ending at a cloud or at a new
 * stock entered through a perpendicular face, plus auxes, sometimes a module,
 * an alias, and links between named elements.
 */
export function genScene(rng: Rng): Scene {
  return toScene(buildScene(rng));
}

function buildScene(rng: Rng): Builder {
  for (;;) {
    const b = newBuilder(rng);
    const hub = addStock(b, { x: rng.int(350, 550), y: rng.int(350, 550) });
    b.hubUid = hub.uid;
    const hubFlows = rng.int(1, 5);
    for (let i = 0; i < hubFlows; i++) {
      addFlow(b, { kind: 'stock', stock: hub });
    }
    const freeFlows = rng.int(0, 3);
    for (let i = 0; i < freeFlows; i++) {
      addFlow(b, { kind: 'free' });
    }
    if (b.flows.length === 0) {
      continue;
    }
    addAuxesModulesAliasesLinks(b);
    return b;
  }
}

function newBuilder(rng: Rng): Builder {
  return {
    rng,
    nextUid: 1,
    hubUid: 0,
    routes: [],
    stocks: [],
    clouds: [],
    flows: [],
    auxes: [],
    modules: [],
    aliases: [],
    links: [],
    extraOutflows: new Map(),
  };
}

function addStock(b: Builder, center: Pt): StockRecord {
  const uid = b.nextUid++;
  const stock: StockRecord = { uid, name: `Stock ${uid}`, center, slots: new Map() };
  b.stocks.push(stock);
  return stock;
}

type Start = { readonly kind: 'stock'; readonly stock: StockRecord } | { readonly kind: 'free' };

function outward(face: Face): Pt {
  switch (face) {
    case 'left':
      return { x: -1, y: 0 };
    case 'right':
      return { x: 1, y: 0 };
    case 'top':
      return { x: 0, y: -1 };
    case 'bottom':
      return { x: 0, y: 1 };
  }
}

function faceLength(face: Face): number {
  return face === 'left' || face === 'right' ? StockHeight : StockWidth;
}

/** The point `along` px from the face's top (left/right faces) or left (top/bottom faces) corner. */
function facePoint(center: Pt, face: Face, along: number): Pt {
  switch (face) {
    case 'left':
      return { x: center.x - HALF_WIDTH, y: center.y - HALF_HEIGHT + along };
    case 'right':
      return { x: center.x + HALF_WIDTH, y: center.y - HALF_HEIGHT + along };
    case 'top':
      return { x: center.x - HALF_WIDTH + along, y: center.y - HALF_HEIGHT };
    case 'bottom':
      return { x: center.x - HALF_WIDTH + along, y: center.y + HALF_HEIGHT };
  }
}

function turn(d: Pt, left: boolean): Pt {
  return left ? { x: d.y || 0, y: -d.x || 0 } : { x: -d.y || 0, y: d.x || 0 };
}

function opposite(d: Pt): Face {
  if (d.x > 0) return 'left';
  if (d.x < 0) return 'right';
  if (d.y > 0) return 'top';
  return 'bottom';
}

/**
 * Segment directions and lengths for a shape, in walk order. `sinkIndex` is
 * the segment that becomes the flow's final segment (the walk is reversed when
 * the start stock is the sink), so it gets MIN_SINK_SEGMENT; every other
 * segment gets MIN_SEGMENT. A bracket is stub, riser, run, riser back, stub.
 */
function shapeWalk(rng: Rng, shape: FlowShape, d0: Pt, reversed: boolean): Array<{ d: Pt; length: number }> {
  const sinkMin = Math.ceil(MIN_SINK_SEGMENT);
  const min = (index: number, count: number): number => ((reversed ? 0 : count - 1) === index ? sinkMin : MIN_SEGMENT);
  const side = rng.bool();
  switch (shape) {
    case 'straight':
      return [{ d: d0, length: rng.int(Math.max(30, min(0, 1)), 200) }];
    case 'L':
      return [
        { d: d0, length: rng.int(Math.max(20, min(0, 2)), 150) },
        { d: turn(d0, side), length: rng.int(Math.max(20, min(1, 2)), 150) },
      ];
    case 'Z':
      return [
        { d: d0, length: rng.int(Math.max(15, min(0, 3)), 120) },
        { d: turn(d0, side), length: rng.int(20, 100) },
        { d: d0, length: rng.int(Math.max(20, min(2, 3)), 120) },
      ];
    case 'bracket': {
      const riser = rng.int(20, 60);
      return [
        { d: d0, length: rng.int(min(0, 5), min(0, 5) + 8) },
        { d: turn(d0, side), length: riser },
        { d: d0, length: rng.int(30, 150) },
        { d: turn(d0, !side), length: riser },
        { d: d0, length: rng.int(min(4, 5), min(4, 5) + 8) },
      ];
    }
  }
}

function addFlow(b: Builder, start: Start): boolean {
  const rng = b.rng;
  for (let attempt = 0; attempt < 40; attempt++) {
    const shape = rng.pick(FLOW_SHAPES);
    // When the walk starts at a stock that is the flow's SINK, the walk is
    // reversed into points; choose that before sizing segments.
    const reversed = start.kind === 'stock' && rng.bool();
    let p0: Pt;
    let d0: Pt;
    let startFace: Face | undefined;
    let startAlong = 0;
    if (start.kind === 'stock') {
      startFace = rng.pick(FACES);
      const along = pickSlot(rng, start.stock, startFace);
      if (along === undefined) {
        continue;
      }
      startAlong = along;
      p0 = facePoint(start.stock.center, startFace, along);
      d0 = outward(startFace);
    } else {
      p0 = { x: rng.int(0, 900), y: rng.int(0, 900) };
      d0 = outward(rng.pick(FACES));
    }
    const walk = shapeWalk(rng, shape, d0, reversed);
    const pts: Pt[] = [p0];
    for (const step of walk) {
      const last = pts[pts.length - 1];
      pts.push({ x: last.x + step.d.x * step.length, y: last.y + step.d.y * step.length });
    }
    const end = pts[pts.length - 1];
    const finalDirection = walk[walk.length - 1].d;

    let farStock: { center: Pt; face: Face; along: number } | undefined;
    if (rng.bool(0.35)) {
      const face = opposite(finalDirection);
      const along = rng.int(CORNER_CLEARANCE, faceLength(face) - CORNER_CLEARANCE);
      // The final walk point is at `along` on `face` of the new stock.
      const probe = facePoint({ x: 0, y: 0 }, face, along);
      farStock = { center: { x: end.x - probe.x, y: end.y - probe.y }, face, along };
    }

    const terminalStocks = [start.kind === 'stock' ? start.stock.center : undefined, farStock?.center].filter(
      (c): c is Pt => c !== undefined,
    );
    if (!pathIsClear(b, pts, terminalStocks)) {
      continue;
    }
    if (farStock !== undefined) {
      const center = farStock.center;
      if (b.stocks.some((s) => boxGap(stockBox(s.center), stockBox(center)) < STOCK_STOCK_GAP)) continue;
      if (b.clouds.some((c) => distanceToBox(c.center, stockBox(center)) < CLOUD_STOCK_GAP)) continue;
      if (
        b.flows.some((f) =>
          f.points
            .slice(1)
            .some((_, i) => segmentHitsBox(f.points[i], f.points[i + 1], inflateBox(stockBox(center), 2))),
        )
      )
        continue;
    }
    if (start.kind === 'free' && b.stocks.some((s) => distanceToBox(p0, stockBox(s.center)) < CLOUD_STOCK_GAP)) {
      continue;
    }
    if (farStock === undefined && b.stocks.some((s) => distanceToBox(end, stockBox(s.center)) < CLOUD_STOCK_GAP)) {
      continue;
    }

    const flowUid = b.nextUid++;
    let walkStart: EndRecord;
    if (start.kind === 'stock') {
      recordSlot(start.stock, startFace!, startAlong);
      walkStart = { kind: 'stock', stock: start.stock, face: startFace! };
    } else {
      const cloud: CloudRecord = { uid: b.nextUid++, flowUid, center: p0 };
      b.clouds.push(cloud);
      walkStart = { kind: 'cloud', cloud };
    }
    let walkEnd: EndRecord;
    let far: RouteRecord['far'];
    if (farStock !== undefined) {
      const stock = addStock(b, farStock.center);
      recordSlot(stock, farStock.face, farStock.along);
      walkEnd = { kind: 'stock', stock, face: farStock.face };
      far = { stockUid: stock.uid, face: farStock.face };
    } else {
      const cloud: CloudRecord = { uid: b.nextUid++, flowUid, center: end };
      b.clouds.push(cloud);
      walkEnd = { kind: 'cloud', cloud };
    }
    const points = reversed ? [...pts].reverse() : pts;
    const flow: FlowRecord = {
      uid: flowUid,
      name: `Flow ${flowUid}`,
      shape,
      points,
      source: reversed ? walkEnd : walkStart,
      sink: reversed ? walkStart : walkEnd,
      valve: placeValve(rng, points),
      detached: false,
    };
    b.flows.push(flow);
    b.routes.push({
      flowUid,
      shape,
      start: start.kind === 'free' ? 'freeCloud' : reversed ? 'hubSink' : 'hubSource',
      far,
    });
    return true;
  }
  return false;
}

function pickSlot(rng: Rng, stock: StockRecord, face: Face): number | undefined {
  const used = stock.slots.get(face) ?? [];
  for (let attempt = 0; attempt < 12; attempt++) {
    const along = rng.bool(0.4)
      ? rng.pick([faceLength(face) / 2, faceLength(face) / 4, (3 * faceLength(face)) / 4])
      : rng.int(CORNER_CLEARANCE, faceLength(face) - CORNER_CLEARANCE);
    if (used.every((u) => Math.abs(u - along) >= PIPE_SPACING)) {
      return along;
    }
  }
  return undefined;
}

function recordSlot(stock: StockRecord, face: Face, along: number): void {
  const used = stock.slots.get(face) ?? [];
  used.push(along);
  stock.slots.set(face, used);
}

/**
 * A new path must not pass through any stock other than its terminals
 * (inflated by 2px so it never grazes one), must not enter its terminal stocks'
 * interiors, and must not touch another flow's endpoint cloud.
 */
function pathIsClear(b: Builder, pts: readonly Pt[], terminalCenters: readonly Pt[]): boolean {
  const isTerminal = (center: Pt): boolean => terminalCenters.some((c) => c.x === center.x && c.y === center.y);
  for (let i = 0; i < pts.length - 1; i++) {
    for (const s of b.stocks) {
      const box = isTerminal(s.center) ? inflateBox(stockBox(s.center), -0.01) : inflateBox(stockBox(s.center), 2);
      if (segmentHitsBox(pts[i], pts[i + 1], box)) {
        return false;
      }
    }
    for (const c of terminalCenters) {
      if (!b.stocks.some((s) => s.center === c) && segmentHitsBox(pts[i], pts[i + 1], inflateBox(stockBox(c), -0.01))) {
        return false;
      }
    }
    for (const c of b.clouds) {
      if (distanceToSegment(c.center, pts[i], pts[i + 1]) < CloudRadius) {
        return false;
      }
    }
  }
  return true;
}

function placeValve(rng: Rng, pts: readonly Pt[]): Pt {
  const length = pathLength(pts);
  const s = rng.bool(0.3) ? length / 2 : rng.float(VALVE_CLAMP_MARGIN + 1, length - VALVE_CLAMP_MARGIN - 1);
  return pointAtArc(pts, s);
}

function addAuxesModulesAliasesLinks(b: Builder): void {
  const rng = b.rng;
  const clearSpot = (): Pt | undefined => {
    for (let attempt = 0; attempt < 30; attempt++) {
      const p = { x: rng.int(0, 900), y: rng.int(0, 900) };
      const nearStock = b.stocks.some((s) => distanceToBox(p, stockBox(s.center)) < 20);
      const nearPath = b.flows.some((f) =>
        f.points.slice(1).some((_, i) => distanceToSegment(p, f.points[i], f.points[i + 1]) < 15),
      );
      const nearCloud = b.clouds.some((c) => Math.hypot(c.center.x - p.x, c.center.y - p.y) < 25);
      if (!nearStock && !nearPath && !nearCloud) {
        return p;
      }
    }
    return undefined;
  };
  const auxCount = rng.int(1, 3);
  for (let i = 0; i < auxCount; i++) {
    const p = clearSpot();
    if (p !== undefined) {
      const uid = b.nextUid++;
      b.auxes.push({ uid, name: `Aux ${uid}`, center: p });
    }
  }
  if (rng.bool(0.3)) {
    const p = clearSpot();
    if (p !== undefined) {
      const uid = b.nextUid++;
      b.modules.push({ uid, name: `Module ${uid}`, center: p });
    }
  }
  const aliasable = [...b.stocks, ...b.auxes];
  if (rng.bool(0.7) && aliasable.length > 0) {
    const p = clearSpot();
    if (p !== undefined) {
      b.aliases.push({ uid: b.nextUid++, aliasOfUid: rng.pick(aliasable).uid, center: p });
    }
  }
  const linkable: UID[] = [
    ...b.stocks.map((s) => s.uid),
    ...b.flows.map((f) => f.uid),
    ...b.auxes.map((a) => a.uid),
    ...b.modules.map((m) => m.uid),
    ...b.aliases.map((a) => a.uid),
  ];
  const linkCount = rng.int(1, 4);
  for (let i = 0; i < linkCount && linkable.length > 1; i++) {
    const fromUid = rng.pick(linkable);
    const toUid = rng.pick(linkable.filter((u) => u !== fromUid));
    b.links.push({ uid: b.nextUid++, fromUid, toUid, arc: rng.bool() ? undefined : rng.int(-60, 60) });
  }
}

// ---------------------------------------------------------------------------
// Imported scenes

export const IMPORT_SHAPES = [
  'offFaceAxis',
  'cornerEndpoint',
  'cloudOffset',
  'valveOffPath',
  'slightlyDiagonal',
  'unattachedFlow',
  'listedInTwoStocks',
] as const;
export type ImportShape = (typeof IMPORT_SHAPES)[number];

export interface ImportMutation {
  readonly shape: ImportShape;
  readonly flowUid: UID;
  /** Arms strict mode must report for this flow, one entry per occurrence. */
  readonly flowArms: readonly FlowArm[];
  /** Stock-list arms this mutation must produce. */
  readonly viewArms: readonly ViewArm[];
}

export interface ImportedScene extends Scene {
  readonly mutations: readonly ImportMutation[];
}

/**
 * A strict scene with one to three corpus shapes applied, each to a different
 * flow:
 * - offFaceAxis: a stock endpoint moved 5-60px outward along its stub (Vensim
 *   stocks larger than 45x35 put endpoints on their real faces);
 * - cornerEndpoint: a straight stock-cloud flow shifted so its stock endpoint
 *   sits exactly on a corner;
 * - cloudOffset: a cloud moved up to CloudRadius off its endpoint;
 * - valveOffPath: the valve moved 1-20px perpendicular off its segment;
 * - slightlyDiagonal: a straight cloud-cloud flow's sink moved 0.5-2px across;
 * - unattachedFlow: a cloud-cloud flow with both attachments and its clouds
 *   removed (Vensim fallback flows);
 * - listedInTwoStocks: a stock-sourced flow also listed in another stock's
 *   outflows (XMILE imports).
 * Every precondition is chosen so the mutation produces exactly its recorded
 * arms and nothing else.
 */
export function genImportedScene(rng: Rng): ImportedScene {
  for (;;) {
    const b = buildScene(rng);
    const want = rng.int(1, 3);
    const order = shuffle(rng, [...IMPORT_SHAPES]);
    const used = new Set<UID>();
    const mutations: ImportMutation[] = [];
    for (const shape of order) {
      if (mutations.length >= want) {
        break;
      }
      const m = applyImportShape(b, shape, used);
      if (m !== undefined) {
        used.add(m.flowUid);
        mutations.push(m);
      }
    }
    if (mutations.length > 0) {
      return { ...toScene(b), mutations };
    }
  }
}

function shuffle<T>(rng: Rng, items: T[]): T[] {
  for (let i = items.length - 1; i > 0; i--) {
    const j = rng.int(0, i);
    [items[i], items[j]] = [items[j], items[i]];
  }
  return items;
}

function applyImportShape(b: Builder, shape: ImportShape, used: ReadonlySet<UID>): ImportMutation | undefined {
  const rng = b.rng;
  const candidates = b.flows.filter((f) => !used.has(f.uid));
  switch (shape) {
    case 'offFaceAxis': {
      for (const f of shuffle(rng, [...candidates])) {
        const ends = [0, f.points.length - 1].filter((i) => (i === 0 ? f.source : f.sink).kind === 'stock');
        for (const index of ends) {
          const adjacent = index === 0 ? f.points[1] : f.points[f.points.length - 2];
          const endpoint = f.points[index];
          const stubLength = Math.hypot(adjacent.x - endpoint.x, adjacent.y - endpoint.y);
          const valveArc = arcPosition(f.points, f.valve);
          const valveFromEnd = index === 0 ? valveArc : pathLength(f.points) - valveArc;
          const d = rng.int(5, 60);
          if (stubLength < d + MIN_SINK_SEGMENT + 1 || valveFromEnd < d + VALVE_CLAMP_MARGIN + 1) {
            continue;
          }
          const ux = (adjacent.x - endpoint.x) / stubLength;
          const uy = (adjacent.y - endpoint.y) / stubLength;
          f.points[index] = { x: endpoint.x + ux * d, y: endpoint.y + uy * d };
          return { shape, flowUid: f.uid, flowArms: ['G4.offFace'], viewArms: [] };
        }
      }
      return undefined;
    }
    case 'cornerEndpoint': {
      for (const f of shuffle(rng, [...candidates])) {
        if (f.points.length !== 2) continue;
        const stockEnd = f.source.kind === 'stock' ? f.source : f.sink.kind === 'stock' ? f.sink : undefined;
        const cloudEnd = f.source.kind === 'cloud' ? f.source : f.sink.kind === 'cloud' ? f.sink : undefined;
        if (stockEnd === undefined || cloudEnd === undefined) continue;
        const index = f.source.kind === 'stock' ? 0 : 1;
        const endpoint = f.points[index];
        const corner = facePoint(stockEnd.stock.center, stockEnd.face, rng.bool() ? 0 : faceLength(stockEnd.face));
        const shift = { x: corner.x - endpoint.x, y: corner.y - endpoint.y };
        const moved = f.points.map((p) => ({ x: p.x + shift.x, y: p.y + shift.y }));
        const cloudCenter = { x: cloudEnd.cloud.center.x + shift.x, y: cloudEnd.cloud.center.y + shift.y };
        const others = b.stocks.filter((s) => s.uid !== stockEnd.stock.uid);
        if (others.some((s) => segmentHitsBox(moved[0], moved[1], inflateBox(stockBox(s.center), 2)))) continue;
        // A shifted cloud near a stock could land inside it (G6.cloudInsideStock)
        // or crowd G3/G6 exemptions, adding arms this mutation does not record.
        // The strict scene's own gaps keep every test seed clear of this guard, so
        // no test observes it; it holds the exactness contract for seeds that do.
        if (b.stocks.some((s) => distanceToBox(cloudCenter, stockBox(s.center)) < CLOUD_STOCK_GAP)) continue;
        f.points = moved;
        f.valve = { x: f.valve.x + shift.x, y: f.valve.y + shift.y };
        cloudEnd.cloud.center = cloudCenter;
        return { shape, flowUid: f.uid, flowArms: ['G4.cornerClearance'], viewArms: [] };
      }
      return undefined;
    }
    case 'cloudOffset': {
      for (const f of shuffle(rng, [...candidates])) {
        const cloudEnd = f.source.kind === 'cloud' ? f.source : f.sink.kind === 'cloud' ? f.sink : undefined;
        if (cloudEnd === undefined) continue;
        const r = rng.float(0.5, CloudRadius);
        const angle = rng.float(0, 2 * Math.PI);
        const center = {
          x: cloudEnd.cloud.center.x + r * Math.cos(angle),
          y: cloudEnd.cloud.center.y + r * Math.sin(angle),
        };
        if (b.stocks.some((s) => distanceToBox(center, stockBox(s.center)) < 1)) continue;
        cloudEnd.cloud.center = center;
        return { shape, flowUid: f.uid, flowArms: ['G7.cloudOffEndpoint'], viewArms: [] };
      }
      return undefined;
    }
    case 'valveOffPath': {
      for (const f of shuffle(rng, [...candidates])) {
        const segment = segmentAtArc(f.points, arcPosition(f.points, f.valve));
        const a = f.points[segment];
        const c = f.points[segment + 1];
        const length = Math.hypot(c.x - a.x, c.y - a.y);
        const offset = rng.float(1, 20) * (rng.bool() ? 1 : -1);
        const valve = {
          x: f.valve.x - ((c.y - a.y) / length) * offset,
          y: f.valve.y + ((c.x - a.x) / length) * offset,
        };
        // Near a corner, moving the valve off its own segment can put it on the
        // adjacent one, and then G8.valveOffPath would not fire. No test seed reaches
        // this guard, so no test observes it; it holds the exactness contract for
        // seeds that do.
        if (distanceToPath(valve, f.points) < 0.5) continue;
        f.valve = valve;
        return { shape, flowUid: f.uid, flowArms: ['G8.valveOffPath'], viewArms: [] };
      }
      return undefined;
    }
    case 'slightlyDiagonal': {
      for (const f of shuffle(rng, [...candidates])) {
        if (f.points.length !== 2 || f.source.kind !== 'cloud' || f.sink.kind !== 'cloud') continue;
        const [a, c] = f.points;
        const length = Math.hypot(c.x - a.x, c.y - a.y);
        const t = arcPosition(f.points, f.valve) / length;
        const across = rng.float(0.5, 2) * (rng.bool() ? 1 : -1);
        const sink = { x: c.x - ((c.y - a.y) / length) * across, y: c.y + ((c.x - a.x) / length) * across };
        const newLength = Math.hypot(sink.x - a.x, sink.y - a.y);
        if (t * newLength < VALVE_CLAMP_MARGIN + 0.5 || (1 - t) * newLength < VALVE_CLAMP_MARGIN + 0.5) continue;
        f.points = [a, sink];
        f.sink.cloud.center = sink;
        f.valve = { x: a.x + (sink.x - a.x) * t, y: a.y + (sink.y - a.y) * t };
        return { shape, flowUid: f.uid, flowArms: ['G2.diagonal'], viewArms: [] };
      }
      return undefined;
    }
    case 'unattachedFlow': {
      for (const f of shuffle(rng, [...candidates])) {
        if (f.source.kind !== 'cloud' || f.sink.kind !== 'cloud') continue;
        f.detached = true;
        return { shape, flowUid: f.uid, flowArms: ['G1.unattachedEndpoint', 'G1.unattachedEndpoint'], viewArms: [] };
      }
      return undefined;
    }
    case 'listedInTwoStocks': {
      for (const f of shuffle(rng, [...candidates])) {
        if (f.source.kind !== 'stock' || f.detached) continue;
        const sourceUid = f.source.stock.uid;
        const others = b.stocks.filter((s) => s.uid !== sourceUid);
        if (others.length === 0) continue;
        const other = rng.pick(others);
        const extra = b.extraOutflows.get(other.uid) ?? [];
        extra.push(f.name);
        b.extraOutflows.set(other.uid, extra);
        return { shape, flowUid: f.uid, flowArms: [], viewArms: ['stockLists.listedNotAttached'] };
      }
      return undefined;
    }
  }
}

// ---------------------------------------------------------------------------
// JSON model assembly

function toScene(b: Builder): Scene {
  const model = modelFromJson(toJsonModel(b));
  return { model, view: model.views[0], hubUid: b.hubUid, routes: b.routes };
}

function toJsonModel(b: Builder): JsonModel {
  const elements: JsonViewElement[] = [];
  for (const s of b.stocks) {
    elements.push({ type: 'stock', uid: s.uid, name: s.name, x: s.center.x, y: s.center.y });
  }
  const detached = new Set(b.flows.filter((f) => f.detached).map((f) => f.uid));
  for (const c of b.clouds) {
    if (!detached.has(c.flowUid)) {
      elements.push({ type: 'cloud', uid: c.uid, flowUid: c.flowUid, x: c.center.x, y: c.center.y });
    }
  }
  const endUid = (end: EndRecord): UID => (end.kind === 'stock' ? end.stock.uid : end.cloud.uid);
  for (const f of b.flows) {
    elements.push({
      type: 'flow',
      uid: f.uid,
      name: f.name,
      x: f.valve.x,
      y: f.valve.y,
      points: f.points.map((p, i) => {
        const attached = f.detached
          ? undefined
          : i === 0
            ? endUid(f.source)
            : i === f.points.length - 1
              ? endUid(f.sink)
              : undefined;
        return attached === undefined ? { x: p.x, y: p.y } : { x: p.x, y: p.y, attachedToUid: attached };
      }),
    });
  }
  for (const a of b.auxes) {
    elements.push({ type: 'aux', uid: a.uid, name: a.name, x: a.center.x, y: a.center.y });
  }
  for (const m of b.modules) {
    elements.push({ type: 'module', uid: m.uid, name: m.name, x: m.center.x, y: m.center.y });
  }
  for (const a of b.aliases) {
    elements.push({ type: 'alias', uid: a.uid, aliasOfUid: a.aliasOfUid, x: a.center.x, y: a.center.y });
  }
  for (const l of b.links) {
    elements.push(
      l.arc === undefined
        ? { type: 'link', uid: l.uid, fromUid: l.fromUid, toUid: l.toUid }
        : { type: 'link', uid: l.uid, fromUid: l.fromUid, toUid: l.toUid, arc: l.arc },
    );
  }

  const listed = (stock: StockRecord, end: 'source' | 'sink'): string[] =>
    b.flows
      .filter((f) => !f.detached)
      .filter((f) => {
        const e = end === 'source' ? f.source : f.sink;
        return e.kind === 'stock' && e.stock.uid === stock.uid;
      })
      .map((f) => f.name);

  return {
    name: 'main',
    stocks: b.stocks.map((s) => ({
      name: s.name,
      initialEquation: '1',
      inflows: listed(s, 'sink'),
      outflows: [...listed(s, 'source'), ...(b.extraOutflows.get(s.uid) ?? [])],
    })),
    flows: b.flows.map((f) => ({ name: f.name, equation: '1' })),
    auxiliaries: b.auxes.map((a) => ({ name: a.name, equation: '1' })),
    modules: b.modules.map((m) => ({ name: m.name, modelName: 'sub' })),
    views: [{ elements, viewBox: { x: 0, y: 0, width: 1000, height: 1000 }, zoom: 1 }],
  } as JsonModel;
}

// ---------------------------------------------------------------------------
// Generator geometry (independent of the checker)

interface Box {
  readonly minX: number;
  readonly maxX: number;
  readonly minY: number;
  readonly maxY: number;
}

function stockBox(center: Pt): Box {
  return {
    minX: center.x - HALF_WIDTH,
    maxX: center.x + HALF_WIDTH,
    minY: center.y - HALF_HEIGHT,
    maxY: center.y + HALF_HEIGHT,
  };
}

function inflateBox(box: Box, by: number): Box {
  return { minX: box.minX - by, maxX: box.maxX + by, minY: box.minY - by, maxY: box.maxY + by };
}

function boxGap(a: Box, b: Box): number {
  const gx = Math.max(a.minX - b.maxX, b.minX - a.maxX, 0);
  const gy = Math.max(a.minY - b.maxY, b.minY - a.maxY, 0);
  return Math.max(gx, gy);
}

function distanceToBox(p: Pt, box: Box): number {
  const dx = Math.max(box.minX - p.x, 0, p.x - box.maxX);
  const dy = Math.max(box.minY - p.y, 0, p.y - box.maxY);
  return Math.hypot(dx, dy);
}

// Axis-aligned segments only (the generator never produces a diagonal before a
// mutation): does the segment overlap the open box with positive length?
function segmentHitsBox(a: Pt, b: Pt, box: Box): boolean {
  if (a.y === b.y) {
    const lo = Math.min(a.x, b.x);
    const hi = Math.max(a.x, b.x);
    return a.y > box.minY && a.y < box.maxY && Math.min(hi, box.maxX) - Math.max(lo, box.minX) > 0;
  }
  const lo = Math.min(a.y, b.y);
  const hi = Math.max(a.y, b.y);
  return a.x > box.minX && a.x < box.maxX && Math.min(hi, box.maxY) - Math.max(lo, box.minY) > 0;
}

function distanceToSegment(p: Pt, a: Pt, b: Pt): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const l2 = dx * dx + dy * dy;
  const t = l2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l2));
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

function distanceToPath(p: Pt, pts: readonly Pt[]): number {
  let best = Infinity;
  for (let i = 0; i < pts.length - 1; i++) {
    best = Math.min(best, distanceToSegment(p, pts[i], pts[i + 1]));
  }
  return best;
}

function pathLength(pts: readonly Pt[]): number {
  let total = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    total += Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
  }
  return total;
}

function pointAtArc(pts: readonly Pt[], s: number): Pt {
  let remaining = s;
  for (let i = 0; i < pts.length - 1; i++) {
    const length = Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
    if (remaining <= length || i === pts.length - 2) {
      const t = length === 0 ? 0 : remaining / length;
      return { x: pts[i].x + (pts[i + 1].x - pts[i].x) * t, y: pts[i].y + (pts[i + 1].y - pts[i].y) * t };
    }
    remaining -= length;
  }
  return pts[pts.length - 1];
}

function arcPosition(pts: readonly Pt[], p: Pt): number {
  let best = Infinity;
  let position = 0;
  let traversed = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    const a = pts[i];
    const c = pts[i + 1];
    const length = Math.hypot(c.x - a.x, c.y - a.y);
    const l2 = length * length;
    const t = l2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * (c.x - a.x) + (p.y - a.y) * (c.y - a.y)) / l2));
    const d = Math.hypot(p.x - (a.x + t * (c.x - a.x)), p.y - (a.y + t * (c.y - a.y)));
    if (d < best) {
      best = d;
      position = traversed + t * length;
    }
    traversed += length;
  }
  return position;
}

function segmentAtArc(pts: readonly Pt[], s: number): number {
  let traversed = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    const length = Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
    // A valve exactly on a corner belongs to the segment it would slide along
    // perpendicular-off from; either works, so take the earlier one.
    if (s <= traversed + length) {
      return i;
    }
    traversed += length;
  }
  return pts.length - 2;
}
