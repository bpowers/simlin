// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The flow geometry invariants G1-G8 of
 * docs/design-plans/2026-09-10-diagram-editing-core.md, as a checker over a
 * StockFlowView.
 *
 * Every check arm has its own name in FLOW_ARMS and its own small function, so
 * a change to one definition in the plan stays local, and the tests derive
 * their fixture rows from that enumeration.
 *
 * Modes. `strict` is what a committed edit must produce for every flow it
 * routed. `tolerant` is what an INPUT view must satisfy for the editor to
 * accept it at all: the plan requires the editor to render imported and legacy
 * views unmodified and heal a flow only when an edit routes it, and `heal` is
 * specified to repair every geometric arm (off-face endpoints, clouds off their
 * endpoint, diagonal segments, unnormalized points), so tolerant mode keeps only
 * the structural G1 arms whose violation leaves nothing to heal from.
 */

import type {
  CloudViewElement,
  FlowViewElement,
  Point,
  StockFlowView,
  StockViewElement,
  UID,
  ViewElement,
} from '@simlin/core/datamodel';

import { FlowArrowheadRadius, StockHeight, StockWidth } from '../../drawing/default';

export const CORNER_CLEARANCE = 3;
export const MIN_SEGMENT = 10;
export const VALVE_CLAMP_MARGIN = 10;
// The renderer pulls the path back 7.5px along the final segment to seat the
// arrowhead (`finalAdjust` in drawing/Flow.tsx); a shorter final segment tucks
// the arrowhead's back into the preceding turn.
export const MIN_SINK_SEGMENT = FlowArrowheadRadius + 7.5;
export const PIPE_SPACING = 10;

// Face points are fractions of the stock size and valves are interpolated, so
// exact float comparison would report noise; every defect these checks exist
// for is a pixel or more.
export const GEOMETRY_EPSILON = 1e-6;

const HALF_WIDTH = StockWidth / 2;
const HALF_HEIGHT = StockHeight / 2;

export const FLOW_ARMS = {
  G1: [
    'minPoints',
    'nonFinite',
    'unattachedEndpoint',
    'danglingAttachment',
    'attachmentKind',
    'foreignCloud',
    'interiorAttached',
    'nonPositiveUid',
    'sourceIsSink',
  ],
  G2: ['diagonal'],
  G3: ['zeroLength', 'collinear', 'shortStub', 'shortRiser', 'shortSink'],
  G4: ['offFace', 'cornerClearance'],
  G5: ['notPerpendicular', 'inward'],
  G6: ['segmentThroughTerminal', 'cloudInsideStock'],
  G7: ['cloudOffEndpoint'],
  G8: ['valveOffPath', 'valveMargin'],
} as const;

export type FlowInvariant = keyof typeof FLOW_ARMS;
export type FlowArm = { [K in FlowInvariant]: `${K}.${(typeof FLOW_ARMS)[K][number]}` }[FlowInvariant];

export const ALL_FLOW_ARMS: readonly FlowArm[] = (Object.keys(FLOW_ARMS) as FlowInvariant[]).flatMap((invariant) =>
  FLOW_ARMS[invariant].map((arm) => `${invariant}.${arm}` as FlowArm),
);

/**
 * The arms tolerant mode still reports. Two structural arms are deliberately
 * absent: an unattached endpoint, because the Vensim importer emits flows with no
 * attachment and the editor must accept them; and a flow whose source and sink are
 * the same element, which no corpus import produces (#720 only speculates that one
 * could) but which tolerating is the permissive direction -- the editor must never
 * throw on it, and routing such a flow is heal's job, not the input gate's.
 */
export const TOLERANT_FLOW_ARMS: ReadonlySet<FlowArm> = new Set<FlowArm>([
  'G1.minPoints',
  'G1.nonFinite',
  'G1.danglingAttachment',
  'G1.attachmentKind',
  'G1.foreignCloud',
  'G1.interiorAttached',
]);

export type FlowInvariantMode = 'strict' | 'tolerant';

export interface FlowInvariantOptions {
  readonly mode: FlowInvariantMode;
  /**
   * The flows an edit routed. In strict mode the full arm set applies only to
   * these (the rest of the view may carry imported violations) and every other
   * flow gets the tolerant arms. Undefined means every flow is routed.
   */
  readonly routed?: ReadonlySet<UID>;
}

