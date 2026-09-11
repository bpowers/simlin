// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Fixtures for the flow-geometry tests. Views are built as engine JSON and
 * loaded through the production `modelFromJson`, so element fields (idents,
 * attachment uids, stock flow lists) are what the editor sees; terminals are
 * derived through the production `flowTerminals`. The helpers here only
 * assemble inputs and read outputs; every geometric expectation a test states
 * is computed with the independent arc helpers below, never with the core's.
 */

import type { JsonModel, JsonViewElement } from '@simlin/engine';
import {
  modelFromJson,
  type CloudViewElement,
  type FlowViewElement,
  type StockFlowView,
  type StockViewElement,
  type UID,
  type ViewElement,
} from '@simlin/core/datamodel';

import type { FlowGeometry } from '../../flow-geometry';
import { checkFlowInvariants, formatFlowViolations } from './flow-invariants';

export type Pt = { readonly x: number; readonly y: number };

export function loadView(elements: readonly JsonViewElement[]): StockFlowView {
  return modelFromJson({ name: 'main', views: [{ elements: [...elements] }] } as JsonModel).views[0];
}

export const stockJson = (uid: UID, x: number, y: number): JsonViewElement => ({
  type: 'stock',
  uid,
  name: `s${uid}`,
  x,
  y,
});

export const cloudJson = (uid: UID, flowUid: UID, x: number, y: number): JsonViewElement => ({
  type: 'cloud',
  uid,
  flowUid,
  x,
  y,
});

export function flowJson(
  uid: UID,
  valve: Pt,
  points: readonly Pt[],
  attachments: { readonly source?: UID; readonly sink?: UID },
): JsonViewElement {
  return {
    type: 'flow',
    uid,
    name: `f${uid}`,
    x: valve.x,
    y: valve.y,
    points: points.map((p, i) => {
      const attached = i === 0 ? attachments.source : i === points.length - 1 ? attachments.sink : undefined;
      return attached === undefined ? { x: p.x, y: p.y } : { x: p.x, y: p.y, attachedToUid: attached };
    }),
  };
}

export function byUidOf(view: StockFlowView): Map<UID, ViewElement> {
  return new Map(view.elements.map((e) => [e.uid, e]));
}

export function flowOf(view: StockFlowView, uid: UID): FlowViewElement {
  const el = view.elements.find((e) => e.uid === uid);
  if (el?.type !== 'flow') {
    throw new Error(`no flow ${uid}`);
  }
  return el;
}

export function stockOf(view: StockFlowView, uid: UID): StockViewElement {
  const el = view.elements.find((e) => e.uid === uid);
  if (el?.type !== 'stock') {
    throw new Error(`no stock ${uid}`);
  }
  return el;
}

export function cloudOf(view: StockFlowView, uid: UID): CloudViewElement {
  const el = view.elements.find((e) => e.uid === uid);
  if (el?.type !== 'cloud') {
    throw new Error(`no cloud ${uid}`);
  }
  return el;
}

/** The view with `changed` elements replaced (or added) and `removed` uids dropped. */
export function patchView(
  view: StockFlowView,
  changed: readonly ViewElement[],
  removed: readonly UID[] = [],
): StockFlowView {
  const m = byUidOf(view);
  for (const uid of removed) {
    m.delete(uid);
  }
  for (const el of changed) {
    m.set(el.uid, el);
  }
  return { ...view, elements: [...m.values()] };
}

/** The view a planner would render after applying `g` (plus any moved terminals). */
export function applyGeometry(
  view: StockFlowView,
  g: FlowGeometry,
  also: readonly ViewElement[] = [],
  removed: readonly UID[] = [],
): StockFlowView {
  return patchView(view, [...also, g.flow, ...g.clouds], removed);
}

/** Strict violations of the routed flows, formatted so a failure shows every arm and its numbers. */
export function strictReport(view: StockFlowView, routed: readonly UID[]): string {
  return formatFlowViolations(checkFlowInvariants(view, { mode: 'strict', routed: new Set(routed) }));
}

// ---------------------------------------------------------------------------
// Independent path geometry (never the core's)

export function pathLength(pts: readonly Pt[]): number {
  let total = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    total += Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
  }
  return total;
}

/** Arc-length position of the nearest point on the path. */
export function arcOf(pts: readonly Pt[], p: Pt): number {
  let best = Infinity;
  let position = 0;
  let traversed = 0;
  for (let i = 0; i < pts.length - 1; i++) {
    const a = pts[i];
    const b = pts[i + 1];
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const l2 = dx * dx + dy * dy;
    const t = l2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l2));
    const d = Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
    if (d < best - 1e-9) {
      best = d;
      position = traversed + t * Math.sqrt(l2);
    }
    traversed += Math.sqrt(l2);
  }
  return position;
}

export function distanceToPath(p: Pt, pts: readonly Pt[]): number {
  let best = Infinity;
  for (let i = 0; i < pts.length - 1; i++) {
    const a = pts[i];
    const b = pts[i + 1];
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const l2 = dx * dx + dy * dy;
    const t = l2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l2));
    best = Math.min(best, Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy)));
  }
  return best;
}

function sample(pts: readonly Pt[], step: number): Pt[] {
  const out: Pt[] = [pts[0]];
  for (let i = 0; i < pts.length - 1; i++) {
    const a = pts[i];
    const b = pts[i + 1];
    const n = Math.max(1, Math.ceil(Math.hypot(b.x - a.x, b.y - a.y) / step));
    for (let k = 1; k <= n; k++) {
      out.push({ x: a.x + ((b.x - a.x) * k) / n, y: a.y + ((b.y - a.y) * k) / n });
    }
  }
  return out;
}

/** Symmetric Hausdorff distance between two paths, sampled every `step` px. */
export function hausdorff(a: readonly Pt[], b: readonly Pt[], step = 1): number {
  let m = 0;
  for (const p of sample(a, step)) {
    m = Math.max(m, distanceToPath(p, b));
  }
  for (const p of sample(b, step)) {
    m = Math.max(m, distanceToPath(p, a));
  }
  return m;
}

/** The segment directions of a path as letters (R, L, D, U): its shape, independent of lengths. */
export function directions(pts: readonly Pt[]): string {
  return pts
    .slice(1)
    .map((p, i) => {
      const q = pts[i];
      if (Math.abs(p.y - q.y) <= 1e-6) {
        return p.x > q.x ? 'R' : 'L';
      }
      return p.y > q.y ? 'D' : 'U';
    })
    .join('');
}

export function fmtFlow(f: FlowViewElement): string {
  const r = (n: number): string => String(Math.round(n * 1000) / 1000);
  return `valve(${r(f.x)},${r(f.y)}) ${f.points.map((p) => `(${r(p.x)},${r(p.y)})${p.attachedToUid === undefined ? '' : `@${p.attachedToUid}`}`).join(' ')}`;
}
