// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Small pure helpers every gesture plan shares: the click threshold, element
 * lookup and merging, hit tests, the other endpoints on a stock, and link arcs
 * that follow their moved endpoints.
 */

import {
  isNamedViewElement,
  type CloudViewElement,
  type FlowViewElement,
  type LinkViewElement,
  type StockFlowView,
  type UID,
  type ViewElement,
} from '@simlin/core/datamodel';

import { radToDeg, updateArcAngle } from '../arc-utils';
import { getVisualCenter } from '../drawing/Connector';
import { AuxRadius, ModuleHeight, ModuleWidth, StockHeight, StockWidth } from '../drawing/default';
import { ClickDragThresholdPx } from '../drawing/pointer-utils';
import { GEOMETRY_EPSILON, type XY } from '../flow-geometry';
import type { GesturePlan, PlanInput } from './types';

/**
 * Whether the pointer has moved past the click threshold. The threshold is in
 * screen pixels (a finger or mouse wobbles in screen space), so the model-space
 * distance is scaled by the zoom before comparing.
 */
export function beyondThreshold(press: XY, current: XY, zoom: number): boolean {
  return Math.hypot(current.x - press.x, current.y - press.y) * zoom >= ClickDragThresholdPx;
}

export function delta(from: XY, to: XY): XY {
  return { x: to.x - from.x, y: to.y - from.y };
}

/** A flow's endpoint at `end`, read off its own points (a routed flow can have more points than its base). */
export function endpointOf(points: readonly XY[], end: 'source' | 'sink'): XY {
  return end === 'source' ? points[0] : points[points.length - 1];
}

/**
 * A flow about to be routed, with each end attached the way strict G1 requires:
 * an end with no attachment, a dangling one, or one attached to anything but a
 * stock or this flow's own cloud (imported data the editor accepts, e.g. Vensim
 * fallback flows) gets a new cloud at its endpoint, taking uids from `nextUid`.
 * The geometry core leaves such an end free and unattached, so the planner owns
 * attaching it. Identity (and no clouds) when both ends are attached.
 */
export function attachLooseEnds(
  flow: FlowViewElement,
  byUid: ReadonlyMap<UID, ViewElement>,
  nextUid: number,
): { readonly flow: FlowViewElement; readonly clouds: readonly CloudViewElement[]; readonly nextUid: number } {
  const last = flow.points.length - 1;
  const clouds: CloudViewElement[] = [];
  let uid = nextUid;
  const points = flow.points.map((p, i) => {
    if (i !== 0 && i !== last) {
      return p;
    }
    const el = p.attachedToUid === undefined ? undefined : byUid.get(p.attachedToUid);
    if (el?.type === 'stock' || (el?.type === 'cloud' && el.flowUid === flow.uid)) {
      return p;
    }
    const cloud: CloudViewElement = {
      type: 'cloud',
      uid: uid++,
      flowUid: flow.uid,
      x: p.x,
      y: p.y,
      isZeroRadius: false,
      ident: undefined,
    };
    clouds.push(cloud);
    return { ...p, attachedToUid: cloud.uid };
  });
  return clouds.length === 0 ? { flow, clouds, nextUid } : { flow: { ...flow, points }, clouds, nextUid: uid };
}

export function byUidOf(elements: readonly ViewElement[]): Map<UID, ViewElement> {
  return new Map(elements.map((el) => [el.uid, el]));
}

/**
 * The view's elements with `changed` substituted by uid (in place, so element
 * order and z-order are kept), `removed` dropped, and `added` appended. An
 * element in `changed` whose uid the view lacks is appended too.
 */