export interface FlowViolation {
  readonly arm: FlowArm;
  /** The flow's uid; for G1.nonPositiveUid, the offending element's. */
  readonly uid: UID;
  readonly numbers: Readonly<Record<string, number>>;
  readonly message: string;
}

export function formatFlowViolations(violations: readonly FlowViolation[]): string {
  return violations.map((v) => `${v.arm} uid=${v.uid} ${JSON.stringify(v.numbers)} ${v.message}`).join('\n');
}

type Terminal =
  | { readonly kind: 'stock'; readonly stock: StockViewElement }
  | { readonly kind: 'cloud'; readonly cloud: CloudViewElement };

interface FlowContext {
  readonly flow: FlowViewElement;
  readonly source: Terminal | undefined;
  readonly sink: Terminal | undefined;
}

type Report = (arm: FlowArm, numbers: Record<string, number>, message: string) => void;

export function checkFlowInvariants(view: StockFlowView, opts: FlowInvariantOptions): FlowViolation[] {
  const out: FlowViolation[] = [];
  const byUid = new Map<UID, ViewElement>();
  for (const el of view.elements) {
    byUid.set(el.uid, el);
  }

  if (opts.mode === 'strict') {
    checkNonPositiveUids(view, out);
  }

  for (const el of view.elements) {
    if (el.type !== 'flow') {
      continue;
    }
    const strict = opts.mode === 'strict' && (opts.routed === undefined || opts.routed.has(el.uid));
    const report: Report = (arm, numbers, message) => {
      if (strict || TOLERANT_FLOW_ARMS.has(arm)) {
        out.push({ arm, uid: el.uid, numbers, message });
      }
    };
    const ctx = checkStructure(el, byUid, report);
    if (ctx === undefined) {
      continue;
    }
    checkOrthogonal(ctx, report);
    checkNormalized(ctx, report);
    checkFaceAttachment(ctx, report);
    checkPerpendicularExit(ctx, report);
    checkNoBodyCrossing(ctx, view, report);
    checkCloudCoincidence(ctx, report);
    checkValveOnPath(ctx, report);
  }
  return out;
}

// ---------------------------------------------------------------------------
// G1 structure

// "No uid <= 0 in a committed view": the Canvas stages sentinel uids (-2..-5)
// for in-creation elements, so this is a property of committed views only and
// applies to every element, routed or not.
function checkNonPositiveUids(view: StockFlowView, out: FlowViolation[]): void {
  for (const el of view.elements) {
    if (el.uid <= 0) {
      out.push({
        arm: 'G1.nonPositiveUid',
        uid: el.uid,
        numbers: { uid: el.uid },
        message: `${el.type} element has uid ${el.uid}`,
      });
    }
  }
}

/**
 * Returns the resolved terminals, or undefined when the flow is too broken for
 * any geometric arm to mean anything (fewer than two points, or a non-finite
 * coordinate every later computation would only echo).
 */
function checkStructure(
  flow: FlowViewElement,
  byUid: ReadonlyMap<UID, ViewElement>,
  report: Report,
): FlowContext | undefined {
  const pts = flow.points;
  const coordinates = [flow.x, flow.y, ...pts.flatMap((p) => [p.x, p.y])];
  const nonFinite = coordinates.filter((v) => !Number.isFinite(v)).length;
  if (nonFinite > 0) {
    report('G1.nonFinite', { count: nonFinite }, `${nonFinite} non-finite coordinate(s)`);
  }
  if (pts.length < 2) {
    report('G1.minPoints', { points: pts.length }, `flow has ${pts.length} point(s)`);
    return undefined;
  }
  for (let i = 1; i < pts.length - 1; i++) {
    const attached = pts[i].attachedToUid;
    if (attached !== undefined) {
      report('G1.interiorAttached', { index: i, attachedToUid: attached }, `interior point ${i} is attached`);
    }
  }
  const source = resolveTerminal(flow, 0, byUid, report);
  const sink = resolveTerminal(flow, pts.length - 1, byUid, report);
  const sourceUid = pts[0].attachedToUid;
  // A cloud at both ends is also M3.cloudEndpointCount (a cloud is an endpoint of
  // its flow exactly once); this arm owns the stock self-loop the planner refuses.
  if (source !== undefined && sink !== undefined && sourceUid === pts[pts.length - 1].attachedToUid) {
    report('G1.sourceIsSink', { attachedToUid: sourceUid! }, `source and sink are both element ${sourceUid}`);
  }
  if (nonFinite > 0) {
    return undefined;
  }
  return { flow, source, sink };
}

