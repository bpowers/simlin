// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * offsetSegment: moving one segment of a flow perpendicular to itself.
 */

import type { FlowViewElement, Point } from '@simlin/core/datamodel';

import {
  type Axis,
  clamp,
  compose,
  coord,
  distance,
  type FlowEnd,
  GEOMETRY_EPSILON,
  isFiniteXY,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  otherAxis,
  samePoint,
  segmentAxisOf,
  type XY,
} from './geometry';
import { applyValveMargin, arcPosition, normalize, pathLength, placeValve, pointAtArc } from './path';
import {
  attachPoints,
  type FaceAttachment,
  faceAttachment,
  faceOfEndpoint,
  type FlowGeometry,
  stubTip,
  terminalIsFinite,
  type Terminals,
  withGeometry,
} from './terminal';
import { FAULT_NONE, pathQuality } from './validity';

export interface OffsetContext {
  /** The view's stocks: a cloud moved with its segment must not land inside one (G6). */
  readonly stocks?: readonly XY[];
}

/** The axis segment `segmentIndex` runs along, and the coordinate it holds constant (the one offsetSegment sets). */
export function segmentHold(
  points: readonly XY[],
  segmentIndex: number,
): { readonly axis: Axis; readonly hold: number } {
  const a = points[segmentIndex];
  const b = points[segmentIndex + 1];
  const axis = segmentAxisOf(a, b);
  return { axis, hold: coord(a, otherAxis(axis)) };
}

// How far past the request offsetSegment looks for the far side of an obstacle
// (a stock the segment or its cloud would enter): more than any stock's extent.
const OBSTACLE_SCAN = 120;
const OBSTACLE_STEP = 2;

/**
 * Move segment `segmentIndex` perpendicular to itself so it holds `coordinate`.
 *
 * The coordinate is first resolved against the adjacent segments: a stock stub
 * keeps at least MIN_SEGMENT (MIN_SINK_SEGMENT at the sink), and an adjacent
 * riser or cloud segment is either at least its minimum or collapsed to zero
 * (removing that corner). These constraints are solved jointly, as the feasible
 * coordinate nearest the request, so a short riser collapses when the request is
 * within half a minimum of collapsing and is pushed out to the minimum otherwise.
 * When the resolved coordinate still puts the path through a terminal body or a
 * cloud inside a stock, the nearest valid coordinate on either side of that
 * obstacle is taken: the segment follows the pointer up to the obstacle and
 * jumps across it once the far side is nearer (a documented feasibility
 * transition).
 *
 * At a terminal the tail is re-solved: a cloud moves with the segment; a stock
 * endpoint sits at the coordinate clamped to the face extent (minus
 * CORNER_CLEARANCE). While the coordinate is within MIN_SEGMENT beyond the
 * extent the segment stays at the extent; beyond that a stub plus a riser join
 * the endpoint to the segment. A tail that is already endpoint -> stub (at most
 * the end's minimum long) -> riser, with the dragged segment following the
 * riser, is re-solved the same way, so a bracket dragged back collapses to
 * straight and stubs never accumulate.
 *
 * The valve stays where it is along its own segment while that segment
 * survives (the dragged segment moves with the valve on it), clamped into the
 * segment's new span; when its segment is removed (a collapsed riser) it moves to
 * the nearest point of the new path, so it jumps no further than the path did. A
 * non-finite coordinate or terminal returns the base flow unchanged.
 */
