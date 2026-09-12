// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Classifying a path against its terminals (G2-G6).
 */

import type { FlowViewElement } from '@simlin/core/datamodel';

import {
  type Axis,
  boxesOverlap,
  coord,
  distance,
  GEOMETRY_EPSILON,
  inflate,
  isFiniteXY,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  segmentThroughBox,
  stockBody,
  strictlyInside,
  type XY,
} from './geometry';
import { faceAttachment, faceOfEndpoint, terminalBody, type Terminals } from './terminal';

/**
 * How a path violates G2-G6, worst last. When nothing is valid the fallback
 * keeps the least severe fault, so G6 (crossing) is given up before G3 minima
 * (short), and those before G2/G4/G5 (structure).
 */
export type RouteFault = 'none' | 'crossing' | 'short' | 'structure';

const FAULT_NAMES: readonly RouteFault[] = ['none', 'crossing', 'short', 'structure'];
export const FAULT_NONE = 0;
const FAULT_CROSSING = 1;
const FAULT_SHORT = 2;
const FAULT_STRUCTURE = 3;

/**
 * G3's "whenever the terminals leave room": the source body inflated by
 * MIN_SEGMENT and the sink body inflated by MIN_SINK_SEGMENT are disjoint.
 */
export function terminalsLeaveRoom(terminals: Terminals): boolean {
  return !boxesOverlap(
    inflate(terminalBody(terminals.source), MIN_SEGMENT),
    inflate(terminalBody(terminals.sink), MIN_SINK_SEGMENT),
  );
}

/** G6's precondition: the two terminal bodies, each inflated by MIN_SEGMENT, do not overlap. */
export function bodiesApart(terminals: Terminals): boolean {
  return !boxesOverlap(
    inflate(terminalBody(terminals.source), MIN_SEGMENT),
    inflate(terminalBody(terminals.sink), MIN_SEGMENT),
  );
}

export interface PathQuality {
  /** A FAULT_* rank: 0 when the path holds G2-G6. */
  readonly fault: number;
  /**
   * Whether the path crosses a terminal body or puts a free endpoint inside a
   * stock, computed whether or not G6's precondition holds. With the bodies
   * apart a crossing is a fault; with them overlapping it is G6's best effort,
   * a preference the ranking still honors rather than no constraint at all.
   */
  readonly crossing: boolean;
  /**
   * Whether some segment is under its G3 minimum, computed whether or not the
   * terminals leave room. With room it is a fault; without, the minima are best
   * effort ("the longest achievable") and a candidate that meets them anyway is
   * preferred, so a drag across the room boundary does not flip between a route
   * that respects the minima and one that merely stopped being required to.
   */
  readonly short: boolean;
}

/**
 * Classify a path against its terminals. `stocks` are the view's stocks a free
 * endpoint must not sit inside (G6's cloud clause); the terminal stocks are
 * always checked. `obstacles` are stocks no segment may pass through even though
 * they are not terminals: the stocks the flow was attached to when the gesture
 * started, which an end may just have left. A path through one ranks and faults
 * exactly like a path through a terminal body.
 */
export function pathQuality(
  points: readonly XY[],
  terminals: Terminals,
  stocks?: readonly XY[],
  obstacles?: readonly XY[],
): PathQuality {
  const n = points.length;
  const structure = { fault: FAULT_STRUCTURE, crossing: false, short: false };
  if (n < 2 || !points.every(isFiniteXY)) {
    return structure;
  }
  const e = GEOMETRY_EPSILON;
  let previousAxis: Axis | undefined;
  for (let i = 0; i < n - 1; i++) {
    const a = points[i];
    const b = points[i + 1];
    const flatX = Math.abs(a.x - b.x) <= e;
    const flatY = Math.abs(a.y - b.y) <= e;
    if (flatX === flatY) {
      return structure;
    }
    const axis: Axis = flatY ? 'x' : 'y';
    if (axis === previousAxis) {
      return structure;
    }
    previousAxis = axis;
  }
  for (const [t, index, adjacent] of [
    [terminals.source, 0, 1],
    [terminals.sink, n - 1, n - 2],
  ] as const) {
    if (t.kind !== 'stock') {
      continue;
    }
    const p = points[index];
    // With the adjacent point, faceOfEndpoint picks the face the (orthogonal)
    // adjacent segment is perpendicular to whenever one exists, so only the
    // outward direction remains to check.
    const face = faceOfEndpoint(t.stock, p, points[adjacent]);
    if (face === undefined) {
      return structure;
    }
    const att = faceAttachment(t.stock, face);
    const along = coord(p, att.along);
    const q = points[adjacent];
    if (along < att.lo - e || along > att.hi + e || att.sign * (coord(q, att.normal) - att.plane) <= e) {
      return structure;
    }
  }
  const crossing = crossesBodies(points, terminals, stocks, obstacles);
  let short = false;
  for (let i = 0; i < n - 1 && !short; i++) {
    const minimum = i === n - 2 ? MIN_SINK_SEGMENT : MIN_SEGMENT;
    short = distance(points[i], points[i + 1]) < minimum - e;
  }
  if (short && terminalsLeaveRoom(terminals)) {
    return { fault: FAULT_SHORT, crossing, short };
  }
  return { fault: crossing && bodiesApart(terminals) ? FAULT_CROSSING : FAULT_NONE, crossing, short };
}

function crossesBodies(
  points: readonly XY[],
  terminals: Terminals,
  stocks: readonly XY[] | undefined,
  obstacles: readonly XY[] | undefined,
): boolean {
  const n = points.length;
  const bodies: readonly XY[] = [
    ...[terminals.source, terminals.sink].flatMap((t): XY[] => (t.kind === 'stock' ? [t.stock] : [])),
    ...(obstacles ?? []),
  ];
  for (const center of bodies) {
    const body = stockBody(center);
    for (let i = 0; i < n - 1; i++) {
      if (segmentThroughBox(points[i], points[i + 1], body)) {
        return true;
      }
    }
  }
  for (const [t, index] of [
    [terminals.source, 0],
    [terminals.sink, n - 1],
  ] as const) {
    if (t.kind !== 'free') {
      continue;
    }
    const p = points[index];
    const inTerminal = [terminals.source, terminals.sink].some(
      (other) => other.kind === 'stock' && strictlyInside(p, stockBody(other.stock)),
    );
    if (inTerminal || stocks?.some((s) => strictlyInside(p, stockBody(s)))) {
      return true;
    }
  }
  return false;
}

/**
 * Classify a flow's path against its terminals (G2-G6). The planner uses this to
 * decide whether committing onto a target yields a view that holds the
 * invariants; `stocks` extends G6's cloud clause to the rest of the view, and
 * `obstacles` its segment clause to the stocks the flow was attached to when the
 * gesture started (see pathQuality).
 */
export function flowFault(
  flow: FlowViewElement,
  terminals: Terminals,
  stocks?: readonly XY[],
  obstacles?: readonly XY[],
): RouteFault {
  return FAULT_NAMES[pathQuality(flow.points, terminals, stocks, obstacles).fault];
}
