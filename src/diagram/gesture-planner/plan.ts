// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * planGesture: one frame of a gesture, evaluated from the press and the pointer
 * now against the rendered view. The preview renders its `elements`, and a
 * release commits the plan evaluated at the release point, which is exactly the
 * frame the preview showed there (E2).
 */

import { canonicalize } from '@simlin/core/canonicalize';
import {
  isNamedViewElement,
  type CloudViewElement,
  type FlowViewElement,
  type StockViewElement,
  type UID,
  type ViewElement,
} from '@simlin/core/datamodel';

import { inCreationUid } from '../drawing/creation-sentinels';
import { AuxRadius } from '../drawing/default';
import {
  flowFault,
  flowTerminals,
  freeTerminal,
  heal,
  offsetSegment,
  routeEnd,
  segmentHold,
  slideValve,
  stockTerminal,
  translate,
  type FlowEnd,
  type Terminal,
  type XY,
} from '../flow-geometry';
import { latchGesture } from './classify';
import {
  attachLooseEnds,
  beyondThreshold,
  byUidOf,
  clickPlan,
  delta,
  endpointOf,
  followLinks,
  idlePlan,
  labelSideForPointer,
  mergeElements,
  occupiedOn,
  segmentNearest,
  stockPositions,
} from './common';
import { planCreateFlow, planFlowEndpoint } from './flow-ends';
import { planCreateLink, planLinkArc, planLinkEndpoint } from './links';
import type { GesturePlan, PlanInput } from './types';

/**
 * Plan one frame. Common to every gesture that edits the view: a frame within
 * the click threshold changes nothing (E1) and settles the click's selection; a
 * read-only frame changes nothing; a missing subject changes nothing. The
 * exceptions: an armed creation tool places its draft on a click, a label drag
 * was already past the label's own threshold when it started, and a rubber band
 * previews only its selection. A pipe press is latched here from the pointer
 * when the caller has not latched it (the Canvas latches on the first move and
 * keeps it).
 */
export function planGesture(input: PlanInput): GesturePlan {
  const gesture = latchGesture(input.gesture, input);
  const moved = beyondThreshold(input.press, input.current, input.zoom);
  switch (gesture.kind) {
    case 'pan':
      return idlePlan(input);
    case 'rubberBand':
      return planRubberBand(input, moved);
    case 'createElement':
      return input.readOnly ? idlePlan(input) : planCreateElement(input, gesture.type, moved);
    case 'label':
      return input.readOnly ? idlePlan(input) : planLabel(input, gesture.uid);
    case 'pipe':
      return clickPlan(input, true);
  }
  if (!moved) {
    const bodyClick = gesture.kind === 'moveSelection' || gesture.kind === 'linkArc';
    return clickPlan(input, bodyClick || gesture.kind === 'slideValve' || gesture.kind === 'offsetSegment');
  }
  if (input.readOnly) {
    return idlePlan(input);
  }
  switch (gesture.kind) {
    case 'moveSelection':
      return planMoveSelection(input);
    case 'slideValve':
      return planSlideValve(input, gesture.flow);
    case 'offsetSegment':
      return planOffsetSegment(input, gesture.flow, gesture.segmentIndex);
    case 'flowEndpoint':
      return planFlowEndpoint(input, gesture.flow, gesture.end);
    case 'createFlow':
      return planCreateFlow(input, gesture.from);
    case 'createLink':
      return planCreateLink(input, gesture.from);
    case 'linkEndpoint':
      return planLinkEndpoint(input, gesture.link);
    case 'linkArc':
      return planLinkArc(input, gesture.link);
  }
}

function editPlan(
  input: PlanInput,
  changed: Map<UID, ViewElement>,
  label: string,
  nextUid = input.view.nextUid,
): GesturePlan {
  if (changed.size === 0) {
    return idlePlan(input);
  }
  const base = byUidOf(input.view.elements);
  for (const [uid, link] of followLinks(base, changed)) {
    changed.set(uid, link);
  }
  return {
    elements: mergeElements(input.view.elements, changed),
    nextUid,
    commit: 'edit',
    selection: input.selection,
    label,
  };
}

