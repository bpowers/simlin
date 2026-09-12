// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Seeded property tests of planGesture over generated scenes.
//
// Every seed builds a strict scene (genScene) or an imported one
// (genImportedScene) and derives gestures from its elements the way the Canvas
// supplies them: a press on an element's center, endpoint, valve, segment or
// label, the selection that press produces, and a pointer walk of eight frames
// that starts within the click threshold and sometimes ends over a stock or a
// named element (a drop target). Every GESTURE_KINDS kind is derived for every
// scene that has a subject for it.
//
// Asserted on every frame: planGesture never throws and plans finite
// coordinates; E1 (within the threshold nothing previews or commits, except a
// creation tool's draft and a label drag, which starts past the label's own
// threshold); E6 (an invalid target never commits); E4 locality (a plan changes
// only its subject, the flows its moving terminals route, their clouds, and the
// links whose endpoints changed; it removes only its own flow's cloud); and on
// committed frames the strict flow invariants over the flows the plan routed
// (a flow both of whose terminals moved is translated, not routed), M3, and a
// buildEditOps that accepts the planned view (no name collision). Across the
// run, every gesture kind that can edit commits at least once. Per scene: E5's
// no-abort half, a JSON round trip of the view keeps sameGeometry.
//
// What this does not establish: M1/M2 after the engine applies the edit
// (editor-gestures-engine.test.ts), continuity between frames (the geometry
// core's sweeps), or the Canvas using these plans for preview and commit.

import { describe, it, expect } from '@rstest/core';

import {
  isNamedViewElement,
  stockFlowViewFromJson,
  stockFlowViewToJson,
  type FlowViewElement,
  type UID,
} from '@simlin/core/datamodel';

import { beyondThreshold, planGesture, sameGeometry, type Gesture, type GesturePlan } from '../gesture-planner';
import { GESTURE_KINDS } from '../gesture-planner/types';
import { buildEditOps } from '../view-model-sync';
import { checkFlowInvariants, formatFlowViolations } from './support/flow-invariants';
import { namesOf, planned, planInput, type Pt, type Scene } from './support/gesture-fixtures';
import { genImportedScene, genScene, Rng } from './support/scene-generator';
import { checkReferentialIntegrity, formatViewViolations } from './support/view-invariants';

const STRICT_SEEDS = 30;
const IMPORTED_SEEDS = 30;
const FRAMES = 8;

interface Case {
  readonly gesture: Gesture;
  readonly selection: ReadonlySet<UID>;
  readonly press: Pt;
  /** Drop targets a walk may end on. */
  readonly targets: readonly Pt[];
}