export function offsetSegment(
  flow: FlowViewElement,
  segmentIndex: number,
  coordinate: number,
  terminals: Terminals,
  ctx: OffsetContext = {},
): FlowGeometry {
  const pts = flow.points;
  if (
    segmentIndex < 0 ||
    segmentIndex >= pts.length - 1 ||
    !Number.isFinite(coordinate) ||
    !terminalIsFinite(terminals.source) ||
    !terminalIsFinite(terminals.sink) ||
    pts.length < 2 ||
    !pts.every(isFiniteXY) ||
    samePoint(pts[segmentIndex], pts[segmentIndex + 1])
  ) {
    return { flow, clouds: [] };
  }
  const tails = {
    source: resolvableTail(pts, segmentIndex, 'source', terminals),
    sink: resolvableTail(pts, segmentIndex, 'sink', terminals),
  };
  const constraints = adjacentConstraints(pts, segmentIndex, terminals, tails);
  const resolve = (c: number): number => resolveCoordinate(c, constraints);
  // Validity is judged on the normalized path: a riser collapsed to zero length
  // is a corner removed, not a zero-length segment.
  const build = (c: number): Point[] => normalize(attachPoints(buildOffset(pts, segmentIndex, c, tails), terminals));
  const valid = (c: number): boolean => pathQuality(build(c), terminals, ctx.stocks).fault === FAULT_NONE;
  let c = resolve(coordinate);
  if (!valid(c) && pathQuality(pts, terminals, ctx.stocks).fault === FAULT_NONE) {
    c = nearestValid(segmentHold(pts, segmentIndex).hold, c, resolve, valid);
  }
  const points = build(c);
  const valve = offsetValve(flow, segmentIndex, points, effectiveHold(c, tails));
  return withGeometry(flow, points, valve, terminals);
}

/**
 * The valid coordinate nearest `request`, looking back toward the (valid) base
 * and past the obstacle, each side found by bisection on the resolved
 * coordinate.
 */
function nearestValid(
  baseHold: number,
  request: number,
  resolve: (c: number) => number,
  valid: (c: number) => boolean,
): number {
  const bisect = (good: number, bad: number): number => {
    for (let iteration = 0; iteration < 32; iteration++) {
      const mid = (good + bad) / 2;
      if (valid(resolve(mid))) {
        good = mid;
      } else {
        bad = mid;
      }
    }
    return resolve(good);
  };
  const back = bisect(baseHold, request);
  const sign = Math.sign(request - baseHold) || 1;
  for (let step = OBSTACLE_STEP; step <= OBSTACLE_SCAN; step += OBSTACLE_STEP) {
    const probe = request + sign * step;
    if (valid(resolve(probe))) {
      const across = bisect(probe, request);
      return Math.abs(across - request) < Math.abs(back - request) ? across : back;
    }
  }
  return back;
}

/**
 * The face whose tail offsetSegment re-solves at `end`, or undefined when the
 * tail is kept. A stock tail is re-solved when the dragged segment is the one
 * leaving the face, or when it follows a stub (at most the end's minimum long)
 * and a riser.
 */
function resolvableTail(pts: readonly XY[], i: number, end: FlowEnd, terminals: Terminals): FaceAttachment | undefined {
  const t = end === 'source' ? terminals.source : terminals.sink;
  if (t.kind !== 'stock') {
    return undefined;
  }
  const n = pts.length;
  const endpoint = end === 'source' ? pts[0] : pts[n - 1];
  const adjacent = end === 'source' ? pts[1] : pts[n - 2];
  const face = faceOfEndpoint(t.stock, endpoint, adjacent);
  if (face === undefined) {
    return undefined;
  }
  const att = faceAttachment(t.stock, face);
  if (segmentAxisOf(pts[i], pts[i + 1]) !== att.normal) {
    return undefined;
  }
  const fromEnd = end === 'source' ? i : n - 2 - i;
  if (fromEnd === 0) {
    return att;
  }
  const minimum = end === 'source' ? MIN_SEGMENT : MIN_SINK_SEGMENT;
  if (fromEnd === 2 && distance(endpoint, adjacent) <= minimum + GEOMETRY_EPSILON) {
    return att;
  }
  return undefined;
}

/**
 * The hold a re-solved stock tail gives the dragged segment: the coordinate
 * itself within the face extent, the extent while within MIN_SEGMENT beyond it,
 * and the coordinate again past that (where a stub and riser appear).
 */