/**
 * The view's flow with its loose ends attached to new clouds (uids from `uids`)
 * and healed against its terminals, identity on a valid attached flow; the
 * lookup map carries every cloud created or moved onto its endpoint.
 */
function healed(
  flow: FlowViewElement,
  base: ReadonlyMap<UID, ViewElement>,
  stocks: readonly XY[],
  uids: { next: number },
): {
  readonly flow: FlowViewElement;
  readonly byUid: Map<UID, ViewElement>;
  readonly clouds: readonly CloudViewElement[];
} {
  const loose = attachLooseEnds(flow, base, uids.next);
  uids.next = loose.nextUid;
  const byUid = new Map(base);
  const clouds = new Map<UID, CloudViewElement>();
  for (const cloud of loose.clouds) {
    byUid.set(cloud.uid, cloud);
    clouds.set(cloud.uid, cloud);
  }
  const g = heal(loose.flow, flowTerminals(loose.flow, byUid), { stocks });
  for (const cloud of g.clouds) {
    byUid.set(cloud.uid, cloud);
    clouds.set(cloud.uid, cloud);
  }
  return { flow: g.flow, byUid, clouds: [...clouds.values()] };
}

function flowOf(input: PlanInput, uid: UID): FlowViewElement | undefined {
  const el = input.view.elements.find((e) => e.uid === uid);
  return el?.type === 'flow' && el.points.length >= 2 ? el : undefined;
}

function isPositioned(el: ViewElement): boolean {
  return (
    el.type === 'stock' ||
    el.type === 'aux' ||
    el.type === 'module' ||
    el.type === 'alias' ||
    el.type === 'cloud' ||
    el.type === 'group'
  );
}

/**
 * Move the selection: selected positioned elements translate; a flow whose two
 * terminals both move translates; a flow with one moving terminal is healed and
 * `routeEnd`-ed to its moved terminal (a stock carries the base face and offset
 * along; a cloud is re-centered on the routed endpoint); a selected flow with no
 * moving terminal slides its valve. Links follow their moved endpoints.
 */
function planMoveSelection(input: PlanInput): GesturePlan {
  const { view, selection } = input;
  const d = delta(input.press, input.current);
  const base = byUidOf(view.elements);
  const changed = new Map<UID, ViewElement>();
  const uids = { next: view.nextUid };
  for (const uid of selection) {
    const el = base.get(uid);
    if (el !== undefined && isPositioned(el)) {
      changed.set(uid, { ...el, x: el.x + d.x, y: el.y + d.y } as ViewElement);
    }
  }
  const moving = new Set(changed.keys());
  const stocks = stockPositions(view.elements);
  const frameStocks = view.elements
    .filter((e) => e.type === 'stock')
    .map((e) => (moving.has(e.uid) ? { x: e.x + d.x, y: e.y + d.y } : { x: e.x, y: e.y }));
  let valid = true;
  const terminalMoves = (uid: UID | undefined): boolean => {
    const el = uid === undefined ? undefined : base.get(uid);
    return uid !== undefined && moving.has(uid) && (el?.type === 'stock' || el?.type === 'cloud');
  };
  for (const el of view.elements) {
    if (el.type !== 'flow' || el.points.length < 2) {
      continue;
    }
    const sourceMoves = terminalMoves(el.points[0].attachedToUid);
    const sinkMoves = terminalMoves(el.points[el.points.length - 1].attachedToUid);
    if (sourceMoves && sinkMoves) {
      changed.set(el.uid, translate(el, d));
    } else if (sourceMoves || sinkMoves) {
      const end = sourceMoves ? 'source' : 'sink';
      valid = routeMovedEnd(input, el, end, d, changed, base, stocks, moving, uids, frameStocks) && valid;
    } else if (selection.has(el.uid)) {
      const h = healed(el, base, stocks, uids);
      for (const cloud of h.clouds) {
        changed.set(cloud.uid, cloud);
      }
      changed.set(el.uid, slideValve(h.flow, d));
    }
  }
  // A routed flow whose geometry cannot hold G2-G6 in this frame (a dragged
  // cloud inside another stock, say) makes the move an invalid drop: it previews
  // and commits nothing, like an invalid target (E6).
  const plan = editPlan(input, changed, 'move', uids.next);
  return valid ? plan : { ...plan, commit: 'none', label: '' };
}

