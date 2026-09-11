// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Paths and the valve: normalize, arc length, the valve policy (an arc-length
 * position measured from an end, margin applied once), translate and
 * slideValve.
 */

import type { FlowViewElement, Point } from '@simlin/core/datamodel';

import { clamp, distance, type FlowEnd, GEOMETRY_EPSILON, isFiniteXY, VALVE_CLAMP_MARGIN, type XY } from './geometry';

/**
 * Remove zero-length segments and collinear interior points (G3's structural
 * arms). Endpoints are always kept: they carry the attachments. Removing one
 * point can make its neighbors collinear, so this repeats to a fixed point.
 * Inputs are orthogonal (heal snaps diagonals first), where a zero-length
 * segment is always also collinear with its neighbor, so one test covers both.
 */
export function normalize(points: readonly Point[]): Point[] {
  let current = [...points];
  for (;;) {
    const next = normalizeOnce(current);
    if (next.length === current.length) {
      return next;
    }
    current = next;
  }
}

function normalizeOnce(points: readonly Point[]): Point[] {
  if (points.length <= 2) {
    return [...points];
  }
  const e = GEOMETRY_EPSILON;
  const result: Point[] = [points[0]];
  for (let i = 1; i < points.length - 1; i++) {
    const prev = result[result.length - 1];
    const curr = points[i];
    const next = points[i + 1];
    const horizontal = Math.abs(prev.y - curr.y) <= e && Math.abs(curr.y - next.y) <= e;
    const vertical = Math.abs(prev.x - curr.x) <= e && Math.abs(curr.x - next.x) <= e;
    if (horizontal || vertical) {
      continue;
    }
    result.push(curr);
  }
  result.push(points[points.length - 1]);
  return result;
}

export function pathLength(points: readonly XY[]): number {
  let total = 0;
  for (let i = 0; i < points.length - 1; i++) {
    total += distance(points[i], points[i + 1]);
  }
  return total;
}

/** The arc-length position of the point on the path nearest to `p` (the earliest one on a tie). */
export function arcPosition(points: readonly XY[], p: XY): number {
  let best = Infinity;
  let position = 0;
  let traversed = 0;
  for (let i = 0; i < points.length - 1; i++) {
    const a = points[i];
    const b = points[i + 1];
    const length = distance(a, b);
    const t =
      length === 0 ? 0 : clamp(((p.x - a.x) * (b.x - a.x) + (p.y - a.y) * (b.y - a.y)) / (length * length), 0, 1);
    const d = Math.hypot(p.x - (a.x + t * (b.x - a.x)), p.y - (a.y + t * (b.y - a.y)));
    if (d < best - GEOMETRY_EPSILON) {
      best = d;
      position = traversed + t * length;
    }
    traversed += length;
  }
  return position;
}

/** The distance from `p` to the nearest point on the path. */
export function distanceToPath(points: readonly XY[], p: XY): number {
  let best = Infinity;
  for (let i = 0; i < points.length - 1; i++) {
    const a = points[i];
    const b = points[i + 1];
    const length = distance(a, b);
    const t =
      length === 0 ? 0 : clamp(((p.x - a.x) * (b.x - a.x) + (p.y - a.y) * (b.y - a.y)) / (length * length), 0, 1);
    best = Math.min(best, Math.hypot(p.x - (a.x + t * (b.x - a.x)), p.y - (a.y + t * (b.y - a.y))));
  }
  return best;
}

/** The point at arc length `s` along the path, clamped to the path. */
export function pointAtArc(points: readonly XY[], s: number): XY {
  if (points.length === 1 || s <= 0) {
    return { x: points[0].x, y: points[0].y };
  }
  let remaining = s;
  for (let i = 0; i < points.length - 1; i++) {
    const a = points[i];
    const b = points[i + 1];
    const length = distance(a, b);
    if (remaining <= length) {
      const t = length === 0 ? 0 : remaining / length;
      return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
    }
    remaining -= length;
  }
  const last = points[points.length - 1];
  return { x: last.x, y: last.y };
}

/**
 * The valve's arc-length distance from `from` on the base path, or undefined
 * when the base path has no length (a creation draft), which places the valve
 * at the midpoint of whatever path is routed. The margin is NOT applied here.
 */
export function valveDistance(points: readonly XY[], valve: XY, from: FlowEnd): number | undefined {
  const length = pathLength(points);
  if (length <= GEOMETRY_EPSILON || !isFiniteXY(valve)) {
    return undefined;
  }
  const s = arcPosition(points, valve);
  return from === 'source' ? s : length - s;
}

/**
 * Place the valve `distance` along the path from `from`, clamped to the path,
 * then apply VALVE_CLAMP_MARGIN. The margin is applied once, here, at the end:
 * applying it while measuring would move a valve that sat inside the margin on
 * the base path further inward whenever the path grows at the far end.
 */