function resolveTerminal(
  flow: FlowViewElement,
  index: number,
  byUid: ReadonlyMap<UID, ViewElement>,
  report: Report,
): Terminal | undefined {
  const end = index === 0 ? 'source' : 'sink';
  const attached = flow.points[index].attachedToUid;
  if (attached === undefined) {
    report('G1.unattachedEndpoint', { endIndex: index }, `${end} endpoint is unattached`);
    return undefined;
  }
  const el = byUid.get(attached);
  if (el === undefined) {
    report(
      'G1.danglingAttachment',
      { endIndex: index, attachedToUid: attached },
      `${end} references missing uid ${attached}`,
    );
    return undefined;
  }
  if (el.type === 'stock') {
    return { kind: 'stock', stock: el };
  }
  if (el.type === 'cloud') {
    if (el.flowUid !== flow.uid) {
      report(
        'G1.foreignCloud',
        { endIndex: index, cloudUid: el.uid, cloudFlowUid: el.flowUid },
        `${end} cloud ${el.uid} belongs to flow ${el.flowUid}`,
      );
      return undefined;
    }
    return { kind: 'cloud', cloud: el };
  }
  report('G1.attachmentKind', { endIndex: index, attachedToUid: attached }, `${end} is attached to a ${el.type}`);
  return undefined;
}

// ---------------------------------------------------------------------------
// G2 orthogonal

function checkOrthogonal(ctx: FlowContext, report: Report): void {
  const pts = ctx.flow.points;
  for (let i = 0; i < pts.length - 1; i++) {
    if (orientation(pts[i], pts[i + 1]) === 'diagonal') {
      report(
        'G2.diagonal',
        { segment: i, dx: pts[i + 1].x - pts[i].x, dy: pts[i + 1].y - pts[i].y },
        `segment ${i} is not axis-aligned`,
      );
    }
  }
}

// ---------------------------------------------------------------------------
// G3 normalized

function checkNormalized(ctx: FlowContext, report: Report): void {
  const pts = ctx.flow.points;
  const segmentCount = pts.length - 1;
  for (let i = 0; i < segmentCount; i++) {
    if (orientation(pts[i], pts[i + 1]) === 'zero') {
      report('G3.zeroLength', { segment: i }, `segment ${i} has zero length`);
    }
  }
  for (let i = 0; i + 1 < segmentCount; i++) {
    const a = orientation(pts[i], pts[i + 1]);
    const b = orientation(pts[i + 1], pts[i + 2]);
    if ((a === 'horizontal' || a === 'vertical') && a === b) {
      report('G3.collinear', { segment: i + 1 }, `segments ${i} and ${i + 1} are collinear`);
    }
  }
  if (!terminalsLeaveRoom(ctx)) {
    return;
  }
  for (let i = 0; i < segmentCount; i++) {
    const length = distance(pts[i], pts[i + 1]);
    if (length <= GEOMETRY_EPSILON) {
      continue;
    }
    if (i === segmentCount - 1) {
      if (length < MIN_SINK_SEGMENT - GEOMETRY_EPSILON) {
        report('G3.shortSink', { segment: i, length, minimum: MIN_SINK_SEGMENT }, `final segment is ${length}px`);
      }
    } else if (i === 0) {
      if (length < MIN_SEGMENT - GEOMETRY_EPSILON) {
        report('G3.shortStub', { segment: i, length, minimum: MIN_SEGMENT }, `first segment is ${length}px`);
      }
    } else if (length < MIN_SEGMENT - GEOMETRY_EPSILON) {
      report('G3.shortRiser', { segment: i, length, minimum: MIN_SEGMENT }, `interior segment ${i} is ${length}px`);
    }
  }
}

/**
 * G3's "whenever the terminals leave room": a route needs at least a
 * MIN_SEGMENT stub out of the source and a MIN_SINK_SEGMENT segment into the
 * sink, so the minima are demanded only when the source body inflated by
 * MIN_SEGMENT and the sink body inflated by MIN_SINK_SEGMENT are disjoint. A
 * missing terminal (an unattached endpoint, tolerated input) cannot crowd.
 */