function cases(rng: Rng, s: Scene): Case[] {
  const els = s.view.elements;
  const stocks = els.filter((e) => e.type === 'stock');
  const flows = els.filter((e): e is FlowViewElement => e.type === 'flow' && e.points.length >= 2);
  const links = els.filter((e) => e.type === 'link');
  const named = els.filter((e) => isNamedViewElement(e) || e.type === 'alias');
  const positioned = els.filter((e) => ['stock', 'aux', 'module', 'alias', 'cloud'].includes(e.type));
  const linkTargets = els.filter((e) => e.type === 'aux' || e.type === 'flow' || e.type === 'module');
  const stockTargets = stocks.map((e) => ({ x: e.x, y: e.y }));
  const anywhere = (): Pt => ({ x: rng.int(0, 900), y: rng.int(0, 900) });
  const out: Case[] = [];
  const add = (gesture: Gesture, selection: readonly UID[], press: Pt, targets: readonly Pt[] = []): void => {
    out.push({ gesture, selection: new Set(selection), press: { x: press.x, y: press.y }, targets });
  };
  if (stocks.length > 0) {
    const st = rng.pick(stocks);
    add({ kind: 'moveSelection' }, [st.uid], st, stockTargets);
    const group = positioned.filter(() => rng.bool(0.4));
    if (group.length > 0) {
      add(
        { kind: 'moveSelection' },
        group.map((e) => e.uid),
        group[0],
      );
    }
  }
  for (const f of flows) {
    add({ kind: 'slideValve', flow: f.uid }, [f.uid], f);
    const i = rng.int(0, f.points.length - 2);
    const a = f.points[i];
    const b = f.points[i + 1];
    add({ kind: 'offsetSegment', flow: f.uid, segmentIndex: i }, [f.uid], { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
    add({ kind: 'flowEndpoint', flow: f.uid, end: 'source' }, [f.uid], f.points[0], stockTargets);
    add({ kind: 'flowEndpoint', flow: f.uid, end: 'sink' }, [f.uid], f.points[f.points.length - 1], stockTargets);
  }
  if (stocks.length > 0) {
    const st = rng.pick(stocks);
    add({ kind: 'createFlow', from: { stock: st.uid } }, [], st, stockTargets);
  }
  add({ kind: 'createFlow', from: 'empty' }, [], anywhere(), stockTargets);
  const linkTargetPts = linkTargets.map((e) => ({ x: e.x, y: e.y }));
  if (named.length > 0) {
    const from = rng.pick(named);
    add({ kind: 'createLink', from: from.uid }, [], from, linkTargetPts);
    const labelled = rng.pick(named);
    add({ kind: 'label', uid: labelled.uid }, [labelled.uid], { x: labelled.x + 30, y: labelled.y });
  }
  for (const l of links) {
    const to = els.find((e) => e.uid === l.toUid);
    const from = els.find((e) => e.uid === l.fromUid);
    if (to !== undefined && from !== undefined) {
      add({ kind: 'linkEndpoint', link: l.uid }, [l.uid], to, linkTargetPts);
      add({ kind: 'linkArc', link: l.uid }, [l.uid], { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 });
    }
  }
  add({ kind: 'createElement', type: rng.pick(['aux', 'stock', 'module'] as const) }, [], anywhere());
  add({ kind: 'rubberBand' }, [], anywhere());
  add({ kind: 'pan' }, [], anywhere());
  return out;
}

function walk(rng: Rng, c: Case): Pt[] {
  const frames: Pt[] = [{ x: c.press.x + rng.float(-2, 2), y: c.press.y + rng.float(-2, 2) }];
  let at = frames[0];
  for (let i = 1; i < FRAMES; i++) {
    at = { x: at.x + rng.float(-45, 45), y: at.y + rng.float(-45, 45) };
    frames.push(at);
  }
  if (c.targets.length > 0 && rng.bool(0.6)) {
    const t = rng.pick(c.targets);
    frames[FRAMES - 1] = { x: t.x + rng.float(-3, 3), y: t.y + rng.float(-3, 3) };
  }
  return frames;
}

// Links carry no position of their own (their x/y are unset in the JSON), so
// only positioned elements and flow paths are checked.
function finite(plan: GesturePlan): boolean {
  return plan.elements.every(
    (el) =>
      el.type === 'link' ||
      (Number.isFinite(el.x) &&
        Number.isFinite(el.y) &&
        (el.type !== 'flow' || el.points.every((p) => Number.isFinite(p.x) && Number.isFinite(p.y)))),
  );
}

function subjects(c: Case): Set<UID> {
  const out = new Set(c.selection);
  const g = c.gesture;
  switch (g.kind) {
    case 'slideValve':
    case 'offsetSegment':
    case 'flowEndpoint':
      out.add(g.flow);
      break;
    case 'linkEndpoint':
    case 'linkArc':
      out.add(g.link);
      break;
    case 'label':
      out.add(g.uid);
      break;
    default:
      break;
  }
  return out;
}

/**
 * E4, derived from the gesture rather than from the planner: the elements a plan
 * may change, and remove.
 */
function localityViolations(s: Scene, c: Case, plan: GesturePlan): string[] {
  const base = new Map(s.view.elements.map((el) => [el.uid, el]));
  const next = new Map(plan.elements.map((el) => [el.uid, el]));
  const changed = new Set<UID>();
  for (const el of plan.elements) {
    if (base.get(el.uid) !== el) {
      changed.add(el.uid);
    }
  }
  const subject = subjects(c);
  const moving = c.gesture.kind === 'moveSelection' ? c.selection : new Set<UID>();
  const out: string[] = [];
  for (const uid of changed) {
    const el = next.get(uid)!;
    const created = !base.has(uid);
    if (created || subject.has(uid)) {
      continue;
    }
    if (el.type === 'flow') {
      const ends = [el.points[0]?.attachedToUid, el.points[el.points.length - 1]?.attachedToUid];
      if (ends.some((u) => u !== undefined && moving.has(u))) {
        continue;
      }
    }
    if (el.type === 'cloud' && (changed.has(el.flowUid) || subject.has(el.flowUid))) {
      continue;
    }
    if (el.type === 'link' && (changed.has(el.fromUid) || changed.has(el.toUid))) {
      continue;
    }
    out.push(`changed ${el.type} ${uid}`);
  }
  for (const [uid, el] of base) {
    if (!next.has(uid) && !(el.type === 'cloud' && subject.has(el.flowUid))) {
      out.push(`removed ${el.type} ${uid}`);
    }
  }
  return out;
}

/** The flows a committed plan routed: changed flows that are not a plain translation of their base. */
function routed(s: Scene, plan: GesturePlan, d: Pt): Set<UID> {
  const base = new Map(s.view.elements.map((el) => [el.uid, el]));
  const out = new Set<UID>();
  for (const el of plan.elements) {
    const b = base.get(el.uid);
    if (el.type !== 'flow' || b === el) {
      continue;
    }
    const shifted = (by: Pt): boolean =>
      b?.type === 'flow' &&
      b.points.length === el.points.length &&
      b.points.every((p, i) => p.x + by.x === el.points[i].x && p.y + by.y === el.points[i].y);
    // Translated with both terminals, or only relabeled: not routed.
    const relabeled = shifted({ x: 0, y: 0 }) && b?.x === el.x && b?.y === el.y;
    if (!shifted(d) && !relabeled) {
      out.add(el.uid);
    }
  }
  return out;
}

interface Tally {
  readonly commits: Map<Gesture['kind'], number>;
  readonly failures: string[];
}

function run(s: Scene, rng: Rng, label: string, tally: Tally): void {
  const names = namesOf(s);
  for (const c of cases(rng, s)) {
    for (const current of walk(rng, c)) {
      const where = `${label} ${JSON.stringify(c.gesture)} sel=${JSON.stringify([...c.selection])} press=${JSON.stringify(c.press)} current=${JSON.stringify(current)}`;
      let plan: GesturePlan;
      try {
        plan = planGesture(planInput(s, c.gesture, c.press, current, { selection: c.selection, names }));
      } catch (err) {
        tally.failures.push(`${where}: threw ${String(err)}`);
        continue;
      }
      if (!finite(plan)) {
        tally.failures.push(`${where}: non-finite coordinate`);
      }
      const exempt = c.gesture.kind === 'createElement' || c.gesture.kind === 'label';
      if (
        !beyondThreshold(c.press, current, 1) &&
        !exempt &&
        (plan.elements !== s.view.elements || plan.commit === 'edit')
      ) {
        tally.failures.push(`${where}: E1 a click changed the view`);
      }
      if (plan.target?.valid === false && plan.commit !== 'none') {
        tally.failures.push(`${where}: E6 an invalid target committed`);
      }
      if (plan.commit !== 'edit') {
        continue;
      }
      tally.commits.set(c.gesture.kind, (tally.commits.get(c.gesture.kind) ?? 0) + 1);
      const view = planned(s, plan);
      const d = { x: current.x - c.press.x, y: current.y - c.press.y };
      const flows = checkFlowInvariants(view, { mode: 'strict', routed: routed(s, plan, d) });
      const refs = checkReferentialIntegrity(view);
      if (flows.length > 0 || refs.length > 0) {
        tally.failures.push(`${where}:\n${formatFlowViolations(flows)}${formatViewViolations(refs)}`);
      }
      const locality = localityViolations(s, c, plan);
      if (locality.length > 0) {
        tally.failures.push(`${where}: E4 ${locality.join(', ')}`);
      }
      try {
        buildEditOps(s.model, s.view, view);
      } catch (err) {
        tally.failures.push(`${where}: buildEditOps refused the planned view: ${String(err)}`);
      }
    }
  }
}

describe('planGesture over generated scenes', () => {
  const tally: Tally = { commits: new Map(), failures: [] };

  it(`strict scenes (${STRICT_SEEDS} seeds): totality, E1, E4, E6 and committed invariants`, () => {
    for (let seed = 1; seed <= STRICT_SEEDS; seed++) {
      const rng = new Rng(seed);
      run(genScene(rng), rng, `strict seed ${seed}`, tally);
    }
    expect(tally.failures.slice(0, 5).join('\n\n')).toBe('');
  });

  it(`imported scenes (${IMPORTED_SEEDS} seeds): totality, E1, E4, E6 and committed invariants`, () => {
    for (let seed = 1; seed <= IMPORTED_SEEDS; seed++) {
      const rng = new Rng(1000 + seed);
      run(genImportedScene(rng), rng, `imported seed ${seed}`, tally);
    }
    expect(tally.failures.slice(0, 5).join('\n\n')).toBe('');
  });

  it('every gesture kind that edits committed at least once', () => {
    const editing = GESTURE_KINDS.filter((k) => k !== 'createElement' && k !== 'rubberBand' && k !== 'pan');
    expect(editing.filter((k) => (tally.commits.get(k) ?? 0) === 0)).toEqual([]);
  });

  it('E5: a JSON round trip of a generated view keeps a live gesture', () => {
    for (let seed = 1; seed <= 10; seed++) {
      const s = genImportedScene(new Rng(seed));
      const json = JSON.parse(JSON.stringify(stockFlowViewToJson(s.view)));
      expect(sameGeometry(s.view, stockFlowViewFromJson(json, s.model.variables))).toBe(true);
    }
  });
});
