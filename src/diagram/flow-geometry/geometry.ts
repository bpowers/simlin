// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Constants, units and small geometry shared by the flow-geometry modules.
 * Positions are absolute model coordinates (px at zoom 1).
 */

import { FlowArrowheadRadius, StockHeight, StockWidth } from '../drawing/default';

/** A stock endpoint stays this far from its face's corners (G4). */
export const CORNER_CLEARANCE = 3;
/** The shortest routed stub or riser (G3). */
export const MIN_SEGMENT = 10;
/** The valve keeps this arc-length distance from the path's ends when the path is long enough (G8). */
export const VALVE_CLAMP_MARGIN = 10;
/**
 * The shortest final segment (G3). The renderer pulls the path back 7.5px to
 * seat the arrowhead (`finalAdjust` in drawing/Flow.tsx); a shorter final
 * segment tucks the arrowhead into the preceding turn.
 */
export const MIN_SINK_SEGMENT = FlowArrowheadRadius + 7.5;
/** Preferred distance between two endpoints on one face (a routing preference, not an invariant). */
export const PIPE_SPACING = 10;
/**
 * Coordinates within this distance are equal. Face points are fractions of the
 * stock size and valves are interpolated, so exact comparison would report
 * float noise; every defect the invariants exist for is a pixel or more.
 */
export const GEOMETRY_EPSILON = 1e-6;

export const HALF_WIDTH = StockWidth / 2;
export const HALF_HEIGHT = StockHeight / 2;

export type Face = 'left' | 'right' | 'top' | 'bottom';
export const FACES: readonly Face[] = ['left', 'right', 'top', 'bottom'];
export type Axis = 'x' | 'y';
export type FlowEnd = 'source' | 'sink';

export interface XY {
  readonly x: number;
  readonly y: number;
}

export function otherAxis(axis: Axis): Axis {
  return axis === 'x' ? 'y' : 'x';
}

/** The point whose `axis` coordinate is `v` and whose other coordinate is `w`. */
export function compose(axis: Axis, v: number, w: number): XY {
  return axis === 'x' ? { x: v, y: w } : { x: w, y: v };
}

export function coord(p: XY, axis: Axis): number {
  return axis === 'x' ? p.x : p.y;
}

export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

export function distance(a: XY, b: XY): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

export function samePoint(a: XY, b: XY): boolean {
  return Math.abs(a.x - b.x) <= GEOMETRY_EPSILON && Math.abs(a.y - b.y) <= GEOMETRY_EPSILON;
}

export function isFiniteXY(p: XY): boolean {
  return Number.isFinite(p.x) && Number.isFinite(p.y);
}

/**
 * A segment's working axis. Exactly axis-aligned segments are what routing
 * produces; imported data can carry a few pixels of drift, which is classified
 * by its dominant axis (ties count as horizontal).
 */
export function segmentAxisOf(a: XY, b: XY): Axis {
  return Math.abs(b.y - a.y) <= Math.abs(b.x - a.x) ? 'x' : 'y';
}

export interface Box {
  readonly minX: number;
  readonly maxX: number;
  readonly minY: number;
  readonly maxY: number;
}

export function stockBody(stock: XY): Box {
  return {
    minX: stock.x - HALF_WIDTH,
    maxX: stock.x + HALF_WIDTH,
    minY: stock.y - HALF_HEIGHT,
    maxY: stock.y + HALF_HEIGHT,
  };
}

export function pointBody(p: XY): Box {
  return { minX: p.x, maxX: p.x, minY: p.y, maxY: p.y };
}

export function inflate(box: Box, by: number): Box {
  return { minX: box.minX - by, maxX: box.maxX + by, minY: box.minY - by, maxY: box.maxY + by };
}

// Touching boxes do not overlap: two bodies exactly MIN_SEGMENT apart leave
// exactly enough room for a routed stub.
export function boxesOverlap(a: Box, b: Box): boolean {
  return a.minX < b.maxX && b.minX < a.maxX && a.minY < b.maxY && b.minY < a.maxY;
}

/**
 * Does a segment pass through the open interior of `box` with positive length?
 * A segment starting on a face or running along an edge line is not "through".
 * The positive-length tolerance is GEOMETRY_EPSILON; on orthogonal paths a larger
 * one would change nothing observable, because an axis-aligned segment can only
 * overlap the interior shallowly by ending inside it, where the adjacent segment
 * or the endpoint check reports the crossing anyway.
 */
export function segmentThroughBox(a: XY, b: XY, box: Box): boolean {
  const e = GEOMETRY_EPSILON;
  if (Math.abs(a.y - b.y) <= e) {
    if (!(a.y > box.minY + e && a.y < box.maxY - e)) {
      return false;
    }
    const lo = Math.max(Math.min(a.x, b.x), box.minX + e);
    const hi = Math.min(Math.max(a.x, b.x), box.maxX - e);
    return hi - lo > e;
  }
  if (Math.abs(a.x - b.x) <= e) {
    if (!(a.x > box.minX + e && a.x < box.maxX - e)) {
      return false;
    }
    const lo = Math.max(Math.min(a.y, b.y), box.minY + e);
    const hi = Math.min(Math.max(a.y, b.y), box.maxY - e);
    return hi - lo > e;
  }
  // A diagonal is structurally invalid on its own; routing never produces one.
  // Sample it so an imported diagonal still reads as crossing when it does.
  for (let t = 0; t <= 1; t += 0.05) {
    const x = a.x + (b.x - a.x) * t;
    const y = a.y + (b.y - a.y) * t;
    if (x > box.minX + e && x < box.maxX - e && y > box.minY + e && y < box.maxY - e) {
      return true;
    }
  }
  return false;
}

export function strictlyInside(p: XY, box: Box): boolean {
  const e = GEOMETRY_EPSILON;
  return p.x > box.minX + e && p.x < box.maxX - e && p.y > box.minY + e && p.y < box.maxY - e;
}