function routeMovedEnd(
  input: PlanInput,
  el: FlowViewElement,
  end: FlowEnd,
  d: XY,
  changed: Map<UID, ViewElement>,
  base: ReadonlyMap<UID, ViewElement>,
  stocks: readonly XY[],
  moving: ReadonlySet<UID>,
  uids: { next: number },
  frameStocks: readonly XY[],
): boolean {
  const h = healed(el, base, stocks, uids);
  const flow = h.flow;
  const n = flow.points.length;
  const endIndex = end === 'source' ? 0 : n - 1;
  const adjacentIndex = end === 'source' ? 1 : n - 2;
  const endUid = flow.points[endIndex].attachedToUid!;
  const terminalEl = h.byUid.get(endUid)!;
  const terminals = flowTerminals(flow, h.byUid);
  const fixed = end === 'source' ? terminals.sink : terminals.source;
  let terminal: Terminal;
  if (terminalEl.type === 'stock') {
    const moved = changed.get(endUid) as StockViewElement;
    terminal = stockTerminal(moved, flow.points[endIndex], flow.points[adjacentIndex], terminalEl);
  } else {
    const cloud = terminalEl as CloudViewElement;
    const at = { x: cloud.x + d.x, y: cloud.y + d.y };
    terminal = freeTerminal(at, { ...cloud, ...at });
  }
  const stockUids = new Set<UID>();
  for (const t of [terminal, fixed]) {
    if (t.kind === 'stock') {
      stockUids.add(t.stock.uid);
    }
  }
  const occupied = occupiedOn(input.view.elements, el.uid, stockUids, (uid) => (moving.has(uid) ? d : { x: 0, y: 0 }));
  const g = routeEnd(flow, end, terminal, { fixed, occupied });
  for (const cloud of h.clouds) {
    changed.set(cloud.uid, cloud);
  }
  for (const cloud of g.clouds) {
    changed.set(cloud.uid, cloud);
  }
  if (terminalEl.type === 'cloud') {
    const p = endpointOf(g.flow.points, end);
    changed.set(endUid, { ...terminalEl, x: p.x, y: p.y });
  }
  changed.set(el.uid, g.flow);
  const routedTerminals = end === 'source' ? { source: terminal, sink: fixed } : { source: fixed, sink: terminal };
  return flowFault(g.flow, routedTerminals, frameStocks) === 'none';
}

/** Slide a sole selected flow's valve along its (healed) path by the pointer travel. */
function planSlideValve(input: PlanInput, flowUid: UID): GesturePlan {
  const el = flowOf(input, flowUid);
  if (el === undefined) {
    return idlePlan(input);
  }
  const uids = { next: input.view.nextUid };
  const h = healed(el, byUidOf(input.view.elements), stockPositions(input.view.elements), uids);
  const changed = new Map<UID, ViewElement>(h.clouds.map((c) => [c.uid, c]));
  changed.set(flowUid, slideValve(h.flow, delta(input.press, input.current)));
  return editPlan(input, changed, 'valve move', uids.next);
}

/**
 * Offset the pressed segment of a sole selected flow perpendicular to itself by
 * the pointer travel. When healing an imported flow changed its points, the
 * segment is the healed path's nearest to the press.
 */
