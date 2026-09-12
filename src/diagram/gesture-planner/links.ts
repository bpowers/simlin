// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Link gestures: drawing a link, dragging a link's arrowhead onto another
 * element, and curving a link by dragging its body.
 */

import type { AuxViewElement, LinkViewElement, UID, ViewElement } from '@simlin/core/datamodel';

import { radToDeg } from '../arc-utils';
import { computeLinkCreationArc, takeoffθ } from '../drawing/Connector';
import { fauxTargetUid } from '../drawing/creation-sentinels';
import type { XY } from '../flow-geometry';
import { byUidOf, idlePlan, isLinkSource, linkTargetUnder, mergeElements } from './common';
import type { GesturePlan, PlanInput } from './types';

/**
 * The element a dragged link's arrowhead is over, and whether the link may end
 * there: never on its own source (that is no target at all, so the drop
 * aborts), and never duplicating another link between the same two elements
 * (a red target).
 */
function linkTarget(
  input: PlanInput,
  fromUid: UID,
  linkUid: UID | undefined,
): { readonly el: ViewElement; readonly valid: boolean } | undefined {
  const el = linkTargetUnder(input.view, input.current);
  if (el === undefined || el.uid === fromUid) {
    return undefined;
  }
  const duplicate = input.view.elements.some(
    (e) => e.type === 'link' && e.uid !== linkUid && e.fromUid === fromUid && e.toUid === el.uid,
  );
  return { el, valid: !duplicate };
}

/**
 * The zero-radius stand-in a link's preview points at while its arrowhead is
 * over no valid target. It exists only in preview frames, which never commit.
 */
function fauxTarget(at: XY): AuxViewElement {
  return {
    type: 'aux',
    uid: fauxTargetUid,
    var: undefined,
    name: '',
    ident: '',
    x: at.x,
    y: at.y,
    labelSide: 'right',
    isZeroRadius: true,
  };
}

/** A link created or reattached with a mouse curves through the pointer; touch links are always straight. */
function arcThrough(input: PlanInput, from: ViewElement, to: ViewElement): number | undefined {
  return input.pointerType === 'touch' ? undefined : computeLinkCreationArc(from, to, input.current);
}

/**
 * Draw a link from `fromUid`. Over a valid target it commits a link curving
 * through the pointer and selects it; anywhere else the preview draws a straight
 * link to the pointer and the drop commits nothing.
 */
export function planCreateLink(input: PlanInput, fromUid: UID): GesturePlan {
  const { view } = input;
  const from = view.elements.find((e) => e.uid === fromUid);
  if (from === undefined || !isLinkSource(from)) {
    return idlePlan(input);
  }
  const linkUid = view.nextUid;
  const t = linkTarget(input, fromUid, undefined);
  const link: LinkViewElement = {
    type: 'link',
    uid: linkUid,
    fromUid,
    toUid: t?.valid ? t.el.uid : fauxTargetUid,
    arc: t?.valid ? arcThrough(input, from, t.el) : undefined,
    multiPoint: undefined,
    isStraight: false,
    polarity: undefined,
    x: 0,
    y: 0,
    isZeroRadius: false,
    ident: undefined,
  };
  const selection = new Set([linkUid]);
  if (t?.valid) {
    return {
      elements: [...view.elements, link],
      nextUid: linkUid + 1,
      target: { uid: t.el.uid, valid: true },
      commit: 'edit',
      selection,
      label: 'link creation',
    };
  }
  return {
    elements: [...view.elements, fauxTarget(input.current), link],
    nextUid: view.nextUid,
    target: t === undefined ? undefined : { uid: t.el.uid, valid: false },
    commit: 'none',
    selection,
    label: '',
  };
}

/**
 * Drag an existing link's arrowhead. Over a valid target the link ends there,
 * curving through the pointer. Over its own source, an invalid target or empty
 * space the preview draws it straight to the pointer and the drop commits
 * nothing: dropping a link never deletes it.
 */
export function planLinkEndpoint(input: PlanInput, linkUid: UID): GesturePlan {
  const { view } = input;
  const base = byUidOf(view.elements);
  const link = base.get(linkUid);
  const from = link?.type === 'link' ? base.get(link.fromUid) : undefined;
  if (link?.type !== 'link' || from === undefined) {
    return idlePlan(input);
  }
  const t = linkTarget(input, link.fromUid, linkUid);
  if (t?.valid) {
    const next = { ...link, toUid: t.el.uid, arc: arcThrough(input, from, t.el) };
    return {
      elements: mergeElements(view.elements, new Map([[linkUid, next]])),
      nextUid: view.nextUid,
      target: { uid: t.el.uid, valid: true },
      commit: 'edit',
      selection: input.selection,
      label: 'link attach',
    };
  }
  const preview = { ...link, toUid: fauxTargetUid, arc: undefined };
  return {
    elements: [...mergeElements(view.elements, new Map([[linkUid, preview]])), fauxTarget(input.current)],
    nextUid: view.nextUid,
    target: t === undefined ? undefined : { uid: t.el.uid, valid: false },
    commit: 'none',
    selection: input.selection,
    label: '',
  };
}

/** Curve a sole selected link through the pointer by dragging its body. */
export function planLinkArc(input: PlanInput, linkUid: UID): GesturePlan {
  const base = byUidOf(input.view.elements);
  const link = base.get(linkUid);
  const from = link?.type === 'link' ? base.get(link.fromUid) : undefined;
  const to = link?.type === 'link' ? base.get(link.toUid) : undefined;
  if (link?.type !== 'link' || from === undefined || to === undefined) {
    return idlePlan(input);
  }
  const arc = radToDeg(takeoffθ({ element: link, from, to, arcPoint: input.current }));
  if (!Number.isFinite(arc)) {
    return idlePlan(input);
  }
  return {
    elements: mergeElements(input.view.elements, new Map([[linkUid, { ...link, arc }]])),
    nextUid: input.view.nextUid,
    commit: 'edit',
    selection: input.selection,
    label: 'link arc',
  };
}