function terminalsLeaveRoom(ctx: FlowContext): boolean {
  if (ctx.source === undefined || ctx.sink === undefined) {
    return true;
  }
  return !boxesOverlap(
    inflate(terminalBody(ctx.source), MIN_SEGMENT),
    inflate(terminalBody(ctx.sink), MIN_SINK_SEGMENT),
  );
}

// ---------------------------------------------------------------------------
// G4 face attachment

function checkFaceAttachment(ctx: FlowContext, report: Report): void {
  for (const [index, terminal] of endTerminals(ctx)) {
    if (terminal?.kind !== 'stock') {
      continue;
    }
    const p = ctx.flow.points[index];
    const stock = terminal.stock;
    const faces = facesOf(p, stock);
    if (faces.length === 0) {
      report(
        'G4.offFace',
        { endIndex: index, dx: p.x - stock.x, dy: p.y - stock.y },
        `endpoint is not on a face of stock ${stock.uid}`,
      );
      continue;
    }
    const clearance = Math.min(...faces.map((face) => faceClearance(p, stock, face)));
    if (clearance < CORNER_CLEARANCE - GEOMETRY_EPSILON) {
      report(
        'G4.cornerClearance',
        { endIndex: index, clearance, minimum: CORNER_CLEARANCE },
        `endpoint is ${clearance}px from a corner of stock ${stock.uid}`,
      );
    }
  }
}

// ---------------------------------------------------------------------------
// G5 perpendicular exit

function checkPerpendicularExit(ctx: FlowContext, report: Report): void {
  const pts = ctx.flow.points;
  for (const [index, terminal] of endTerminals(ctx)) {
    if (terminal?.kind !== 'stock') {
      continue;
    }
    const p = pts[index];
    const faces = facesOf(p, terminal.stock);
    // "That face" is undefined for an off-face endpoint; G4.offFace owns it.
    if (faces.length === 0) {
      continue;
    }
    // The adjacent segment is the first one of positive length, so a coincident
    // point (G3.zeroLength) does not hide the direction the pipe actually takes.
    const neighbor = firstDistinctNeighbor(pts, index);
    if (neighbor === undefined) {
      continue;
    }
    const dx = neighbor.x - p.x;
    const dy = neighbor.y - p.y;
    if (faces.some((face) => leavesOutward(face, dx, dy))) {
      continue;
    }
    const perpendicular = faces.some((face) =>
      face === 'left' || face === 'right' ? Math.abs(dy) <= GEOMETRY_EPSILON : Math.abs(dx) <= GEOMETRY_EPSILON,
    );
    report(
      perpendicular ? 'G5.inward' : 'G5.notPerpendicular',
      { endIndex: index, dx, dy },
      `segment at the ${faces.join('/')} face ${perpendicular ? 'points into the stock' : 'is not perpendicular to it'}`,
    );
  }
}

function leavesOutward(face: Face, dx: number, dy: number): boolean {
  const horizontal = Math.abs(dy) <= GEOMETRY_EPSILON;
  const vertical = Math.abs(dx) <= GEOMETRY_EPSILON;
  switch (face) {
    case 'left':
      return horizontal && dx < 0;
    case 'right':
      return horizontal && dx > 0;
    case 'top':
      return vertical && dy < 0;
    case 'bottom':
      return vertical && dy > 0;
  }
}

// ---------------------------------------------------------------------------
// G6 no body crossing

/**
 * "No cloud center lies inside a stock" is read as ANY stock of the view. Read
 * as this flow's other terminal the clause could never fire: a cloud inside
 * that stock makes the inflated terminal bodies overlap, which is exactly the
 * precondition that exempts G6.
 */