function tailHold(att: FaceAttachment, c: number): number {
  if (c > att.hi && c <= att.hi + MIN_SEGMENT) {
    return att.hi;
  }
  if (c < att.lo && c >= att.lo - MIN_SEGMENT) {
    return att.lo;
  }
  return c;
}

interface Tails {
  readonly source: FaceAttachment | undefined;
  readonly sink: FaceAttachment | undefined;
}

/** The hold the dragged segment actually takes for coordinate `c`, after each re-solved tail's extent band. */
function effectiveHold(c: number, tails: Tails): number {
  let hold = c;
  if (tails.source !== undefined) {
    hold = tailHold(tails.source, hold);
  }
  if (tails.sink !== undefined) {
    hold = tailHold(tails.sink, hold);
  }
  return hold;
}

interface Constraints {
  /** The coordinate must be at least `from` in direction `sign` (a stock stub's minimum). */
  readonly halves: ReadonlyArray<{ readonly from: number; readonly sign: 1 | -1 }>;
  /** The coordinate must be at `t` (collapsed) or at least `m` from it (a riser or cloud segment). */
  readonly aways: ReadonlyArray<{ readonly t: number; readonly m: number }>;
}

function adjacentConstraints(pts: readonly XY[], i: number, terminals: Terminals, tails: Tails): Constraints {
  const n = pts.length;
  const last = n - 2;
  const { axis } = segmentHold(pts, i);
  const H = otherAxis(axis);
  const halves: Array<{ from: number; sign: 1 | -1 }> = [];
  const aways: Array<{ t: number; m: number }> = [];
  const adjacent = (neighbor: number, fixedIndex: number, end: FlowEnd | undefined): void => {
    const t = end === 'source' ? terminals.source : end === 'sink' ? terminals.sink : undefined;
    const minimum = end === 'sink' ? MIN_SINK_SEGMENT : MIN_SEGMENT;
    if (t?.kind === 'stock') {
      const face = faceOfEndpoint(t.stock, pts[fixedIndex], pts[neighbor]);
      if (face !== undefined) {
        const att = faceAttachment(t.stock, face);
        halves.push({ from: stubTip(att, minimum), sign: att.sign });
        return;
      }
    }
    aways.push({ t: coord(pts[fixedIndex], H), m: minimum });
  };
  if (tails.source === undefined && i >= 1) {
    adjacent(i, i - 1, i - 1 === 0 ? 'source' : undefined);
  }
  if (tails.sink === undefined && i <= last - 1) {
    adjacent(i + 1, i + 2, i + 1 === last ? 'sink' : undefined);
  }
  return { halves, aways };
}

/**
 * The coordinate nearest `c` satisfying every constraint. The candidates are
 * `c` clamped by the half-lines and each away constraint's collapse point and
 * its two minimum positions; when none satisfies everything (the constraints
 * contradict), the clamped request is returned and validity rejects the path.
 */
function resolveCoordinate(c: number, constraints: Constraints): number {
  const e = GEOMETRY_EPSILON;
  const clampHalves = (v: number): number => {
    for (const h of constraints.halves) {
      v = h.sign > 0 ? Math.max(v, h.from) : Math.min(v, h.from);
    }
    return v;
  };
  const satisfies = (v: number): boolean =>
    constraints.halves.every((h) => h.sign * (v - h.from) >= -e) &&
    constraints.aways.every((a) => Math.abs(v - a.t) <= e || Math.abs(v - a.t) >= a.m - e);
  const clamped = clampHalves(c);
  if (satisfies(clamped)) {
    return clamped;
  }
  const candidates = constraints.aways.flatMap((a) => [a.t, a.t - a.m, a.t + a.m]).filter(satisfies);
  if (candidates.length === 0) {
    return clamped;
  }
  return candidates.reduce((best, v) => (Math.abs(v - c) < Math.abs(best - c) ? v : best));
}

/**
 * The path with segment `i` at hold `c`. A free terminal's endpoint on the
 * dragged segment moves with it (its cloud follows the endpoint); a re-solved
 * stock tail is rebuilt from the face attachment.
 */