function planOffsetSegment(input: PlanInput, flowUid: UID, segmentIndex: number): GesturePlan {
  const el = flowOf(input, flowUid);
  if (el === undefined) {
    return idlePlan(input);
  }
  const stocks = stockPositions(input.view.elements);
  const uids = { next: input.view.nextUid };
  const h = healed(el, byUidOf(input.view.elements), stocks, uids);
  const index = h.flow === el ? segmentIndex : segmentNearest(h.flow.points, input.press);
  if (index < 0 || index >= h.flow.points.length - 1) {
    return idlePlan(input);
  }
  const d = delta(input.press, input.current);
  const { axis, hold } = segmentHold(h.flow.points, index);
  const coordinate = hold + (axis === 'x' ? d.y : d.x);
  const g = offsetSegment(h.flow, index, coordinate, flowTerminals(h.flow, h.byUid), { stocks });
  const changed = new Map<UID, ViewElement>(h.clouds.map((c) => [c.uid, c]));
  for (const cloud of g.clouds) {
    changed.set(cloud.uid, cloud);
  }
  changed.set(flowUid, g.flow);
  return editPlan(input, changed, 'pipe move', uids.next);
}

/**
 * Move a label to the side the pointer points it at. No click threshold here:
 * the label component starts the gesture only once its own threshold is
 * crossed, and a side that does not change commits nothing.
 */
function planLabel(input: PlanInput, uid: UID): GesturePlan {
  const el = input.view.elements.find((e) => e.uid === uid);
  if (el === undefined || !(isNamedViewElement(el) || el.type === 'alias')) {
    return idlePlan(input);
  }
  const side = labelSideForPointer(el, input.current);
  if (side === el.labelSide) {
    return idlePlan(input);
  }
  return editPlan(input, new Map([[uid, { ...el, labelSide: side }]]), 'label move');
}

/**
 * The rubber band's selection: stocks, clouds, flows (by valve), modules and
 * aliases whose center lies in the rectangle, and auxes whose center lies in it
 * or whose circle holds one of its corners. Links and groups are never
 * rubber-band selected. A click selects nothing.
 */
function planRubberBand(input: PlanInput, moved: boolean): GesturePlan {
  if (!moved) {
    return { ...idlePlan(input), commit: 'select', selection: input.clickSelection ?? new Set() };
  }
  const left = Math.min(input.press.x, input.current.x);
  const right = Math.max(input.press.x, input.current.x);
  const top = Math.min(input.press.y, input.current.y);
  const bottom = Math.max(input.press.y, input.current.y);
  const inside = (p: XY): boolean => p.x >= left && p.x <= right && p.y >= top && p.y <= bottom;
  const corners: XY[] = [
    { x: left, y: top },
    { x: right, y: top },
    { x: left, y: bottom },
    { x: right, y: bottom },
  ];
  const selection = new Set<UID>();
  for (const el of input.view.elements) {
    switch (el.type) {
      case 'cloud':
      case 'stock':
      case 'flow':
      case 'module':
      case 'alias':
        if (inside(el)) {
          selection.add(el.uid);
        }
        break;
      case 'aux':
        if (inside(el) || corners.some((c) => Math.hypot(c.x - el.x, c.y - el.y) <= AuxRadius)) {
          selection.add(el.uid);
        }
        break;
      default:
        break;
    }
  }
  return { ...idlePlan(input), commit: 'select', selection };
}

const DRAFT_NAMES = { aux: 'New Variable', stock: 'New Stock', module: 'New Module' } as const;

/**
 * An armed aux, stock or module tool stages a draft under its default name at
 * the pointer (at the press for a click), then hands off to the name editor.
 * Nothing commits here: the draft lives in the Canvas through name editing and
 * the create is planned against the view rendered when the name is done.
 */
function planCreateElement(input: PlanInput, type: 'aux' | 'stock' | 'module', moved: boolean): GesturePlan {
  const at = moved ? input.current : input.press;
  const name = input.names(DRAFT_NAMES[type]);
  const common = {
    uid: inCreationUid,
    var: undefined,
    x: at.x,
    y: at.y,
    name,
    ident: canonicalize(name),
    isZeroRadius: false,
  } as const;
  const draft: ViewElement =
    type === 'aux'
      ? { ...common, type: 'aux', labelSide: 'right' }
      : type === 'stock'
        ? { ...common, type: 'stock', labelSide: 'bottom', inflows: [], outflows: [] }
        : { ...common, type: 'module', labelSide: 'bottom' };
  return {
    ...idlePlan(input),
    elements: [...input.view.elements, draft],
    draft,
    handoff: { editName: inCreationUid },
  };
}