export function placeValve(points: readonly XY[], from: FlowEnd, distanceFromEnd: number | undefined): XY {
  const length = pathLength(points);
  let s: number;
  if (distanceFromEnd === undefined) {
    s = length / 2;
  } else {
    s = clamp(from === 'source' ? distanceFromEnd : length - distanceFromEnd, 0, length);
  }
  return pointAtArc(points, applyValveMargin(length, s));
}

/**
 * G8's margin. At exactly two margins long the only valid position is the
 * midpoint, which the clamp also produces, so the `<` boundary is a choice
 * without consequence.
 */
export function applyValveMargin(length: number, s: number): number {
  if (length < 2 * VALVE_CLAMP_MARGIN) {
    return length / 2;
  }
  return clamp(s, VALVE_CLAMP_MARGIN, length - VALVE_CLAMP_MARGIN);
}

/**
 * Move a flow whose two terminals both move by `delta`: every point and the
 * valve translate. The caller moves the terminal elements (selected clouds and
 * stocks translate as positioned elements), so there are no clouds to report.
 * A non-finite delta returns the flow unchanged.
 */
export function translate(flow: FlowViewElement, delta: XY): FlowViewElement {
  if (!isFiniteXY(delta)) {
    return flow;
  }
  return {
    ...flow,
    x: flow.x + delta.x,
    y: flow.y + delta.y,
    points: flow.points.map((p) => ({ ...p, x: p.x + delta.x, y: p.y + delta.y })),
  };
}

/**
 * Slide the valve along the path by the pointer delta projected onto the path.
 *
 * The delta is applied as if the pointer traveled straight from the press: on
 * each segment the valve moves at the rate the delta projects onto that
 * segment's direction, and when it reaches a corner it continues onto the next
 * segment with the time that is left, if the delta projects forward along it
 * (otherwise it rests at the corner). Carrying the remaining TIME rather than
 * the remaining vector is what keeps this continuous: a delta component
 * perpendicular to the valve's segment is never banked and released all at
 * once when the corner is reached. The valve crosses corners instead of hopping
 * to whichever segment is nearest; it lags the pointer at a corner by design,
 * and `delta` is measured from the press, so the grab offset is kept. The path
 * changes nothing a cloud sits on, so there are no clouds to report.
 */
export function slideValve(flow: FlowViewElement, delta: XY): FlowViewElement {
  const pts = flow.points;
  const segments: Array<{ start: number; length: number; tx: number; ty: number }> = [];
  let traversed = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    const length = distance(pts[i], pts[i + 1]);
    if (length > GEOMETRY_EPSILON) {
      segments.push({
        start: traversed,
        length,
        tx: (pts[i + 1].x - pts[i].x) / length,
        ty: (pts[i + 1].y - pts[i].y) / length,
      });
    }
    traversed += length;
  }
  if (segments.length === 0 || !isFiniteXY(delta) || !pts.every(isFiniteXY)) {
    return flow;
  }
  const total = traversed;
  let pos = isFiniteXY(flow) ? arcPosition(pts, flow) : total / 2;
  const rate = (j: number): number => delta.x * segments[j].tx + delta.y * segments[j].ty;
  let j = segmentAtArc(segments, pos);
  // A valve exactly on a corner belongs to whichever adjacent segment the delta moves it along.
  if (
    Math.abs(rate(j)) <= GEOMETRY_EPSILON &&
    j + 1 < segments.length &&
    pos >= segments[j + 1].start - GEOMETRY_EPSILON
  ) {
    j++;
  }
  let time = 1;
  while (time > 0) {
    const seg = segments[j];
    const v = rate(j);
    // Only a neighbor the delta still moves the valve along is entered, in either
    // direction. The forward check is redundant with the other branches: a
    // neighbor pointing back is entered with no time spent and stops the valve at
    // the corner, since the backward branch's own guard sees the segment it came
    // from moving forward; a perpendicular neighbor stops it in the final branch.
    if (v > GEOMETRY_EPSILON) {
      const need = (seg.start + seg.length - pos) / v;
      if (need >= time || j + 1 >= segments.length || rate(j + 1) <= GEOMETRY_EPSILON) {
        pos = Math.min(pos + v * time, seg.start + seg.length);
        break;
      }
      pos = seg.start + seg.length;
      time -= need;
      j++;
    } else if (v < -GEOMETRY_EPSILON) {
      const need = (pos - seg.start) / -v;
      if (need >= time || j === 0 || rate(j - 1) >= -GEOMETRY_EPSILON) {
        pos = Math.max(pos + v * time, seg.start);
        break;
      }
      pos = seg.start;
      time -= need;
      j--;
    } else {
      break;
    }
  }
  const valve = pointAtArc(pts, applyValveMargin(total, pos));
  return { ...flow, x: valve.x, y: valve.y };
}

function segmentAtArc(segments: ReadonlyArray<{ start: number; length: number }>, s: number): number {
  for (let i = 0; i < segments.length; i++) {
    if (s <= segments[i].start + segments[i].length) {
      return i;
    }
  }
  return segments.length - 1;
}