function buildOffset(pts: readonly XY[], i: number, c: number, tails: Tails): XY[] {
  const n = pts.length;
  const last = n - 2;
  const { axis } = segmentHold(pts, i);
  const hold = effectiveHold(c, tails);
  const head: XY[] = [];
  let u: XY;
  if (tails.source !== undefined) {
    const att = tails.source;
    const along = clamp(hold, att.lo, att.hi);
    const endpoint = compose(att.along, along, att.plane);
    if (Math.abs(along - hold) <= GEOMETRY_EPSILON) {
      u = endpoint;
    } else {
      const tip = stubTip(att, MIN_SEGMENT);
      head.push(endpoint, compose(att.normal, tip, along));
      u = compose(att.normal, tip, hold);
    }
  } else if (i === 0) {
    u = compose(axis, pts[0][axis], hold);
  } else {
    head.push(...pts.slice(0, i));
    u = compose(axis, coord(pts[i], axis), hold);
  }
  const tail: XY[] = [];
  let v: XY;
  if (tails.sink !== undefined) {
    const att = tails.sink;
    const along = clamp(hold, att.lo, att.hi);
    const endpoint = compose(att.along, along, att.plane);
    if (Math.abs(along - hold) <= GEOMETRY_EPSILON) {
      v = endpoint;
    } else {
      const tip = stubTip(att, MIN_SINK_SEGMENT);
      v = compose(att.normal, tip, hold);
      tail.push(compose(att.normal, tip, along), endpoint);
    }
  } else if (i === last) {
    v = compose(axis, pts[n - 1][axis], hold);
  } else {
    v = compose(axis, coord(pts[i + 1], axis), hold);
    tail.push(...pts.slice(i + 2));
  }
  return [...head, u, v, ...tail];
}

/**
 * The valve after an offset. Its base segment j survives when the new path has
 * a segment on the same axis holding the same coordinate (for j = i, the new
 * hold `c`) whose span overlaps j's: the valve keeps its coordinate along that
 * axis, clamped into the surviving segment's span. Otherwise it moves to the
 * nearest point of the new path.
 */
function offsetValve(base: FlowViewElement, i: number, points: readonly XY[], c: number): XY {
  const pts = base.points;
  if (!isFiniteXY(base)) {
    return placeValve(points, 'source', undefined);
  }
  const s0 = arcPosition(pts, base);
  let start = 0;
  let j = 0;
  for (; j < pts.length - 2; j++) {
    const length = distance(pts[j], pts[j + 1]);
    if (s0 <= start + length) {
      break;
    }
    start += length;
  }
  const { axis, hold } = segmentHold(pts, j);
  const newHold = j === i ? c : hold;
  const along = coord(base, axis);
  const spanLo = Math.min(coord(pts[j], axis), coord(pts[j + 1], axis));
  const spanHi = Math.max(coord(pts[j], axis), coord(pts[j + 1], axis));
  let best: XY | undefined;
  let bestGap = Infinity;
  for (let k = 0; k < points.length - 1; k++) {
    const a = points[k];
    const b = points[k + 1];
    if (
      samePoint(a, b) ||
      segmentAxisOf(a, b) !== axis ||
      Math.abs(coord(a, otherAxis(axis)) - newHold) > GEOMETRY_EPSILON
    ) {
      continue;
    }
    const lo = Math.min(coord(a, axis), coord(b, axis));
    const hi = Math.max(coord(a, axis), coord(b, axis));
    if (Math.min(hi, spanHi) - Math.max(lo, spanLo) < -GEOMETRY_EPSILON) {
      continue;
    }
    const at = clamp(along, lo, hi);
    const gap = Math.abs(at - along);
    if (gap < bestGap) {
      bestGap = gap;
      best = compose(axis, at, newHold);
    }
  }
  const at = best ?? { x: base.x, y: base.y };
  return pointAtArc(points, applyValveMargin(pathLength(points), arcPosition(points, at)));
}
