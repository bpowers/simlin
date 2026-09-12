// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * classifyPress: what a press starts. The Canvas hit-tests the DOM event into a
 * `PressHit` and tracks its pointers; everything that decides which gesture
 * follows, which selection the press applies at once and which a click settles
 * on, and whether an armed tool is cleared, lives here.
 */

import type { StockFlowView, UID } from '@simlin/core/datamodel';

import { segmentHold, type XY } from '../flow-geometry';
import { beyondThreshold, delta, isLinkSource, segmentNearest } from './common';
import type { PressGesture, Tool } from './types';

export type PressHit =
  /** The empty canvas. */
  | { readonly kind: 'canvas' }
  /** An element's body, a flow or link arrowhead, or a flow's source grip. */
  | { readonly kind: 'element'; readonly uid: UID; readonly part: 'body' | 'arrowhead' | 'source' }
  | { readonly kind: 'labelDoubleClick'; readonly uid: UID }
  /** A label dragged past its own click threshold (the label component owns that threshold). */
  | { readonly kind: 'labelDrag'; readonly uid: UID }
  | { readonly kind: 'moduleDoubleClick'; readonly uid: UID }
  /** The overlay behind an open name editor. */
  | { readonly kind: 'nameEditor' };

export interface PressInput {
  readonly view: StockFlowView;
  readonly selection: ReadonlySet<UID>;
  readonly tool: Tool | undefined;
  readonly hit: PressHit;
  /** The press in model coordinates. */
  readonly point: XY;
  readonly shiftKey: boolean;
  /** Ctrl or Meta. */
  readonly toggleKey: boolean;
  readonly pointerType: string;
  readonly readOnly: boolean;
  /** The host has an undo or redo queued: a gesture planned on the view it replaces could not commit. */
  readonly pressesDisabled: boolean;
  /** Pointers down, this press included. */
  readonly pointers: number;
  /** Another pointer owns a live gesture. */
  readonly gestureLive: boolean;
}

export type PressOutcome =
  | { readonly kind: 'ignore' }
  /** A second touch: abort any live gesture (E5) and start a pinch. */
  | { readonly kind: 'pinch' }
  /** A second pointer: abort the live gesture without committing (E5). */
  | { readonly kind: 'abort' }
  /** Commit the open name editor. */
  | { readonly kind: 'commitName' }
  | { readonly kind: 'drill'; readonly uid: UID }
  | {
      readonly kind: 'editName';
      readonly uid: UID;
      readonly selection: ReadonlySet<UID>;
      readonly clearTool: boolean;
    }
  /** A selection change and nothing else: no drag follows. */
  | { readonly kind: 'select'; readonly selection: ReadonlySet<UID>; readonly clearTool: boolean }
  | {
      readonly kind: 'start';
      readonly gesture: PressGesture;
      /** The selection applied at press, or undefined to leave it (a deferred single select). */
      readonly selection: ReadonlySet<UID> | undefined;
      /** The selection a release within the click threshold settles on; undefined keeps the press's. */
      readonly clickSelection: ReadonlySet<UID> | undefined;
      readonly clearTool: boolean;
    };

const EMPTY: ReadonlySet<UID> = new Set();

function start(
  gesture: PressGesture,
  selection: ReadonlySet<UID> | undefined,
  clickSelection: ReadonlySet<UID> | undefined,
  clearTool: boolean,
): PressOutcome {
  return { kind: 'start', gesture, selection, clickSelection, clearTool };
}

/**
 * The press table. In order:
 *
 * - the name editor's overlay commits the name (before anything else, so a
 *   typed name is never lost to a disabled or multi-touch press);
 * - presses are ignored while the host has an undo or redo queued;
 * - a second touch starts a pinch (a third is ignored), and any other second
 *   pointer aborts the live gesture;
 * - a label double-click opens the name editor (read-only: selects only);
 * - a label drag moves the label;
 * - on the empty canvas: an aux/stock/module tool stages a draft element, the
 *   flow tool draws a flow out of empty space, touch or Shift pans, and anything
 *   else rubber-bands (a click clears the selection). The link tool on the empty
 *   canvas starts nothing of its own and stays armed;
 * - on an element: the link tool on a named element or an alias draws a link,
 *   the flow tool on a stock draws a flow; any other armed tool is cleared and
 *   the press behaves as with no tool;
 * - a flow's arrowhead or source grip drags that end, a link's arrowhead drags
 *   the link's end, each selecting just the flow or link;
 * - a modifier press toggles the element: toggling it out starts nothing,
 *   toggling it in moves the new selection;
 * - a cloud that is unselected or the sole selection drags its flow's end; a
 *   cloud in a multi-element selection moves with the selection;
 * - a press on a selected element defers collapsing the selection to it until a
 *   click, so a drag moves the whole selection; an unselected element is
 *   selected at once;
 * - when the element ends up the sole selection, a link's body adjusts its arc
 *   and a flow's pipe or valve waits for the first move to latch slide or offset.
 */
