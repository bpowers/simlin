// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Face attachment and terminals. This module is the single owner of how a pipe
 * attaches to a stock: route's candidates, routeEnd's pinned terminal,
 * offsetSegment's tail re-solve and heal's re-pin all derive endpoints, outward
 * directions and stub tips from `faceAttachment` and `stubTip`.
 */

import type {
  CloudViewElement,
  FlowViewElement,
  Point,
  StockViewElement,
  UID,
  ViewElement,
} from '@simlin/core/datamodel';

import {
  type Axis,
  type Box,
  clamp,
  compose,
  CORNER_CLEARANCE,
  coord,
  distance,
  type Face,
  FACES,
  GEOMETRY_EPSILON,
  HALF_HEIGHT,
  HALF_WIDTH,
  isFiniteXY,
  pointBody,
  segmentAxisOf,
  stockBody,
  type XY,
} from './geometry';

/** Where a pipe may attach to one face of a stock. */
export interface FaceAttachment {
  readonly face: Face;
  /** The axis a segment leaving this face runs along. */
  readonly normal: Axis;
  /** The axis the face itself runs along. */
  readonly along: Axis;
  /** +1 when leaving the face increases the normal coordinate. */
  readonly sign: 1 | -1;
  /** The face's coordinate on the normal axis. */
  readonly plane: number;
  /** The stock center's coordinate on the along axis. */
  readonly center: number;
  /** Valid endpoint positions on the along axis: the face extent minus CORNER_CLEARANCE. */
  readonly lo: number;
  readonly hi: number;
}

export function faceAttachment(stock: XY, face: Face): FaceAttachment {
  switch (face) {
    case 'left':
    case 'right': {
      const extent = HALF_HEIGHT - CORNER_CLEARANCE;
      return {
        face,
        normal: 'x',
        along: 'y',
        sign: face === 'right' ? 1 : -1,
        plane: stock.x + (face === 'right' ? HALF_WIDTH : -HALF_WIDTH),
        center: stock.y,
        lo: stock.y - extent,
        hi: stock.y + extent,
      };
    }
    case 'top':
    case 'bottom': {
      const extent = HALF_WIDTH - CORNER_CLEARANCE;
      return {
        face,
        normal: 'y',
        along: 'x',
        sign: face === 'bottom' ? 1 : -1,
        plane: stock.y + (face === 'bottom' ? HALF_HEIGHT : -HALF_HEIGHT),
        center: stock.x,
        lo: stock.x - extent,
        hi: stock.x + extent,
      };
    }
  }
}

/** The normal-axis coordinate of a stub `min` long leaving the face. */
export function stubTip(att: FaceAttachment, min: number): number {
  return att.plane + att.sign * min;
}

/** The endpoint at `along` on the face, clamped into the valid range. */
export function facePoint(att: FaceAttachment, along: number): XY {
  return compose(att.along, clamp(along, att.lo, att.hi), att.plane);
}

/**
 * The face an endpoint lies on (within GEOMETRY_EPSILON), or undefined when it
 * is on none. A corner point is on two faces; the one whose adjacent segment
 * leaves perpendicular is preferred when `adjacent` is given.
 */
export function faceOfEndpoint(stock: XY, p: XY, adjacent?: XY): Face | undefined {
  const dx = p.x - stock.x;
  const dy = p.y - stock.y;
  const e = GEOMETRY_EPSILON;
  const faces: Face[] = [];
  if (Math.abs(Math.abs(dx) - HALF_WIDTH) <= e && Math.abs(dy) <= HALF_HEIGHT + e) {
    faces.push(dx > 0 ? 'right' : 'left');
  }
  if (Math.abs(Math.abs(dy) - HALF_HEIGHT) <= e && Math.abs(dx) <= HALF_WIDTH + e) {
    faces.push(dy > 0 ? 'bottom' : 'top');
  }
  if (faces.length > 1 && adjacent !== undefined) {
    const axis = segmentAxisOf(p, adjacent);
    const perpendicular = faces.find((f) => faceAttachment(stock, f).normal === axis);
    if (perpendicular !== undefined) {
      return perpendicular;
    }
  }
  return faces[0];
}

/**
 * The nearest valid face point to `p`, over all four faces. When `adjacent` is
 * given, a tie (a corner point, equidistant from two faces) goes to the face the
 * adjacent segment leaves perpendicular to, so a stub keeps its direction.
 */
export function nearestFaceAttachment(stock: XY, p: XY, adjacent?: XY): { readonly face: Face; readonly point: XY } {
  const preferred = adjacent === undefined ? undefined : segmentAxisOf(p, adjacent);
  let best: { face: Face; point: XY } | undefined;
  let bestDistance = Infinity;
  for (const face of FACES) {
    const att = faceAttachment(stock, face);
    const point = facePoint(att, coord(p, att.along));
    const d = distance(point, p);
    const tie = Math.abs(d - bestDistance) <= GEOMETRY_EPSILON;
    if (d < bestDistance - GEOMETRY_EPSILON || (tie && preferred !== undefined && att.normal === preferred)) {
      bestDistance = Math.min(d, bestDistance);
      best = { face, point };
    }
  }
  return best!;
}