function checkNoBodyCrossing(ctx: FlowContext, view: StockFlowView, report: Report): void {
  // With one terminal missing (tolerated input) there is no pair to overlap,
  // so the precondition holds.
  if (
    ctx.source !== undefined &&
    ctx.sink !== undefined &&
    boxesOverlap(inflate(terminalBody(ctx.source), MIN_SEGMENT), inflate(terminalBody(ctx.sink), MIN_SEGMENT))
  ) {
    return;
  }
  const pts = ctx.flow.points;
  const stocks = uniqueTerminalStocks(ctx);
  for (const stock of stocks) {
    for (let i = 0; i < pts.length - 1; i++) {
      if (segmentEntersInterior(pts[i], pts[i + 1], stock)) {
        report(
          'G6.segmentThroughTerminal',
          { segment: i, stockUid: stock.uid },
          `segment ${i} crosses stock ${stock.uid}`,
        );
      }
    }
  }
  for (const [, terminal] of endTerminals(ctx)) {
    if (terminal?.kind !== 'cloud') {
      continue;
    }
    for (const el of view.elements) {
      if (el.type === 'stock' && strictlyInside(terminal.cloud, el)) {
        report(
          'G6.cloudInsideStock',
          { cloudUid: terminal.cloud.uid, stockUid: el.uid },
          `cloud ${terminal.cloud.uid} lies inside stock ${el.uid}`,
        );
      }
    }
  }
}

function uniqueTerminalStocks(ctx: FlowContext): StockViewElement[] {
  const stocks: StockViewElement[] = [];
  for (const [, terminal] of endTerminals(ctx)) {
    if (terminal?.kind === 'stock' && !stocks.some((s) => s.uid === terminal.stock.uid)) {
      stocks.push(terminal.stock);
    }
  }
  return stocks;
}

// ---------------------------------------------------------------------------
// G7 cloud coincidence

function checkCloudCoincidence(ctx: FlowContext, report: Report): void {
  for (const [index, terminal] of endTerminals(ctx)) {
    if (terminal?.kind !== 'cloud') {
      continue;
    }
    const d = distance(ctx.flow.points[index], terminal.cloud);
    if (d > GEOMETRY_EPSILON) {
      report(
        'G7.cloudOffEndpoint',
        { endIndex: index, distance: d },
        `cloud ${terminal.cloud.uid} is ${d}px from its endpoint`,
      );
    }
  }
}

// ---------------------------------------------------------------------------
// G8 valve on path

/**
 * "When the path is long enough" is read as: long enough for a position at
 * least VALVE_CLAMP_MARGIN from both ends to exist, i.e. >= 2 margins. The
 * margin is measured in arc length from the path's ends, not per segment.
 */
function checkValveOnPath(ctx: FlowContext, report: Report): void {
  const pts = ctx.flow.points;
  const valve = { x: ctx.flow.x, y: ctx.flow.y };
  let best = Infinity;
  let arcPosition = 0;
  let traversed = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    const length = distance(pts[i], pts[i + 1]);
    const { d, t } = distanceToSegment(valve, pts[i], pts[i + 1]);
    if (d < best) {
      best = d;
      arcPosition = traversed + t * length;
    }
    traversed += length;
  }
  if (best > GEOMETRY_EPSILON) {
    report('G8.valveOffPath', { distance: best }, `valve is ${best}px off the path`);
    return;
  }
  const pathLength = traversed;
  if (pathLength < 2 * VALVE_CLAMP_MARGIN) {
    return;
  }
  const fromEnd = Math.min(arcPosition, pathLength - arcPosition);
  if (fromEnd < VALVE_CLAMP_MARGIN - GEOMETRY_EPSILON) {
    report(
      'G8.valveMargin',
      { arcPosition, pathLength, margin: VALVE_CLAMP_MARGIN },
      `valve is ${fromEnd}px from an end of the path`,
    );
  }
}

// ---------------------------------------------------------------------------
// Geometry

type Face = 'left' | 'right' | 'top' | 'bottom';

interface Box {
  readonly minX: number;
  readonly maxX: number;
  readonly minY: number;
  readonly maxY: number;
}

function endTerminals(ctx: FlowContext): Array<[number, Terminal | undefined]> {
  return [
    [0, ctx.source],
    [ctx.flow.points.length - 1, ctx.sink],
  ];
}

function terminalBody(terminal: Terminal): Box {
  if (terminal.kind === 'stock') {
    const s = terminal.stock;
    return { minX: s.x - HALF_WIDTH, maxX: s.x + HALF_WIDTH, minY: s.y - HALF_HEIGHT, maxY: s.y + HALF_HEIGHT };
  }
  const c = terminal.cloud;
  return { minX: c.x, maxX: c.x, minY: c.y, maxY: c.y };
}

