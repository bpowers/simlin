// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Gestures that move a flow's end onto a stock or into empty space: dragging an
 * existing end (`flowEndpoint`) and drawing a new flow (`createFlow`).
 */

import { canonicalize } from '@simlin/core/canonicalize';
import type { CloudViewElement, FlowViewElement, StockViewElement, UID, ViewElement } from '@simlin/core/datamodel';

import {
  flowFault,
  flowTerminals,
  freeTerminal,
  heal,
  route,
  routeEnd,
  stockTerminal,
  type FlowEnd,
  type Terminal,
  type Terminals,
  type XY,
} from '../flow-geometry';
import {
  attachLooseEnds,
  byUidOf,
  delta,
  endpointOf,
  followLinks,
  idlePlan,
  mergeElements,
  occupiedOn,
  stockPositions,
  stockUnder,
} from './common';
import type { GesturePlan, PlanInput } from './types';

function stockVariableExists(input: PlanInput, stock: StockViewElement): boolean {
  return input.variables.get(canonicalize(stock.name))?.type === 'stock';
}

function stockUidsOf(...terminals: readonly Terminal[]): Set<UID> {
  const out = new Set<UID>();
  for (const t of terminals) {
    if (t.kind === 'stock') {
      out.add(t.stock.uid);
    }
  }
  return out;
}

function newCloud(uid: UID, flowUid: UID, at: XY): CloudViewElement {
  return { type: 'cloud', uid, flowUid, x: at.x, y: at.y, isZeroRadius: false, ident: undefined };
}

/**
 * Drag one end of a flow. The end follows the pointer, keeping the grab offset
 * (it moves by the pointer's travel from the press). The pointer, not the end,
 * hit-tests targets:
 *
 * - over a stock, the end routes onto it, and the drop is valid exactly when
 *   the stock's variable exists, the stock is not the flow's other end, and the
 *   routed flow holds G2-G6; a valid drop deletes the end's old cloud;
 * - over an invalid stock the preview shows the end free at the pointer and the
 *   drop commits nothing (E6);
 * - over empty space the end becomes, or stays, a cloud at the end, committing
 *   when the routed flow holds G2-G6.
 *
 * Cloud-attached and stock-attached ends are one gesture. The flow is healed
 * first, since the drag routes it.
 */
export function planFlowEndpoint(input: PlanInput, flowUid: UID, end: FlowEnd): GesturePlan {
  const { view } = input;
  const base = byUidOf(view.elements);
  const el = base.get(flowUid);
  if (el?.type !== 'flow' || el.points.length < 2) {
    return idlePlan(input);
  }
  const stocks = stockPositions(view.elements);
  const loose = attachLooseEnds(el, base, view.nextUid);
  const byUid = new Map(base);
  for (const cloud of loose.clouds) {
    byUid.set(cloud.uid, cloud);
  }
  const h = heal(loose.flow, flowTerminals(loose.flow, byUid), { stocks });
  for (const cloud of h.clouds) {
    byUid.set(cloud.uid, cloud);
  }
  const flow = h.flow;
  const n = flow.points.length;
  const endIndex = end === 'source' ? 0 : n - 1;
  const adjacentIndex = end === 'source' ? 1 : n - 2;
  const terminals = flowTerminals(flow, byUid);
  const fixed = end === 'source' ? terminals.sink : terminals.source;
  const attachedUid = flow.points[endIndex].attachedToUid;
  const endEl = attachedUid === undefined ? undefined : byUid.get(attachedUid);
  // Every stock is in the way, the one this end leaves included: a route keeping
  // the old face's line would run straight through it, and a pipe through any
  // stock reads as attached to it.
  const obstacles = stocks;
  const withEnd = (t: Terminal): Terminals =>
    end === 'source' ? { source: t, sink: fixed } : { source: fixed, sink: t };
  const changed = new Map<UID, ViewElement>([...loose.clouds, ...h.clouds].map((c) => [c.uid, c]));
  const finish = (
    commit: GesturePlan['commit'],
    nextUid: number,
    target: GesturePlan['target'],
    removed: ReadonlySet<UID>,
  ) => {
    for (const [uid, link] of followLinks(base, changed)) {
      changed.set(uid, link);
    }
    return {
      elements: mergeElements(view.elements, changed, removed),
      nextUid,
      target,
      commit,
      selection: input.selection,
      label: commit === 'edit' ? 'flow attach' : '',
    };
  };

  const target = stockUnder(view, input.current);
  let mark: GesturePlan['target'];
  if (target !== undefined) {
    const terminal =
      endEl?.uid === target.uid
        ? stockTerminal(target, flow.points[endIndex], flow.points[adjacentIndex])
        : stockTerminal(target);
    const g = routeEnd(flow, end, terminal, {
      fixed,
      occupied: occupiedOn(view.elements, flowUid, stockUidsOf(terminal, fixed)),
      obstacles,
    });
    const distinct = !(fixed.kind === 'stock' && fixed.stock.uid === target.uid);
    const valid =
      distinct && stockVariableExists(input, target) && flowFault(g.flow, withEnd(terminal), stocks) === 'none';
    if (valid) {
      const removed = new Set<UID>();
      if (endEl?.type === 'cloud') {
        removed.add(endEl.uid);
        changed.delete(endEl.uid);
      }
      changed.set(flowUid, g.flow);
      return finish('edit', loose.nextUid, { uid: target.uid, valid: true }, removed);
    }
    mark = { uid: target.uid, valid: false };
  }

  const d = delta(input.press, input.current);
  const at = { x: flow.points[endIndex].x + d.x, y: flow.points[endIndex].y + d.y };
  let nextUid = loose.nextUid;
  const cloud = endEl?.type === 'cloud' ? endEl : newCloud(nextUid++, flowUid, at);
  const terminal = freeTerminal(at, cloud);
  const g = routeEnd(flow, end, terminal, {
    fixed,
    occupied: occupiedOn(view.elements, flowUid, stockUidsOf(fixed)),
    obstacles,
  });
  const p = endpointOf(g.flow.points, end);
  changed.set(flowUid, g.flow);
  changed.set(cloud.uid, { ...cloud, x: p.x, y: p.y });
  const valid = mark === undefined && flowFault(g.flow, withEnd(terminal), stocks) === 'none';
  return finish(valid ? 'edit' : 'none', nextUid, mark, new Set());
}