/**
 * A stock terminal. `face` and `offset` describe the BASE flow's attachment
 * (offset is the endpoint's along-face distance from the face center), so
 * stickiness needs no previous frame; both are undefined for a stock the flow
 * was not attached to before (a new target).
 */
export interface StockTerminal {
  readonly kind: 'stock';
  readonly stock: StockViewElement;
  readonly face?: Face;
  readonly offset?: number;
}

/**
 * A free terminal: a cloud, or a point with no element (the pointer before a
 * cloud exists). The endpoint is attached to `cloud` when one is given, and the
 * cloud is moved onto the endpoint when an operation moves the endpoint.
 */
export interface FreeTerminal {
  readonly kind: 'free';
  readonly point: XY;
  readonly cloud?: CloudViewElement;
}

export type Terminal = StockTerminal | FreeTerminal;

export interface Terminals {
  readonly source: Terminal;
  readonly sink: Terminal;
}

/**
 * A stock terminal whose base attachment is read from `endpoint` relative to
 * `from` (the stock's base position; defaults to `stock`). A planner moving a
 * stock passes the moved stock and the base stock, so the base face and offset
 * travel with the stock. An endpoint on no face attaches to the nearest face.
 */
export function stockTerminal(stock: StockViewElement, endpoint?: XY, adjacent?: XY, from: XY = stock): StockTerminal {
  if (endpoint === undefined || !isFiniteXY(endpoint)) {
    return { kind: 'stock', stock };
  }
  const face = faceOfEndpoint(from, endpoint, adjacent) ?? nearestFaceAttachment(from, endpoint, adjacent).face;
  const att = faceAttachment(from, face);
  return { kind: 'stock', stock, face, offset: coord(endpoint, att.along) - att.center };
}

export function freeTerminal(point: XY, cloud?: CloudViewElement): FreeTerminal {
  const at = { x: point.x, y: point.y };
  return cloud === undefined ? { kind: 'free', point: at } : { kind: 'free', point: at, cloud };
}

/**
 * A flow's terminals as the view supplies them: a stock endpoint's base face
 * and offset are read from the endpoint (and its adjacent point, which picks
 * the perpendicular face at a corner); a cloud endpoint's terminal is its
 * cloud, at the cloud's center; an unattached or dangling endpoint is a free
 * point with no cloud, which no operation attaches.
 */
export function flowTerminals(flow: FlowViewElement, byUid: ReadonlyMap<UID, ViewElement>): Terminals {
  const pts = flow.points;
  const n = pts.length;
  const terminalAt = (index: number, adjacentIndex: number): Terminal => {
    const p = pts[index];
    if (p === undefined) {
      return freeTerminal({ x: flow.x, y: flow.y });
    }
    const el = p.attachedToUid === undefined ? undefined : byUid.get(p.attachedToUid);
    if (el?.type === 'stock') {
      return stockTerminal(el, p, pts[adjacentIndex]);
    }
    if (el?.type === 'cloud') {
      return freeTerminal(el, el);
    }
    return freeTerminal(p);
  };
  return { source: terminalAt(0, 1), sink: terminalAt(n - 1, n - 2) };
}

export function terminalUid(t: Terminal): UID | undefined {
  return t.kind === 'stock' ? t.stock.uid : t.cloud?.uid;
}

export function terminalBody(t: Terminal): Box {
  return t.kind === 'stock' ? stockBody(t.stock) : pointBody(t.point);
}

/**
 * The geometry an operation produced: the new flow, plus every cloud whose
 * position changed because its endpoint moved (a cloud endpoint always equals
 * its cloud's center, G7).
 */
export interface FlowGeometry {
  readonly flow: FlowViewElement;
  readonly clouds: readonly CloudViewElement[];
}

export function cloudUpdates(points: readonly XY[], terminals: Terminals): CloudViewElement[] {
  const out: CloudViewElement[] = [];
  const ends: Array<[Terminal, XY]> = [
    [terminals.source, points[0]],
    [terminals.sink, points[points.length - 1]],
  ];
  for (const [t, p] of ends) {
    if (t.kind === 'free' && t.cloud !== undefined && (t.cloud.x !== p.x || t.cloud.y !== p.y)) {
      out.push({ ...t.cloud, x: p.x, y: p.y });
    }
  }
  return out;
}

export function attachPoints(points: readonly XY[], terminals: Terminals): Point[] {
  const last = points.length - 1;
  return points.map((p, i) => ({
    x: p.x,
    y: p.y,
    attachedToUid: i === 0 ? terminalUid(terminals.source) : i === last ? terminalUid(terminals.sink) : undefined,
  }));
}

export function withGeometry(
  flow: FlowViewElement,
  points: readonly XY[],
  valve: XY,
  terminals: Terminals,
): FlowGeometry {
  return {
    flow: { ...flow, x: valve.x, y: valve.y, points: attachPoints(points, terminals) },
    clouds: cloudUpdates(points, terminals),
  };
}

/** Whether every coordinate a terminal carries is finite (a NaN pointer is a caller bug, never geometry). */
export function terminalIsFinite(t: Terminal): boolean {
  return t.kind === 'stock'
    ? isFiniteXY(t.stock) && (t.offset === undefined || Number.isFinite(t.offset))
    : isFiniteXY(t.point);
}