function inflate(box: Box, by: number): Box {
  return { minX: box.minX - by, maxX: box.maxX + by, minY: box.minY - by, maxY: box.maxY + by };
}

// Touching boxes do not overlap: the precondition exempts crowding, and two
// bodies exactly MIN_SEGMENT apart leave exactly enough room.
function boxesOverlap(a: Box, b: Box): boolean {
  return a.minX < b.maxX && b.minX < a.maxX && a.minY < b.maxY && b.minY < a.maxY;
}

type Orientation = 'zero' | 'horizontal' | 'vertical' | 'diagonal';

function orientation(a: Point, b: Point): Orientation {
  const flatX = Math.abs(b.x - a.x) <= GEOMETRY_EPSILON;
  const flatY = Math.abs(b.y - a.y) <= GEOMETRY_EPSILON;
  if (flatX && flatY) return 'zero';
  if (flatY) return 'horizontal';
  if (flatX) return 'vertical';
  return 'diagonal';
}

function distance(a: { x: number; y: number }, b: { x: number; y: number }): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

function distanceToSegment(p: { x: number; y: number }, a: Point, b: Point): { d: number; t: number } {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lengthSquared = dx * dx + dy * dy;
  const t = lengthSquared === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / lengthSquared));
  return { d: Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy)), t };
}

function firstDistinctNeighbor(pts: readonly Point[], index: number): Point | undefined {
  const step = index === 0 ? 1 : -1;
  for (let i = index + step; i >= 0 && i < pts.length; i += step) {
    if (orientation(pts[index], pts[i]) !== 'zero') {
      return pts[i];
    }
  }
  return undefined;
}

function facesOf(p: Point, stock: StockViewElement): Face[] {
  const dx = p.x - stock.x;
  const dy = p.y - stock.y;
  const faces: Face[] = [];
  if (Math.abs(Math.abs(dx) - HALF_WIDTH) <= GEOMETRY_EPSILON && Math.abs(dy) <= HALF_HEIGHT + GEOMETRY_EPSILON) {
    faces.push(dx > 0 ? 'right' : 'left');
  }
  if (Math.abs(Math.abs(dy) - HALF_HEIGHT) <= GEOMETRY_EPSILON && Math.abs(dx) <= HALF_WIDTH + GEOMETRY_EPSILON) {
    faces.push(dy > 0 ? 'bottom' : 'top');
  }
  return faces;
}

function faceClearance(p: Point, stock: StockViewElement, face: Face): number {
  return face === 'left' || face === 'right'
    ? HALF_HEIGHT - Math.abs(p.y - stock.y)
    : HALF_WIDTH - Math.abs(p.x - stock.x);
}

function strictlyInside(p: { x: number; y: number }, stock: StockViewElement): boolean {
  return (
    Math.abs(p.x - stock.x) < HALF_WIDTH - GEOMETRY_EPSILON && Math.abs(p.y - stock.y) < HALF_HEIGHT - GEOMETRY_EPSILON
  );
}

// Liang-Barsky clip against the stock rectangle inset by GEOMETRY_EPSILON, so a
// segment that starts on a face or runs along an edge line is not "through".
function segmentEntersInterior(a: Point, b: Point, stock: StockViewElement): boolean {
  const minX = stock.x - HALF_WIDTH + GEOMETRY_EPSILON;
  const maxX = stock.x + HALF_WIDTH - GEOMETRY_EPSILON;
  const minY = stock.y - HALF_HEIGHT + GEOMETRY_EPSILON;
  const maxY = stock.y + HALF_HEIGHT - GEOMETRY_EPSILON;
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  let t0 = 0;
  let t1 = 1;
  const clip = (p: number, q: number): boolean => {
    if (p === 0) {
      return q > 0;
    }
    const r = q / p;
    if (p < 0) {
      if (r > t1) return false;
      if (r > t0) t0 = r;
    } else {
      if (r < t0) return false;
      if (r < t1) t1 = r;
    }
    return true;
  };
  if (!clip(-dx, a.x - minX) || !clip(dx, maxX - a.x) || !clip(-dy, a.y - minY) || !clip(dy, maxY - a.y)) {
    return false;
  }
  return (t1 - t0) * Math.hypot(dx, dy) > GEOMETRY_EPSILON;
}
