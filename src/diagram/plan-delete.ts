// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// The view a delete produces. Model ops are not built here: the controller
// derives them from (base view, next view) through buildEditOps, so deleting a
// variable, detaching flows from a deleted stock, and removing aliases all
// reach the engine through the same diff every other edit uses.

import type { CloudViewElement, StockFlowView, UID, ViewElement } from '@simlin/core/datamodel';

/**
 * Remove `selection` from `view`:
 *
 * - every selected element except a cloud whose flow is not also removed (a
 *   cloud is a flow endpoint, not a variable; deleting it alone would leave the
 *   flow's end dangling, so the request is ignored);
 * - the clouds of removed flows;
 * - the aliases of removed elements;
 * - every link touching a removed element (an alias or cloud included);
 * - and every endpoint of a surviving flow that was attached to a removed stock
 *   becomes a new cloud at that endpoint.
 *
 * Uids that survive are unchanged; new clouds take uids from `nextUid`.
 */
export function planDelete(view: StockFlowView, selection: ReadonlySet<UID>): StockFlowView {
  const removed = new Set<UID>();
  for (const el of view.elements) {
    if (selection.has(el.uid) && el.type !== 'cloud') {
      removed.add(el.uid);
    }
  }
  for (const el of view.elements) {
    if (el.type === 'cloud' && removed.has(el.flowUid)) {
      removed.add(el.uid);
    }
  }
  for (const el of view.elements) {
    if (el.type === 'alias' && removed.has(el.aliasOfUid)) {
      removed.add(el.uid);
    }
  }
  for (const el of view.elements) {
    if (el.type === 'link' && (removed.has(el.fromUid) || removed.has(el.toUid))) {
      removed.add(el.uid);
    }
  }

  let nextUid = view.nextUid;
  const clouds: CloudViewElement[] = [];
  const elements: ViewElement[] = [];
  for (const el of view.elements) {
    if (removed.has(el.uid)) {
      continue;
    }
    if (el.type !== 'flow') {
      elements.push(el);
      continue;
    }
    const points = el.points.map((pt) => {
      if (pt.attachedToUid === undefined || !removed.has(pt.attachedToUid)) {
        return pt;
      }
      const cloud: CloudViewElement = {
        type: 'cloud',
        uid: nextUid++,
        x: pt.x,
        y: pt.y,
        flowUid: el.uid,
        isZeroRadius: false,
        ident: undefined,
      };
      clouds.push(cloud);
      return { ...pt, attachedToUid: cloud.uid };
    });
    elements.push({ ...el, points });
  }

  return { ...view, elements: [...elements, ...clouds], nextUid };
}
