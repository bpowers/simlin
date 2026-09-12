// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * E5: when a republished view invalidates a live gesture.
 */

import type { StockFlowView, UID, ViewElement } from '@simlin/core/datamodel';

import { GEOMETRY_EPSILON } from '../flow-geometry';

function near(a: number, b: number): boolean {
  return Math.abs(a - b) <= GEOMETRY_EPSILON || (Number.isNaN(a) && Number.isNaN(b));
}

function nearOptional(a: number | undefined, b: number | undefined): boolean {
  return a === undefined || b === undefined ? a === b : near(a, b);
}

/**
 * Whether two versions of an element agree on every field a gesture reads:
 * position, attachments and label side, compared within GEOMETRY_EPSILON.
 * Derived fields (`isStraight`, `var`, `ident`) are ignored, since an engine
 * round trip re-derives them, and so are names: a gesture copies names from the
 * view it renders, and a rename reaches the canvas as its own republish.
 */
function sameReadFields(a: ViewElement, b: ViewElement): boolean {
  if (a.type !== b.type) {
    return false;
  }
  // A link has no position of its own: the datamodel reads its x/y as NaN, and
  // nothing draws or routes through them, so only its ends and arc are read.
  if (a.type !== 'link' && (!near(a.x, b.x) || !near(a.y, b.y))) {
    return false;
  }
  switch (a.type) {
    case 'flow': {
      const bp = (b as typeof a).points;
      return (
        a.labelSide === (b as typeof a).labelSide &&
        a.points.length === bp.length &&
        a.points.every((p, i) => near(p.x, bp[i].x) && near(p.y, bp[i].y) && p.attachedToUid === bp[i].attachedToUid)
      );
    }
    case 'stock':
    case 'aux':
    case 'module':
      return a.labelSide === (b as typeof a).labelSide;
    case 'alias':
      return a.aliasOfUid === (b as typeof a).aliasOfUid && a.labelSide === (b as typeof a).labelSide;
    case 'cloud':
      return a.flowUid === (b as typeof a).flowUid;
    case 'link': {
      const l = b as typeof a;
      return a.fromUid === l.fromUid && a.toUid === l.toUid && nearOptional(a.arc, l.arc);
    }
    case 'group': {
      const g = b as typeof a;
      return near(a.width, g.width) && near(a.height, g.height);
    }
  }
}

/**
 * Whether `current` still agrees with the view a gesture captured at press on
 * every element's read fields (see `sameReadFields`), with the same set of
 * elements. The comparison covers the whole view: a flow endpoint's targets are
 * every stock, a link's every named element, and a move routes through any
 * attachment, so every element is read by some gesture that can be live.
 * Republishes that change nothing geometric (sim results, error annotations, a
 * round trip one ULP away, the press's own selection change) keep the gesture.
 */
export function sameGeometry(base: StockFlowView, current: StockFlowView): boolean {
  if (base === current) {
    return true;
  }
  if (base.elements.length !== current.elements.length) {
    return false;
  }
  const byUid = new Map<UID, ViewElement>(current.elements.map((el) => [el.uid, el]));
  if (byUid.size !== current.elements.length) {
    return false;
  }
  return base.elements.every((el) => {
    const other = byUid.get(el.uid);
    return other !== undefined && sameReadFields(el, other);
  });
}