export function classifyPress(input: PressInput): PressOutcome {
  const { hit, tool } = input;
  if (hit.kind === 'nameEditor') {
    return { kind: 'commitName' };
  }
  if (hit.kind === 'moduleDoubleClick') {
    return { kind: 'drill', uid: hit.uid };
  }
  if (input.pressesDisabled) {
    return { kind: 'ignore' };
  }
  if (input.pointers > 2) {
    return { kind: 'ignore' };
  }
  if (input.pointers === 2) {
    return input.pointerType === 'touch' ? { kind: 'pinch' } : { kind: 'abort' };
  }
  if (input.gestureLive) {
    return { kind: 'abort' };
  }
  const clearTool = tool !== undefined;
  switch (hit.kind) {
    case 'labelDoubleClick': {
      const only = new Set([hit.uid]);
      return input.readOnly
        ? { kind: 'select', selection: only, clearTool }
        : { kind: 'editName', uid: hit.uid, selection: only, clearTool };
    }
    case 'labelDrag':
      return start({ kind: 'label', uid: hit.uid }, new Set([hit.uid]), undefined, clearTool);
    case 'canvas':
      if (tool === 'aux' || tool === 'stock' || tool === 'module') {
        return start({ kind: 'createElement', type: tool }, EMPTY, undefined, false);
      }
      if (tool === 'flow') {
        return start({ kind: 'createFlow', from: 'empty' }, undefined, undefined, false);
      }
      if (input.pointerType === 'touch' || input.shiftKey) {
        return start({ kind: 'pan' }, undefined, undefined, false);
      }
      return start({ kind: 'rubberBand' }, undefined, EMPTY, false);
    case 'element':
      return classifyElementPress(input, hit.uid, hit.part);
  }
}

function classifyElementPress(input: PressInput, uid: UID, part: 'body' | 'arrowhead' | 'source'): PressOutcome {
  const { view, tool, selection } = input;
  const el = view.elements.find((e) => e.uid === uid);
  if (el === undefined) {
    return { kind: 'ignore' };
  }
  if (part === 'body' && tool === 'link' && isLinkSource(el)) {
    return start({ kind: 'createLink', from: uid }, undefined, undefined, false);
  }
  if (part === 'body' && tool === 'flow' && el.type === 'stock') {
    return start({ kind: 'createFlow', from: { stock: uid } }, undefined, undefined, false);
  }
  const clearTool = tool !== undefined;
  const only = new Set([uid]);
  if (el.type === 'flow' && part !== 'body') {
    return start(
      { kind: 'flowEndpoint', flow: uid, end: part === 'source' ? 'source' : 'sink' },
      only,
      undefined,
      clearTool,
    );
  }
  if (el.type === 'link' && part === 'arrowhead') {
    return start({ kind: 'linkEndpoint', link: uid }, only, undefined, clearTool);
  }
  const selected = selection.has(uid);
  if (input.shiftKey || input.toggleKey) {
    if (selected) {
      return { kind: 'select', selection: new Set([...selection].filter((u) => u !== uid)), clearTool };
    }
    const added = new Set([...selection, uid]);
    return start({ kind: 'moveSelection' }, added, undefined, clearTool);
  }
  if (el.type === 'cloud' && !(selected && selection.size > 1)) {
    const flow = view.elements.find((e) => e.uid === el.flowUid);
    if (flow?.type === 'flow' && flow.points.length >= 2) {
      const end =
        flow.points[0].attachedToUid === uid
          ? 'source'
          : flow.points[flow.points.length - 1].attachedToUid === uid
            ? 'sink'
            : undefined;
      if (end !== undefined) {
        return start({ kind: 'flowEndpoint', flow: flow.uid, end }, new Set([flow.uid]), undefined, clearTool);
      }
    }
  }
  const effective = selected ? selection : only;
  let gesture: PressGesture = { kind: 'moveSelection' };
  if (effective.size === 1 && el.type === 'link') {
    gesture = { kind: 'linkArc', link: uid };
  } else if (effective.size === 1 && el.type === 'flow' && el.points.length >= 2) {
    gesture = { kind: 'pipe', flow: uid, segmentIndex: segmentNearest(el.points, input.point) };
  }
  return start(gesture, selected ? undefined : only, only, clearTool);
}

/**
 * Latch a pipe press once the pointer passes the click threshold: a
 * perpendicular-dominant first move offsets the pressed segment, anything else
 * slides the valve. Every other gesture is returned unchanged, as is a pipe
 * press still within the threshold.
 */
export function latchGesture(
  gesture: PressGesture,
  input: { readonly view: StockFlowView; readonly press: XY; readonly current: XY; readonly zoom: number },
): PressGesture {
  if (gesture.kind !== 'pipe' || !beyondThreshold(input.press, input.current, input.zoom)) {
    return gesture;
  }
  const flow = input.view.elements.find((e) => e.uid === gesture.flow);
  if (flow?.type !== 'flow' || gesture.segmentIndex < 0 || gesture.segmentIndex >= flow.points.length - 1) {
    return { kind: 'slideValve', flow: gesture.flow };
  }
  const { axis } = segmentHold(flow.points, gesture.segmentIndex);
  const d = delta(input.press, input.current);
  const along = Math.abs(axis === 'x' ? d.x : d.y);
  const across = Math.abs(axis === 'x' ? d.y : d.x);
  return across > along
    ? { kind: 'offsetSegment', flow: gesture.flow, segmentIndex: gesture.segmentIndex }
    : { kind: 'slideValve', flow: gesture.flow };
}

/**
 * A mouse move that reports no button held belongs to a release the canvas
 * never saw (it landed outside the window): the gesture is cancelled, not
 * committed.
 */
export function isLostRelease(pointerType: string, buttons: number): boolean {
  return pointerType === 'mouse' && buttons === 0;
}