const NEW_FLOW_NAME = 'New Flow';

/**
 * Draw a new flow out of a stock (the route picks its face) or out of empty
 * space (a cloud at the press point), with its sink at the pointer: onto a
 * stock under the pointer when valid (a different stock from the source, whose
 * variable exists, with a route holding G2-G6), else a cloud at the pointer. A
 * valid drop selects the flow and hands off to its name editor; an invalid
 * target commits nothing (E6).
 */
export function planCreateFlow(input: PlanInput, from: { readonly stock: UID } | 'empty'): GesturePlan {
  const { view } = input;
  const base = byUidOf(view.elements);
  let nextUid = view.nextUid;
  const flowUid = nextUid++;
  const name = input.names(NEW_FLOW_NAME);
  const draft: FlowViewElement = {
    type: 'flow',
    uid: flowUid,
    var: undefined,
    name,
    ident: canonicalize(name),
    x: input.press.x,
    y: input.press.y,
    labelSide: 'bottom',
    points: [],
    isZeroRadius: false,
  };
  let source: Terminal;
  let sourceCloud: CloudViewElement | undefined;
  if (from === 'empty') {
    sourceCloud = newCloud(nextUid++, flowUid, input.press);
    source = freeTerminal(input.press, sourceCloud);
  } else {
    const stock = base.get(from.stock);
    if (stock?.type !== 'stock') {
      return idlePlan(input);
    }
    source = stockTerminal(stock);
  }
  const stocks = stockPositions(view.elements);
  const selection = new Set([flowUid]);
  const plan = (
    flow: FlowViewElement,
    sinkCloud: CloudViewElement | undefined,
    commit: 'edit' | 'none',
    target: GesturePlan['target'],
  ): GesturePlan => {
    const added = new Map<UID, ViewElement>([[flowUid, flow]]);
    const last = flow.points[flow.points.length - 1];
    if (sourceCloud !== undefined) {
      added.set(sourceCloud.uid, { ...sourceCloud, x: flow.points[0].x, y: flow.points[0].y });
    }
    if (sinkCloud !== undefined) {
      added.set(sinkCloud.uid, { ...sinkCloud, x: last.x, y: last.y });
    }
    return {
      elements: mergeElements(view.elements, added),
      nextUid,
      target,
      commit,
      selection,
      handoff: commit === 'edit' ? { editName: flowUid } : undefined,
      label: commit === 'edit' ? 'flow creation' : '',
    };
  };

  const target = stockUnder(view, input.current);
  let mark: GesturePlan['target'];
  if (target !== undefined) {
    const terminal = stockTerminal(target);
    const g = route(source, terminal, {
      flow: draft,
      occupied: occupiedOn(view.elements, flowUid, stockUidsOf(source, terminal)),
      obstacles: stocks,
    });
    const distinct = !(source.kind === 'stock' && source.stock.uid === target.uid);
    const valid =
      distinct &&
      stockVariableExists(input, target) &&
      flowFault(g.flow, { source, sink: terminal }, stocks) === 'none';
    if (valid) {
      return plan(g.flow, undefined, 'edit', { uid: target.uid, valid: true });
    }
    mark = { uid: target.uid, valid: false };
  }
  const sinkCloud = newCloud(nextUid++, flowUid, input.current);
  const sink = freeTerminal(input.current, sinkCloud);
  const g = route(source, sink, {
    flow: draft,
    occupied: occupiedOn(view.elements, flowUid, stockUidsOf(source)),
    obstacles: stocks,
  });
  const valid = mark === undefined && flowFault(g.flow, { source, sink }, stocks) === 'none';
  return plan(g.flow, sinkCloud, valid ? 'edit' : 'none', mark);
}