export function mergeElements(
  elements: readonly ViewElement[],
  changed: ReadonlyMap<UID, ViewElement>,
  removed: ReadonlySet<UID> = new Set(),
): ViewElement[] {
  const out: ViewElement[] = [];
  const seen = new Set<UID>();
  for (const el of elements) {
    seen.add(el.uid);
    if (removed.has(el.uid)) {
      continue;
    }
    out.push(changed.get(el.uid) ?? el);
  }
  for (const [uid, el] of changed) {
    if (!seen.has(uid) && !removed.has(uid)) {
      out.push(el);
    }
  }
  return out;
}

/** The plan that changes nothing: what a sub-threshold frame, a read-only drag and an aborted subject all render. */
export function idlePlan(input: PlanInput): GesturePlan {
  return {
    elements: input.view.elements,
    nextUid: input.view.nextUid,
    commit: 'none',
    selection: input.selection,
    label: '',
  };
}

/**
 * The plan of a release within the click threshold: nothing moves, and the
 * selection settles on the press's click selection when that differs.
 */
export function clickPlan(input: PlanInput, details: boolean): GesturePlan {
  const selection = input.clickSelection ?? input.selection;
  const changed = !sameSet(selection, input.selection);
  return { ...idlePlan(input), selection, commit: changed ? 'select' : 'none', details };
}

export function sameSet<T>(a: ReadonlySet<T>, b: ReadonlySet<T>): boolean {
  return a.size === b.size && [...a].every((v) => b.has(v));
}

export function stockContains(stock: XY, p: XY): boolean {
  return Math.abs(p.x - stock.x) <= StockWidth / 2 && Math.abs(p.y - stock.y) <= StockHeight / 2;
}

function circleContains(center: XY, p: XY, radius: number): boolean {
  return Math.hypot(p.x - center.x, p.y - center.y) <= radius;
}

/** The first stock of the view whose body contains `p`. */
export function stockUnder(view: StockFlowView, p: XY) {
  for (const el of view.elements) {
    if (el.type === 'stock' && stockContains(el, p)) {
      return el;
    }
  }
  return undefined;
}

/**
 * The first element a link can point at under `p`: an aux (its circle), a flow
 * (its valve) or a module (its body). Stocks and aliases are not link targets.
 */
export function linkTargetUnder(view: StockFlowView, p: XY): ViewElement | undefined {
  for (const el of view.elements) {
    if ((el.type === 'aux' || el.type === 'flow') && circleContains(el, p, AuxRadius)) {
      return el;
    }
    if (el.type === 'module' && Math.abs(p.x - el.x) <= ModuleWidth / 2 && Math.abs(p.y - el.y) <= ModuleHeight / 2) {
      return el;
    }
  }
  return undefined;
}

export function stockPositions(elements: readonly ViewElement[]): XY[] {
  return elements.filter((el) => el.type === 'stock').map((el) => ({ x: el.x, y: el.y }));
}

/**
 * The endpoints of flows other than `flowUid` attached to any of `stockUids`,
 * for the slot preference. `shift` moves the endpoints on a moving stock with
 * it, since the preference reads the frame's coordinates.
 */
export function occupiedOn(
  elements: readonly ViewElement[],
  flowUid: UID,
  stockUids: ReadonlySet<UID>,
  shift: (stockUid: UID) => XY = () => ({ x: 0, y: 0 }),
): XY[] {
  const out: XY[] = [];
  for (const el of elements) {
    if (el.type !== 'flow' || el.uid === flowUid || el.points.length < 2) {
      continue;
    }
    for (const p of [el.points[0], el.points[el.points.length - 1]]) {
      if (p.attachedToUid !== undefined && stockUids.has(p.attachedToUid)) {
        const d = shift(p.attachedToUid);
        out.push({ x: p.x + d.x, y: p.y + d.y });
      }
    }
  }
  return out;
}

/** The index of the segment of `points` nearest to `p` (the earliest on a tie). */
export function segmentNearest(points: readonly XY[], p: XY): number {
  let best = 0;
  let bestDistance = Infinity;
  for (let i = 0; i < points.length - 1; i++) {
    const a = points[i];
    const b = points[i + 1];
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const l2 = dx * dx + dy * dy;
    const t = l2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l2));
    const d = Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
    if (d < bestDistance - GEOMETRY_EPSILON) {
      bestDistance = d;
      best = i;
    }
  }
  return best;
}

