// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Route search: `route` and `routeEnd`.
 *
 * A route between two ports is an orthogonal polyline with k bends. Its
 * segments alternate axes starting with A0, so it is fully described by the
 * coordinate each segment HOLDS constant: h0 (the source endpoint's coordinate
 * across A0), interior holds h1..h(k-1), and hk (the sink endpoint's across the
 * last axis). For a face port h0/hk is the endpoint's position along the face,
 * the only free coordinate an attached endpoint has. Interior holds come from
 * candidates that are continuous functions of the terminals and belong to the
 * port pair being generated (so a tie between two pairs never switches on a
 * third pair's feasibility); face positions are solved from a preference. The
 * winning route therefore changes discontinuously only when the ranking changes
 * winner: when feasibility changes, which E3 documents.
 */

import type { FlowViewElement } from '@simlin/core/datamodel';

import {
  type Axis,
  type Box,
  boxesOverlap,
  clamp,
  compose,
  CORNER_CLEARANCE,
  coord,
  FACES,
  type FlowEnd,
  GEOMETRY_EPSILON,
  inflate,
  isFiniteXY,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  otherAxis,
  PIPE_SPACING,
  segmentAxisOf,
  stockBody,
  type XY,
} from './geometry';
import { normalize, pathLength, placeValve, valveDistance } from './path';
import {
  type FaceAttachment,
  faceAttachment,
  facePoint,
  type FlowGeometry,
  stubTip,
  type Terminal,
  terminalIsFinite,
  type Terminals,
  withGeometry,
} from './terminal';
import { FAULT_NONE, pathQuality } from './validity';

interface FacePort {
  readonly kind: 'face';
  readonly att: FaceAttachment;
  /** The preferred along-face position, within [lo, hi]. */
  readonly pref: number;
  readonly pinned: boolean;
  /** The terminal had a base face and this is not it. */
  readonly offBase: boolean;
}

interface PointPort {
  readonly kind: 'point';
  readonly point: XY;
  /** The axis the adjacent segment must run along (a preserved corner), or undefined (any). */
  readonly axis: Axis | undefined;
}

type Port = FacePort | PointPort;

/**
 * The ports a terminal offers. A pinned stock terminal offers only its base face
 * at its base offset (routeEnd's fixed end). An unpinned stock offers all four
 * faces: the base face prefers the base offset, any other face the slot the
 * routing preference picks.
 */
function portsOf(t: Terminal, pinned: boolean, occupied: readonly XY[]): Port[] {
  if (t.kind === 'free') {
    return [{ kind: 'point', point: t.point, axis: undefined }];
  }
  const ports: FacePort[] = [];
  for (const face of FACES) {
    if (pinned && t.face !== undefined && face !== t.face) {
      continue;
    }
    const att = faceAttachment(t.stock, face);
    const isBase = t.face === face;
    const pref =
      isBase && t.offset !== undefined ? clamp(att.center + t.offset, att.lo, att.hi) : slotPreference(att, occupied);
    ports.push({ kind: 'face', att, pref, pinned: pinned && isBase, offBase: t.face !== undefined && !isBase });
  }
  return ports;
}

/**
 * The routing preference for a flow newly landing on a face: the position
 * nearest the face center at least PIPE_SPACING from every existing endpoint on
 * the face, else the position maximizing the minimum distance to them.
 */
export function slotPreference(att: FaceAttachment, occupied: readonly XY[]): number {
  const used = occupied
    .filter(
      (p) =>
        Math.abs(coord(p, att.normal) - att.plane) <= GEOMETRY_EPSILON &&
        coord(p, att.along) >= att.lo - CORNER_CLEARANCE - GEOMETRY_EPSILON &&
        coord(p, att.along) <= att.hi + CORNER_CLEARANCE + GEOMETRY_EPSILON,
    )
    .map((p) => coord(p, att.along));
  if (used.length === 0) {
    return att.center;
  }
  const minDistance = (v: number): number => Math.min(...used.map((u) => Math.abs(u - v)));
  const inRange = (v: number): boolean => v >= att.lo - GEOMETRY_EPSILON && v <= att.hi + GEOMETRY_EPSILON;
  const nearestCenter = (vs: readonly number[]): number =>
    vs.reduce((best, v) => (Math.abs(v - att.center) < Math.abs(best - att.center) ? v : best));
  const candidates = [att.center, att.lo, att.hi, ...used.flatMap((u) => [u - PIPE_SPACING, u + PIPE_SPACING])];
  const spaced = candidates.filter((v) => inRange(v) && minDistance(v) >= PIPE_SPACING - GEOMETRY_EPSILON);
  if (spaced.length > 0) {
    return nearestCenter(spaced);
  }
  const sorted = [...used].sort((a, b) => a - b);
  const gaps = [att.lo, att.hi, ...sorted.slice(1).map((u, i) => (u + sorted[i]) / 2)].filter(inRange);
  const best = Math.max(...gaps.map(minDistance));
  return nearestCenter(gaps.filter((v) => minDistance(v) >= best - GEOMETRY_EPSILON));
}

type AlongConstraint =
  | { readonly kind: 'half'; readonly from: number; readonly sign: 1 | -1 }
  | { readonly kind: 'away'; readonly t: number; readonly m: number };

/**
 * The along-face position nearest `pref` that satisfies `c`: a half-line (the
 * position must be at least `from` in direction `sign`) or a minimum distance
 * from `t`. Infeasible constraints return the clamped preference, which the
 * validity check then rejects.
 */
function solveAlong(port: FacePort, pref: number, c: AlongConstraint): number {
  if (port.pinned) {
    return port.pref;
  }
  const { lo, hi } = port.att;
  const v = clamp(pref, lo, hi);
  if (c.kind === 'half') {
    const a = c.sign > 0 ? Math.max(lo, c.from) : lo;
    const b = c.sign < 0 ? Math.min(hi, c.from) : hi;
    return a <= b ? clamp(v, a, b) : v;
  }
  if (Math.abs(v - c.t) >= c.m - GEOMETRY_EPSILON) {
    return v;
  }
  const options = [c.t - c.m, c.t + c.m].filter((o) => o >= lo - GEOMETRY_EPSILON && o <= hi + GEOMETRY_EPSILON);
  if (options.length === 0) {
    return v;
  }
  return options.reduce((best, o) => (Math.abs(o - v) < Math.abs(best - v) ? o : best));
}

interface Candidate {
  readonly points: XY[];
  readonly bends: number;
  readonly sticky: number;
  readonly axisChange: number;
  readonly length: number;
  readonly fault: number;
  /** A best effort G6 miss on a valid candidate (see PathQuality), honored as a preference. */
  readonly crossing: boolean;
  /** Passes through a non-terminal stock (see PathQuality); worth generating detours to avoid. */
  readonly obstructed: boolean;
  readonly index: number;
}

interface Search {
  readonly sources: readonly Port[];
  readonly sinks: readonly Port[];
  /** Minimum length of the segment adjacent to each port. */
  readonly minSource: number;
  readonly minSink: number;
  readonly terminals: Terminals;
  /** Stocks besides the terminals a candidate should not pass through; one that does ranks as crossing (see pathQuality). */
  readonly obstacles: readonly XY[];
  /** Turns a generated path into the flow's full path (routeEnd prepends or appends a preserved prefix). */
  readonly assemble: (path: XY[]) => XY[];
  /** Candidates a caller refuses outright (a tail folding back over its preserved prefix). */
  readonly refuse: (points: readonly XY[]) => boolean;
  readonly baseFirstAxis: Axis | undefined;
  readonly baseLastAxis: Axis | undefined;
  /**
   * The most bends a candidate is ranked with: 2 for a route between two
   * terminals (straight, L, Z), 3 for a tail a preserved corner forces into a
   * turn. Past that only `detours` generates more.
   */
  readonly maxBends: number;
  /** When nothing valid exists, also try U turns and up to four bends (never for routeEnd's pinned attempt). */
  readonly detours: boolean;
  /** Base corner coordinates per axis: the holds that keep an existing shape. */
  readonly baseHolds: { readonly x: readonly number[]; readonly y: readonly number[] };
  /** A minimum riser either side of each point port, per axis: ahead of the pair midpoint. */
  readonly pointPools: { readonly x: readonly number[]; readonly y: readonly number[] };
  /** Pair-independent hold candidates per axis after the midpoint (point coordinates, stub tips, body clearances). */
  readonly pools: { readonly x: readonly number[]; readonly y: readonly number[] };
}

function portRef(port: Port): XY {
  return port.kind === 'point' ? port.point : facePoint(port.att, port.pref);
}

/** The port's stub tip on `axis` (a face whose normal is `axis`), else its reference coordinate. */
function tipOf(port: Port, axis: Axis, min: number): number {
  return port.kind === 'face' && port.att.normal === axis ? stubTip(port.att, min) : coord(portRef(port), axis);
}

function dedupe(values: readonly number[]): number[] {
  const out: number[] = [];
  for (const v of values) {
    if (Number.isFinite(v) && !out.some((o) => Math.abs(o - v) <= GEOMETRY_EPSILON)) {
      out.push(v);
    }
  }
  return out;
}

/**
 * The box a route between the search's ports can be expected to pass through:
 * the terminal bodies and port references, inflated by two minimum segments.
 * Only obstacles meeting it contribute clearance holds, so a view's distant
 * stocks do not multiply the candidates of every search.
 */
function reachBox(search: Omit<Search, 'pools' | 'pointPools' | 'baseHolds'>): Box {
  const box = { minX: Infinity, maxX: -Infinity, minY: Infinity, maxY: -Infinity };
  const add = (b: Box): void => {
    box.minX = Math.min(box.minX, b.minX);
    box.maxX = Math.max(box.maxX, b.maxX);
    box.minY = Math.min(box.minY, b.minY);
    box.maxY = Math.max(box.maxY, b.maxY);
  };
  const pointBox = (p: XY): Box => ({ minX: p.x, maxX: p.x, minY: p.y, maxY: p.y });
  for (const t of [search.terminals.source, search.terminals.sink]) {
    add(t.kind === 'stock' ? stockBody(t.stock) : pointBox(t.point));
  }
  for (const port of [...search.sources, ...search.sinks]) {
    add(pointBox(portRef(port)));
  }
  return inflate(box, 2 * MIN_SEGMENT);
}

/**
 * Pair-independent hold candidates per axis. `pointPools` is the minimum
 * distance either side of a point port: a Z whose riser hugs a cloud turns into
 * the L the cloud reaches with the least change, and a tail leaves a preserved
 * corner by exactly a minimum riser, so these rank ahead of a pair's midpoint.
 * `pools` follows the midpoint: point coordinates, stub tips, and clearances
 * around each terminal body and each obstacle within reach (a route can only go
 * around a body whose clearance is a hold it can take).
 */
function buildPools(search: Omit<Search, 'pools' | 'pointPools' | 'baseHolds'>): Pick<Search, 'pools' | 'pointPools'> {
  const reach = reachBox(search);
  const nearObstacles = search.obstacles.filter((center) => boxesOverlap(stockBody(center), reach));
  const pool = (axis: Axis): { point: number[]; rest: number[] } => {
    const point: number[] = [];
    const rest: number[] = [];
    for (const [ports, min] of [
      [search.sources, search.minSource],
      [search.sinks, search.minSink],
    ] as const) {
      for (const port of ports) {
        if (port.kind === 'point') {
          point.push(coord(port.point, axis) - min, coord(port.point, axis) + min);
          rest.push(coord(port.point, axis));
        } else if (port.att.normal === axis) {
          rest.push(stubTip(port.att, min));
        }
      }
    }
    const bodies: readonly XY[] = [
      ...[search.terminals.source, search.terminals.sink].flatMap((t): XY[] => (t.kind === 'stock' ? [t.stock] : [])),
      ...nearObstacles,
    ];
    for (const center of bodies) {
      const body = stockBody(center);
      rest.push(axis === 'x' ? body.minX - MIN_SEGMENT : body.minY - MIN_SEGMENT);
      rest.push(axis === 'x' ? body.maxX + MIN_SEGMENT : body.maxY + MIN_SEGMENT);
    }
    return { point: dedupe(point), rest: dedupe(rest) };
  };
  const x = pool('x');
  const y = pool('y');
  return { pointPools: { x: x.point, y: y.point }, pools: { x: x.rest, y: y.rest } };
}

/**
 * The hold candidates for a port pair on `axis`, in priority order: each base
 * corner clamped into the pair's feasible band (between the two ports' stub
 * tips), the point-port clearances, the band's midpoint, then the shared pools.
 * Clamping keeps an existing run where it is while the band covers it and
 * follows the band's edge when it does not, so the hold is a continuous function
 * of the terminals; the midpoint is the pair's own fallback, never another
 * pair's.
 */
function holdCandidates(search: Search, P: Port, Q: Port, axis: Axis): number[] {
  const a = tipOf(P, axis, search.minSource);
  const b = tipOf(Q, axis, search.minSink);
  const lo = Math.min(a, b);
  const hi = Math.max(a, b);
  return dedupe([
    ...search.baseHolds[axis].map((v) => clamp(v, lo, hi)),
    ...search.pointPools[axis],
    (a + b) / 2,
    ...search.pools[axis],
  ]);
}

/**
 * Which two-bend shapes to emit. A plain route's two-bend shape is a Z (it
 * leaves and arrives travelling the same way); a U (arriving travelling back)
 * is a detour, generated only when nothing else is valid. A tail ending at a
 * preserved corner may need either, so both are emitted with the other shapes.
 */
type ShapeTier = 'shapes' | 'uTurns';

function generate(search: Search, k: number, out: Candidate[], tier: ShapeTier = 'shapes'): void {
  for (const P of search.sources) {
    for (const Q of search.sinks) {
      for (const A0 of ['x', 'y'] as const) {
        generateShape(search, P, Q, k, A0, out, tier);
      }
    }
  }
}

function generateShape(search: Search, P: Port, Q: Port, k: number, A0: Axis, out: Candidate[], tier: ShapeTier): void {
  const preserved = (P.kind === 'point' && P.axis !== undefined) || (Q.kind === 'point' && Q.axis !== undefined);
  if (tier === 'uTurns' && (k !== 2 || preserved)) {
    return;
  }
  const A1 = otherAxis(A0);
  const Ak = k % 2 === 0 ? A0 : A1;
  if (P.kind === 'face' ? P.att.normal !== A0 : P.axis !== undefined && P.axis !== A0) {
    return;
  }
  if (Q.kind === 'face' ? Q.att.normal !== Ak : Q.axis !== undefined && Q.axis !== Ak) {
    return;
  }
  const e = GEOMETRY_EPSILON;
  const start = P.kind === 'face' ? P.att.plane : coord(P.point, A0);
  const end = Q.kind === 'face' ? Q.att.plane : coord(Q.point, Ak);

  if (k === 0) {
    let v: number;
    if (P.kind === 'face' && Q.kind === 'face') {
      const lo = Math.max(P.att.lo, Q.att.lo);
      const hi = Math.min(P.att.hi, Q.att.hi);
      if (lo > hi + e) {
        return;
      }
      // Unpinned, a straight between two faces splits the difference between
      // the two preferences: each end gives way equally.
      v = P.pinned ? P.pref : Q.pinned ? Q.pref : clamp((P.pref + Q.pref) / 2, lo, hi);
    } else if (P.kind === 'face' && Q.kind === 'point') {
      v = coord(Q.point, A1);
      if (!alongAccepts(P, v)) {
        return;
      }
    } else if (P.kind === 'point' && Q.kind === 'face') {
      v = coord(P.point, A1);
      if (!alongAccepts(Q, v)) {
        return;
      }
    } else if (P.kind === 'point' && Q.kind === 'point') {
      v = coord(P.point, A1);
      if (Math.abs(coord(Q.point, A1) - v) > e) {
        return;
      }
    } else {
      return;
    }
    emit(search, P, Q, [compose(A0, start, v), compose(A0, end, v)], 0, out);
    return;
  }

  const outwardOk = (port: Port, portCoord: number, v: number): boolean =>
    port.kind === 'face' ? port.att.sign * (v - port.att.plane) > e : Math.abs(v - portCoord) > e;
  const holds: number[] = new Array(k + 1).fill(0);
  const axisOfHold = (i: number): Axis => (i % 2 === 1 ? A0 : A1);
  const candidates = { x: holdCandidates(search, P, Q, 'x'), y: holdCandidates(search, P, Q, 'y') };

  const finish = (): void => {
    const fixedP = P.kind === 'point' ? coord(P.point, A1) : undefined;
    const fixedQ = Q.kind === 'point' ? coord(Q.point, otherAxis(Ak)) : undefined;
    const solveP = (pref: number, neighbor: number): number => {
      if (fixedP !== undefined) return fixedP;
      const port = P as FacePort;
      if (k === 1) {
        return Q.kind === 'face'
          ? solveAlong(port, pref, { kind: 'half', from: stubTip(Q.att, search.minSink), sign: Q.att.sign })
          : solveAlong(port, pref, { kind: 'away', t: end, m: search.minSink });
      }
      return solveAlong(port, pref, { kind: 'away', t: neighbor, m: MIN_SEGMENT });
    };
    const solveQ = (pref: number, neighbor: number): number => {
      if (fixedQ !== undefined) return fixedQ;
      const port = Q as FacePort;
      if (k === 1) {
        return P.kind === 'face'
          ? solveAlong(port, pref, { kind: 'half', from: stubTip(P.att, search.minSource), sign: P.att.sign })
          : solveAlong(port, pref, { kind: 'away', t: start, m: search.minSource });
      }
      return solveAlong(port, pref, { kind: 'away', t: neighbor, m: MIN_SEGMENT });
    };
    const prefP = P.kind === 'face' ? P.pref : fixedP!;
    const prefQ = Q.kind === 'face' ? Q.pref : fixedQ!;
    const variants: Array<[number, number]> = [];
    if (k === 2) {
      // The riser joins the two endpoints' along positions, so they are solved
      // against each other, once in each order.
      const a1 = solveP(prefP, prefQ);
      variants.push([a1, solveQ(prefQ, a1)]);
      const b2 = solveQ(prefQ, prefP);
      variants.push([solveP(prefP, b2), b2]);
    } else {
      variants.push([solveP(prefP, holds[2]), solveQ(prefQ, holds[k - 2])]);
    }
    const seen: Array<[number, number]> = [];
    for (const [a, b] of variants) {
      if (seen.some(([sa, sb]) => Math.abs(sa - a) <= e && Math.abs(sb - b) <= e)) {
        continue;
      }
      seen.push([a, b]);
      holds[0] = a;
      holds[k] = b;
      const points: XY[] = [compose(A0, start, holds[0])];
      for (let i = 1; i <= k; i++) {
        const Ai = i % 2 === 0 ? A0 : A1;
        points.push(compose(Ai, holds[i - 1], holds[i]));
      }
      points.push(compose(Ak, end, holds[k]));
      if (k === 2 && !preserved) {
        const leaving = Math.sign(coord(points[1], A0) - coord(points[0], A0));
        const arriving = Math.sign(coord(points[3], A0) - coord(points[2], A0));
        if ((leaving !== arriving) !== (tier === 'uTurns')) {
          continue;
        }
      }
      emit(search, P, Q, points, k, out);
    }
  };

  const fill = (i: number): void => {
    if (i === k) {
      finish();
      return;
    }
    const axis = axisOfHold(i);
    // A hold a minimum riser away from the parallel hold two segments back (or
    // from a point port's fixed coordinate there) keeps a tail from detouring to
    // a far candidate while a near feasible position exists.
    const neighbors: number[] = [];
    if (i >= 3) {
      neighbors.push(holds[i - 2] - MIN_SEGMENT, holds[i - 2] + MIN_SEGMENT);
    } else if (i === 2 && P.kind === 'point') {
      neighbors.push(coord(P.point, A1) - MIN_SEGMENT, coord(P.point, A1) + MIN_SEGMENT);
    }
    if (i === k - 2 && Q.kind === 'point') {
      neighbors.push(coord(Q.point, otherAxis(Ak)) - MIN_SEGMENT, coord(Q.point, otherAxis(Ak)) + MIN_SEGMENT);
    }
    for (const v of neighbors.length === 0 ? candidates[axis] : [...candidates[axis], ...neighbors]) {
      if (i === 1 && !outwardOk(P, start, v)) continue;
      if (i === k - 1 && !outwardOk(Q, end, v)) continue;
      // An interior segment between holds i-2 and i must have length.
      if (i >= 3 && Math.abs(v - holds[i - 2]) <= e) continue;
      holds[i] = v;
      fill(i + 1);
    }
  };
  if (k === 1) {
    finish();
  } else {
    fill(1);
  }
}

function alongAccepts(port: FacePort, v: number): boolean {
  const e = GEOMETRY_EPSILON;
  if (port.pinned) {
    return Math.abs(v - port.pref) <= e;
  }
  return v >= port.att.lo - e && v <= port.att.hi + e;
}

function emit(search: Search, P: Port, Q: Port, path: XY[], bends: number, out: Candidate[]): void {
  const points = search.assemble(path);
  if (search.refuse(points)) {
    return;
  }
  const sticky = (P.kind === 'face' && P.offBase ? 1 : 0) + (Q.kind === 'face' && Q.offBase ? 1 : 0);
  const n = points.length;
  let axisChange = 0;
  if (search.baseFirstAxis !== undefined && segmentAxisOf(points[0], points[1]) !== search.baseFirstAxis) {
    axisChange++;
  }
  if (search.baseLastAxis !== undefined && segmentAxisOf(points[n - 2], points[n - 1]) !== search.baseLastAxis) {
    axisChange++;
  }
  const quality = pathQuality(points, search.terminals, undefined, search.obstacles);
  out.push({
    points,
    bends,
    sticky,
    axisChange,
    length: pathLength(points),
    fault: quality.fault,
    crossing: quality.crossing,
    obstructed: quality.obstructed,
    index: out.length,
  });
}

function compareCandidates(a: Candidate, b: Candidate): number {
  // Lengths within GEOMETRY_EPSILON tie: Z variants whose riser sits at different
  // holds have the same length up to float noise, and letting that noise rank
  // them flips the riser between frames.
  const byLength = Math.abs(a.length - b.length) > GEOMETRY_EPSILON ? a.length - b.length : 0;
  return a.bends - b.bends || a.axisChange - b.axisChange || byLength || a.index - b.index;
}

/**
 * Pick the route: validity; then, among valid candidates, not crossing a
 * terminal body (G6's best effort when the bodies overlap and crossing is no
 * fault); then stickiness (keep the base faces when they have a candidate
 * within one bend of the best); then bends, axis change and length. Shapes past
 * straight, L and Z are generated only when needed: tails up to `maxBends` when
 * nothing valid exists yet or the stickiness window reaches past what was
 * generated, and, when `detours` is set and nothing is valid -- or every valid
 * candidate crosses something and one of them passes through a non-terminal
 * stock -- U turns and then more bends. A detour around a stock therefore beats
 * a route through it, while a crossing G6 excuses (overlapping terminal bodies)
 * generates no detours. With nothing valid at all, the least severe fault wins,
 * so a route always exists.
 */
function search(s: Search): XY[] {
  const candidates: Candidate[] = [];
  let generated = 2;
  for (let k = 0; k <= generated; k++) {
    generate(s, k, candidates);
  }
  let uTurns = false;
  for (;;) {
    const valid = candidates.filter((c) => c.fault === FAULT_NONE);
    if (valid.length === 0) {
      if (s.detours && !uTurns) {
        uTurns = true;
        generate(s, 2, candidates, 'uTurns');
        continue;
      }
      if (generated < 4 && (generated < s.maxBends || s.detours || candidates.length === 0)) {
        generate(s, ++generated, candidates);
        continue;
      }
      if (candidates.length === 0) {
        return [];
      }
      // The totality rule: G6 relaxed before G3 before structure. Searches without
      // detours (routeEnd's preserved and pinned attempts) reach this branch
      // often, but routeEnd discards a winner that is not valid. With detours it
      // is a backstop no generated scene or table row reaches, so the fault
      // ordering is observable through no test.
      const fallback = [...candidates].sort(
        (a, b) =>
          a.fault - b.fault ||
          Number(a.crossing) - Number(b.crossing) ||
          a.sticky - b.sticky ||
          compareCandidates(a, b),
      );
      return fallback[0].points;
    }
    const clear = valid.filter((c) => !c.crossing);
    if (clear.length === 0 && s.detours && valid.some((c) => c.obstructed)) {
      if (!uTurns) {
        uTurns = true;
        generate(s, 2, candidates, 'uTurns');
        continue;
      }
      if (generated < 4) {
        generate(s, ++generated, candidates);
        continue;
      }
    }
    const pool = clear.length > 0 ? clear : valid;
    const best = Math.min(...pool.map((c) => c.bends));
    const window = pool.filter((c) => c.bends <= best + 1);
    const minSticky = Math.min(...window.map((c) => c.sticky));
    if (minSticky > 0 && best + 1 > generated && generated < s.maxBends) {
      generate(s, ++generated, candidates);
      continue;
    }
    return window.filter((c) => c.sticky === minSticky).sort(compareCandidates)[0].points;
  }
}

function distinctPath(points: readonly XY[]): boolean {
  return points.length >= 2 && points.every(isFiniteXY) && pathLength(points) > GEOMETRY_EPSILON;
}

function endAxes(points: readonly XY[]): { baseFirstAxis: Axis | undefined; baseLastAxis: Axis | undefined } {
  if (!distinctPath(points)) {
    return { baseFirstAxis: undefined, baseLastAxis: undefined };
  }
  const n = points.length;
  return {
    baseFirstAxis: segmentAxisOf(points[0], points[1]),
    baseLastAxis: segmentAxisOf(points[n - 2], points[n - 1]),
  };
}

function baseHoldsOf(base: readonly XY[]): Search['baseHolds'] {
  const interior = distinctPath(base) ? base.slice(1, -1).filter(isFiniteXY) : [];
  return { x: dedupe(interior.map((p) => p.x)), y: dedupe(interior.map((p) => p.y)) };
}

/** Route between two terminals with no preserved prefix; a missing route (no candidates at all) is undefined. */
export function routeBetween(
  terminals: Terminals,
  pinned: { readonly source: boolean; readonly sink: boolean },
  base: readonly XY[],
  occupied: readonly XY[],
  detours = true,
  obstacles: readonly XY[] = [],
): XY[] {
  const partial = {
    sources: portsOf(terminals.source, pinned.source, occupied),
    sinks: portsOf(terminals.sink, pinned.sink, occupied),
    minSource: MIN_SEGMENT,
    minSink: MIN_SINK_SEGMENT,
    terminals,
    obstacles,
    assemble: (path: XY[]) => path,
    refuse: () => false,
    ...endAxes(base),
    maxBends: 2,
    detours,
    baseHolds: baseHoldsOf(base),
  };
  return search({ ...partial, ...buildPools(partial) });
}

export interface RouteContext {
  /**
   * The flow being routed, as it was when the gesture started: its identity is
   * kept, and its path supplies the valve's arc position and the shape
   * stickiness. A creation draft with no length routes fresh.
   */
  readonly flow: FlowViewElement;
  /** The end the valve's arc position is measured from (the fixed end); defaults to the source. */
  readonly valveFrom?: FlowEnd;
  /**
   * Other flows' endpoints on the terminal stocks, for the slot preference, in
   * the coordinates of THIS frame: a planner moving a stock moves the other
   * endpoints on it too before passing them.
   */
  readonly occupied?: readonly XY[];
  /**
   * The view's stocks, in this frame's coordinates. A candidate through one
   * that is not a terminal ranks as crossing, so the route goes around it
   * wherever it can: a pipe through a stock reads as attached to it.
   */
  readonly obstacles?: readonly XY[];
}

/**
 * The minimal orthogonal route between two terminals: a straight, L or Z over
 * every face pair, ranked as `search` describes. When none is valid a U turn,
 * then more bends, are tried (two clouds a pixel off each other's line, or a
 * stock over its own cloud, have no valid straight, L or Z). Total: when nothing
 * is valid (the terminal bodies overlap), G6 is relaxed first and a route is
 * still returned. The valve keeps its arc distance from `ctx.valveFrom`. A
 * non-finite terminal returns the base flow unchanged.
 */
export function route(source: Terminal, sink: Terminal, ctx: RouteContext): FlowGeometry {
  if (!terminalIsFinite(source) || !terminalIsFinite(sink)) {
    return { flow: ctx.flow, clouds: [] };
  }
  const terminals = { source, sink };
  const from = ctx.valveFrom ?? 'source';
  const points = routeBetween(
    terminals,
    { source: false, sink: false },
    ctx.flow.points,
    ctx.occupied ?? [],
    true,
    ctx.obstacles ?? [],
  );
  const valve = placeValve(points, from, valveDistance(ctx.flow.points, ctx.flow, from));
  return withGeometry(ctx.flow, points, valve, terminals);
}

export interface RouteEndContext {
  /** The terminal at the end that does not move. */
  readonly fixed: Terminal;
  /** Other flows' endpoints on the terminal stocks, in this frame's coordinates (see RouteContext). */
  readonly occupied?: readonly XY[];
  /**
   * The view's stocks, in this frame's coordinates (see RouteContext), including
   * one the moving end has just left, which a preserved corner on its old face's
   * line would otherwise run straight through.
   */
  readonly obstacles?: readonly XY[];
}

/**
 * Re-route one end of `flow` (the base flow) to `terminal`, keeping as much of
 * the path near the fixed end as stays valid. Preserve k interior corners
 * counted from the fixed end, for k = K..1 with K all but the corner adjacent
 * to the re-routed end; the first k whose tail yields a valid path that crosses
 * no terminal body and meets every G3 minimum (even where the terminals leave no
 * room and G3 would excuse it) wins. A tail may not fold back over its preserved prefix, nor
 * give the path more bends than the base had: preserving corners keeps a shape,
 * and a tail that grows it is a detour, which releasing to fewer preserved
 * corners replaces. Then k = 0 with the fixed terminal pinned to its base face
 * and offset, and only if that is still invalid or crossing is the flow
 * released to `route`. A path through `ctx.obstacles` ranks as crossing in
 * every attempt: a preserved or pinned tail through one is refused, and the
 * released search prefers a route around it but never refuses its last resort.
 *
 * The valve keeps its arc-length distance from the fixed end. A non-finite
 * terminal returns the base flow unchanged.
 */
export function routeEnd(flow: FlowViewElement, end: FlowEnd, terminal: Terminal, ctx: RouteEndContext): FlowGeometry {
  if (!terminalIsFinite(terminal) || !terminalIsFinite(ctx.fixed)) {
    return { flow, clouds: [] };
  }
  const terminals: Terminals =
    end === 'source' ? { source: terminal, sink: ctx.fixed } : { source: ctx.fixed, sink: terminal };
  const fixedEnd: FlowEnd = end === 'source' ? 'sink' : 'source';
  const occupied = ctx.occupied ?? [];
  const obstacles = ctx.obstacles ?? [];
  const base = distinctPath(flow.points) ? normalize(flow.points) : flow.points;
  const acceptable = (points: readonly XY[]): boolean => {
    const quality = pathQuality(points, terminals, undefined, obstacles);
    return quality.fault === FAULT_NONE && !quality.crossing && !quality.short;
  };
  let points: XY[] | undefined;
  if (distinctPath(base)) {
    for (let k = Math.max(0, base.length - 3); k >= 1 && points === undefined; k--) {
      const tail = preservedTail(base, end, k, terminals, occupied, obstacles);
      points = tail !== undefined && acceptable(tail) ? tail : undefined;
    }
  }
  if (points === undefined) {
    // No detours while pinned: a U turn that keeps the fixed endpoint put is
    // worse than releasing it to slide along its face into a straight route.
    const pinned = routeBetween(
      terminals,
      { source: fixedEnd === 'source', sink: fixedEnd === 'sink' },
      base,
      occupied,
      false,
      obstacles,
    );
    // Of `acceptable`'s clauses, a terminal crossing never fires here: a pinned
    // search generates no detours and at most two bends, so a valid winner is a
    // straight, L or same-direction Z, monotone in both axes. It leaves the fixed
    // face outward and enters the moving terminal's face inward, so it stays
    // outside both bodies. The clause acts on preserved tails ("a tail crossing
    // its stock is refused even where G6 excuses the crossing" in
    // flow-geometry-route.test.ts) and on an obstacle a monotone path can still
    // pass through, which releases the flow to the full search below.
    if (pinned.length > 0 && acceptable(pinned)) {
      points = pinned;
    }
  }
  if (points === undefined) {
    points = routeBetween(terminals, { source: false, sink: false }, base, occupied, true, obstacles);
  }
  const valve = placeValve(points, fixedEnd, valveDistance(flow.points, flow, fixedEnd));
  return withGeometry(flow, points, valve, terminals);
}

function preservedTail(
  base: readonly XY[],
  end: FlowEnd,
  k: number,
  terminals: Terminals,
  occupied: readonly XY[],
  obstacles: readonly XY[],
): XY[] | undefined {
  const n = base.length;
  const moving = end === 'source' ? terminals.source : terminals.sink;
  const movingPorts = portsOf(moving, false, occupied);
  const baseBends = n - 2;
  const baseUTurns = uTurnCount(base);
  const refuse = (points: readonly XY[]): boolean =>
    points.length - 2 > baseBends || uTurnCount(points) > baseUTurns || foldsBack(points, prefixSegments);
  let partial: Omit<Search, 'pools' | 'pointPools'>;
  let prefixSegments: Array<readonly [XY, XY]>;
  if (end === 'sink') {
    const prefix = base.slice(0, k + 1);
    prefixSegments = segmentsOf(prefix);
    const corner = prefix[k];
    const port: PointPort = { kind: 'point', point: corner, axis: otherAxis(segmentAxisOf(prefix[k - 1], corner)) };
    partial = {
      sources: [port],
      sinks: movingPorts,
      minSource: MIN_SEGMENT,
      minSink: MIN_SINK_SEGMENT,
      terminals,
      obstacles,
      assemble: (path) => [...prefix.slice(0, k), ...path],
      refuse,
      ...endAxes(base),
      maxBends: 3,
      detours: false,
      baseHolds: baseHoldsOf(base),
    };
  } else {
    const prefix = base.slice(n - 1 - k);
    prefixSegments = segmentsOf(prefix);
    const corner = prefix[0];
    const port: PointPort = { kind: 'point', point: corner, axis: otherAxis(segmentAxisOf(corner, prefix[1])) };
    partial = {
      sources: movingPorts,
      sinks: [port],
      minSource: MIN_SEGMENT,
      minSink: MIN_SEGMENT,
      terminals,
      obstacles,
      assemble: (path) => [...path, ...prefix.slice(1)],
      refuse,
      ...endAxes(base),
      maxBends: 3,
      detours: false,
      baseHolds: baseHoldsOf(base),
    };
  }
  const points = search({ ...partial, ...buildPools(partial) });
  return points.length > 0 && pathQuality(points, terminals, undefined, obstacles).fault === FAULT_NONE
    ? points
    : undefined;
}

/**
 * The number of U turns in a path: two consecutive turns in the same rotational
 * direction (a Z turns one way and back; a U turns the same way twice and heads
 * back past where it came from).
 */
function uTurnCount(points: readonly XY[]): number {
  let count = 0;
  let previous = 0;
  for (let i = 1; i < points.length - 1; i++) {
    const ax = points[i].x - points[i - 1].x;
    const ay = points[i].y - points[i - 1].y;
    const bx = points[i + 1].x - points[i].x;
    const by = points[i + 1].y - points[i].y;
    const turn = Math.sign(ax * by - ay * bx);
    if (turn !== 0 && turn === previous) {
      count++;
    }
    previous = turn;
  }
  return count;
}

function segmentsOf(points: readonly XY[]): Array<readonly [XY, XY]> {
  return points.slice(1).map((p, i) => [points[i], p] as const);
}

/**
 * Whether some segment of `points` runs back over a preserved segment: the two
 * are parallel, travel in opposite directions, hold coordinates less than
 * MIN_SEGMENT apart, and overlap in span. The pipe would draw over itself.
 */
function foldsBack(points: readonly XY[], preserved: ReadonlyArray<readonly [XY, XY]>): boolean {
  const e = GEOMETRY_EPSILON;
  const segments = segmentsOf(points);
  for (const [a, b] of segments) {
    const axis = segmentAxisOf(a, b);
    const hold = coord(a, otherAxis(axis));
    const direction = Math.sign(coord(b, axis) - coord(a, axis));
    for (const [c, d] of preserved) {
      if (segmentAxisOf(c, d) !== axis || Math.sign(coord(d, axis) - coord(c, axis)) !== -direction) {
        continue;
      }
      if (Math.abs(coord(c, otherAxis(axis)) - hold) >= MIN_SEGMENT - e) {
        continue;
      }
      const lo = Math.max(Math.min(coord(a, axis), coord(b, axis)), Math.min(coord(c, axis), coord(d, axis)));
      const hi = Math.min(Math.max(coord(a, axis), coord(b, axis)), Math.max(coord(c, axis), coord(d, axis)));
      if (hi - lo > e) {
        return true;
      }
    }
  }
  return false;
}
