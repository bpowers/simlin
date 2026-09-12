// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * heal: repairing a flow an edit is about to route (imported or legacy data).
 */

import type { FlowViewElement } from '@simlin/core/datamodel';

import {
  compose,
  coord,
  isFiniteXY,
  otherAxis,
  segmentAxisOf,
  stockBody,
  strictlyInside,
  VALVE_CLAMP_MARGIN,
  GEOMETRY_EPSILON,
  type XY,
} from './geometry';
import { applyValveMargin, arcPosition, distanceToPath, normalize, pathLength, placeValve, pointAtArc } from './path';
import { routeBetween, route } from './route';
import {
  attachPoints,
  cloudUpdates,
  faceAttachment,
  faceOfEndpoint,
  facePoint,
  type FlowGeometry,
  freeTerminal,
  nearestFaceAttachment,
  terminalIsFinite,
  type Terminals,
  terminalUid,
  withGeometry,
} from './terminal';
import { FAULT_NONE, pathQuality } from './validity';

export interface HealContext {
  /** The view's stocks: a healed cloud must not sit inside one (G6's cloud clause). */
  readonly stocks?: readonly XY[];
}

/**
 * Repair a flow an edit is about to route. Identity on a valid flow, and
 * idempotent. In order: attach the endpoints to their terminals (a stock
 * endpoint off its face, or inside the corner clearance, is re-pinned to the
 * nearest valid point, along the face its adjacent segment is perpendicular to
 * when there is one; a cloud is moved onto its endpoint rather than the pipe
 * onto the cloud, and out of any stock it would sit inside), snap
 * slightly-diagonal segments along their dominant axis, normalize, and only
 * when the result still violates G2-G6, re-route. The valve is re-projected onto
 * the healed path and the margin applied. A non-finite terminal returns the
 * flow unchanged; non-finite points (or fewer than two) route afresh.
 */
export function heal(flow: FlowViewElement, terminals: Terminals, ctx: HealContext = {}): FlowGeometry {
  if (!terminalIsFinite(terminals.source) || !terminalIsFinite(terminals.sink)) {
    return { flow, clouds: [] };
  }
  const pts = flow.points;
  if (pts.length < 2 || !pts.every(isFiniteXY)) {
    return route(terminals.source, terminals.sink, { flow: { ...flow, points: [] } });
  }
  if (isHealthy(flow, terminals, ctx.stocks)) {
    return { flow, clouds: cloudUpdates(pts, terminals) };
  }
  const healed: XY[] = pts.map((p) => ({ x: p.x, y: p.y }));
  const n = healed.length;
  for (const [t, index, adjacent] of [
    [terminals.source, 0, 1],
    [terminals.sink, n - 1, n - 2],
  ] as const) {
    if (t.kind !== 'stock') {
      continue;
    }
    const p = healed[index];
    const face = faceOfEndpoint(t.stock, p, healed[adjacent]);
    if (face === undefined) {
      healed[index] = nearestFaceAttachment(t.stock, p, healed[adjacent]).point;
      continue;
    }
    const att = faceAttachment(t.stock, face);
    healed[index] = facePoint(att, coord(p, att.along));
  }
  const movable = (index: number): boolean =>
    (index > 0 && index < n - 1) ||
    (index === 0 && terminals.source.kind === 'free') ||
    (index === n - 1 && terminals.sink.kind === 'free');
  for (let i = 0; i < n - 1; i++) {
    const a = healed[i];
    const b = healed[i + 1];
    if (Math.abs(a.x - b.x) <= GEOMETRY_EPSILON || Math.abs(a.y - b.y) <= GEOMETRY_EPSILON) {
      continue;
    }
    const H = otherAxis(segmentAxisOf(a, b));
    if (movable(i + 1)) {
      healed[i + 1] = compose(H, coord(a, H), coord(b, otherAxis(H)));
    } else if (movable(i)) {
      healed[i] = compose(H, coord(b, H), coord(a, otherAxis(H)));
    }
  }
  if (terminals.source.kind === 'free') {
    healed[0] = outOfStocks(healed[0], healed[1], ctx.stocks);
  }
  if (terminals.sink.kind === 'free') {
    healed[n - 1] = outOfStocks(healed[n - 1], healed[n - 2], ctx.stocks);
  }
  // A free terminal is wherever its endpoint was healed to (its cloud follows),
  // so a re-route starts from there, not from the cloud's original center: that
  // center may be exactly the stock interior the endpoint was just moved out of.
  const moved: Terminals = {
    source: terminals.source.kind === 'free' ? freeTerminal(healed[0], terminals.source.cloud) : terminals.source,
    sink: terminals.sink.kind === 'free' ? freeTerminal(healed[n - 1], terminals.sink.cloud) : terminals.sink,
  };
  let points: XY[] = normalize(attachPoints(healed, moved));
  if (pathQuality(points, moved, ctx.stocks).fault !== FAULT_NONE) {
    points = routeBetween(moved, { source: false, sink: false }, points, []);
  }
  const length = pathLength(points);
  const valve = isFiniteXY(flow)
    ? pointAtArc(points, applyValveMargin(length, arcPosition(points, flow)))
    : placeValve(points, 'source', undefined);
  return withGeometry(flow, points, valve, moved);
}

/**
 * A free endpoint inside a stock moved along its adjacent segment's axis to the
 * nearer edge of that stock, so the cloud lands on the boundary (not inside,
 * G6) and the segment stays orthogonal.
 */
function outOfStocks(p: XY, adjacent: XY, stocks: readonly XY[] | undefined): XY {
  const stock = stocks?.find((s) => strictlyInside(p, stockBody(s)));
  if (stock === undefined) {
    return p;
  }
  const axis = segmentAxisOf(p, adjacent);
  const body = stockBody(stock);
  const lo = axis === 'x' ? body.minX : body.minY;
  const hi = axis === 'x' ? body.maxX : body.maxY;
  const v = coord(p, axis);
  return compose(axis, v - lo <= hi - v ? lo : hi, coord(p, otherAxis(axis)));
}

/**
 * A flow heal leaves alone: attached to its terminals, valid (G2-G6, clouds
 * outside `stocks`), and its valve on the path within the margin. A free
 * terminal always sits at its own endpoint here: `flowTerminals` reads a cloud
 * terminal at the cloud's center, and a cloud off its endpoint is reported (and
 * moved) through `cloudUpdates` either way.
 */
function isHealthy(flow: FlowViewElement, terminals: Terminals, stocks: readonly XY[] | undefined): boolean {
  const pts = flow.points;
  const n = pts.length;
  if (
    pts[0].attachedToUid !== terminalUid(terminals.source) ||
    pts[n - 1].attachedToUid !== terminalUid(terminals.sink)
  ) {
    return false;
  }
  if (pts.some((p, i) => i > 0 && i < n - 1 && p.attachedToUid !== undefined)) {
    return false;
  }
  if (pathQuality(pts, terminals, stocks).fault !== FAULT_NONE || !isFiniteXY(flow)) {
    return false;
  }
  if (distanceToPath(pts, flow) > GEOMETRY_EPSILON) {
    return false;
  }
  const length = pathLength(pts);
  if (length < 2 * VALVE_CLAMP_MARGIN) {
    return true;
  }
  const s = arcPosition(pts, flow);
  return Math.min(s, length - s) >= VALVE_CLAMP_MARGIN - GEOMETRY_EPSILON;
}
