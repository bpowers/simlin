// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The flow geometry core: one owner for how a flow's pipe attaches to stocks,
 * how it is routed, how a segment is offset, and where its valve sits. Pure: no
 * React, no DOM, never mutates its inputs.
 *
 * The invariants this module maintains are G2-G8 of
 * docs/design-plans/2026-09-10-diagram-editing-core.md. Positions are absolute
 * model coordinates. A terminal is either a stock (attached through one of its
 * four faces) or a free point (a cloud, or the pointer during a drag); the
 * stored cloud endpoint is the cloud's center, as the renderer expects.
 *
 * Every routing decision reads the gesture's BASE flow (the flow as it was when
 * the gesture started) rather than a previous frame, so a planner can evaluate
 * any frame at any pointer position and get the same answer: preview and commit
 * cannot diverge, and continuity is a property of these functions, not of the
 * order frames were computed in.
 *
 * Modules: `geometry` (constants, units, boxes), `terminal` (face attachment and
 * terminals), `validity` (G2-G6 classification), `path` (normalize, arc length,
 * the valve, translate, slideValve), `route` (route, routeEnd), `offset-segment`
 * and `heal`.
 */

export {
  CORNER_CLEARANCE,
  FACES,
  GEOMETRY_EPSILON,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  PIPE_SPACING,
  VALVE_CLAMP_MARGIN,
  type Axis,
  type Face,
  type FlowEnd,
  type XY,
} from './geometry';
export {
  faceAttachment,
  faceOfEndpoint,
  facePoint,
  flowTerminals,
  freeTerminal,
  nearestFaceAttachment,
  stockTerminal,
  stubTip,
  type FaceAttachment,
  type FlowGeometry,
  type FreeTerminal,
  type StockTerminal,
  type Terminal,
  type Terminals,
} from './terminal';
export { flowFault, type RouteFault } from './validity';
export {
  arcPosition,
  normalize,
  pathLength,
  placeValve,
  pointAtArc,
  slideValve,
  translate,
  valveDistance,
} from './path';
export { route, routeEnd, type RouteContext, type RouteEndContext } from './route';
export { offsetSegment, segmentHold, type OffsetContext } from './offset-segment';
export { heal, type HealContext } from './heal';