export type LabelSideName = 'top' | 'left' | 'bottom' | 'right';

/**
 * The side a label snaps to for a pointer at `pointer` around an element at
 * `center`: the quadrant of the direction from the pointer toward the center
 * (a pointer left of the center puts the label on the left).
 */
export function labelSideForPointer(center: XY, pointer: XY): LabelSideName {
  const angle = (Math.atan2(center.y - pointer.y, center.x - pointer.x) * 180) / Math.PI;
  if (-45 < angle && angle <= 45) {
    return 'left';
  } else if (45 < angle && angle <= 135) {
    return 'top';
  } else if (-135 < angle && angle <= -45) {
    return 'bottom';
  }
  return 'right';
}

// Endpoints moving by the same amount within this tolerance translate a link
// rather than bend it: float noise between two routed positions is not rotation.
const MOVEMENT_EQUALITY_EPSILON = 0.1;

/**
 * Links whose endpoint elements moved, updated once from the final elements:
 * both ends moved alike keeps the arc (and translates a multi-point path);
 * otherwise the arc turns with the line between the endpoints' visual centers,
 * so the curve keeps its shape relative to that line. A straight link (no arc)
 * stays straight. Returns the changed links by uid; `next` holds only the
 * elements the gesture changed.
 */
export function followLinks(
  base: ReadonlyMap<UID, ViewElement>,
  next: ReadonlyMap<UID, ViewElement>,
): Map<UID, LinkViewElement> {
  const out = new Map<UID, LinkViewElement>();
  for (const link of base.values()) {
    if (link.type !== 'link' || next.has(link.uid)) {
      continue;
    }
    const oldFrom = base.get(link.fromUid);
    const oldTo = base.get(link.toUid);
    if (oldFrom === undefined || oldTo === undefined) {
      continue;
    }
    const newFrom = next.get(link.fromUid) ?? oldFrom;
    const newTo = next.get(link.toUid) ?? oldTo;
    const fromDelta = delta(newFrom, oldFrom);
    const toDelta = delta(newTo, oldTo);
    const didMove = fromDelta.x !== 0 || fromDelta.y !== 0 || toDelta.x !== 0 || toDelta.y !== 0;
    if (!didMove) {
      continue;
    }
    const sameMovement =
      Math.abs(fromDelta.x - toDelta.x) < MOVEMENT_EQUALITY_EPSILON &&
      Math.abs(fromDelta.y - toDelta.y) < MOVEMENT_EQUALITY_EPSILON;
    if (sameMovement) {
      if (link.multiPoint !== undefined) {
        const multiPoint = link.multiPoint.map((p) => ({ ...p, x: p.x - fromDelta.x, y: p.y - fromDelta.y }));
        out.set(link.uid, { ...link, multiPoint });
      }
      continue;
    }
    const oldFromVisual = getVisualCenter(oldFrom);
    const oldToVisual = getVisualCenter(oldTo);
    const newFromVisual = getVisualCenter(newFrom);
    const newToVisual = getVisualCenter(newTo);
    const oldθ = Math.atan2(oldToVisual.cy - oldFromVisual.cy, oldToVisual.cx - oldFromVisual.cx);
    const newθ = Math.atan2(newToVisual.cy - newFromVisual.cy, newToVisual.cx - newFromVisual.cx);
    out.set(link.uid, { ...link, arc: updateArcAngle(link.arc, radToDeg(oldθ - newθ)) });
  }
  return out;
}

/** Whether `el` can be the source of a link: a named element or an alias of one (M3). */
export function isLinkSource(el: ViewElement): boolean {
  return isNamedViewElement(el) || el.type === 'alias';
}
