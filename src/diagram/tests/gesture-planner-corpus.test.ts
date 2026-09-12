// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Every flow of a handful of imported corpus models, dragged through each flow
// gesture, through the real WASM engine's import (engine import -> serializeJson
// -> projectFromJson, the editor's load path).
//
// What this establishes: for these models (Stella, Vensim via xmutil, native
// MDL including unattached fallback flows, Simlin-authored XMILE), planGesture
// never throws on an imported flow, every committed plan's routed flows hold the
// strict invariants (the imported violations are healed by routing), and each
// gesture commits on some flow of each model. What it does not establish: the
// other ~480 corpus models, or anything about the engine applying these edits.

import { describe, it, expect, beforeAll } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import type { FlowViewElement, Model, UID } from '@simlin/core/datamodel';

import { planGesture, type Gesture, type GesturePlan } from '../gesture-planner';
import { describeWithEngine, editorModel, loadEngine, type EngineModule } from './support/engine';
import { checkFlowInvariants, formatFlowViolations } from './support/flow-invariants';
import { namesOf, planned, planInput, routedFlows, type Pt, type Scene } from './support/gesture-fixtures';
import { checkReferentialIntegrity, formatViewViolations } from './support/view-invariants';

const repoRoot = path.join(__dirname, '..', '..', '..');

const CORPUS: readonly string[] = [
  'test/test-models/samples/teacup/teacup.stmx',
  'test/land_model/land_model.stmx',
  'test/test-models/tests/abs/test_abs.xmile',
  'test/test-models/samples/Roessler_Chaos/roessler_chaos.mdl',
  'test/test-models/tests/subscript_mapping_simple/test_subscript_mapping_simple.mdl',
  'test/cross_element_ltm/cross_element.stmx',
];

type FlowGestureKind = 'flowEndpoint' | 'slideValve' | 'offsetSegment' | 'moveTerminal';
const FLOW_GESTURES: readonly FlowGestureKind[] = ['flowEndpoint', 'slideValve', 'offsetSegment', 'moveTerminal'];

function gesturesFor(
  f: FlowViewElement,
): Array<{ kind: FlowGestureKind; gesture: Gesture; selection: UID[]; press: Pt; current: Pt }> {
  const src = f.points[0];
  const sink = f.points[f.points.length - 1];
  const a = f.points[0];
  const b = f.points[1];
  const horizontal = Math.abs(b.y - a.y) <= Math.abs(b.x - a.x);
  const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
  const out: Array<{ kind: FlowGestureKind; gesture: Gesture; selection: UID[]; press: Pt; current: Pt }> = [
    {
      kind: 'flowEndpoint',
      gesture: { kind: 'flowEndpoint', flow: f.uid, end: 'sink' },
      selection: [f.uid],
      press: sink,
      current: { x: sink.x + 40, y: sink.y + 60 },
    },
    {
      kind: 'flowEndpoint',
      gesture: { kind: 'flowEndpoint', flow: f.uid, end: 'source' },
      selection: [f.uid],
      press: src,
      current: { x: src.x - 50, y: src.y + 20 },
    },
    {
      kind: 'slideValve',
      gesture: { kind: 'slideValve', flow: f.uid },
      selection: [f.uid],
      press: f,
      current: { x: f.x + 12, y: f.y + 6 },
    },
    {
      kind: 'offsetSegment',
      gesture: { kind: 'offsetSegment', flow: f.uid, segmentIndex: 0 },
      selection: [f.uid],
      press: mid,
      current: horizontal ? { x: mid.x, y: mid.y + 25 } : { x: mid.x + 25, y: mid.y },
    },
  ];
  if (src.attachedToUid !== undefined) {
    out.push({
      kind: 'moveTerminal',
      gesture: { kind: 'moveSelection' },
      selection: [src.attachedToUid],
      press: src,
      current: { x: src.x + 30, y: src.y + 35 },
    });
  }
  return out;
}

describeWithEngine('gestures over imported corpus models', () => {
  let engine: EngineModule;

  beforeAll(async () => {
    engine = await loadEngine();
  });

  async function load(file: string): Promise<Model> {
    const bytes = new Uint8Array(fs.readFileSync(path.join(repoRoot, file)));
    const project = file.endsWith('.mdl') ? await engine.Project.openVensim(bytes) : await engine.Project.open(bytes);
    try {
      return await editorModel(project);
    } finally {
      await project.dispose();
    }
  }

  for (const file of CORPUS) {
    it(file, async () => {
      const model = await load(file);
      const s: Scene = { model, view: model.views[0] };
      const names = namesOf(s);
      const commits = new Map<FlowGestureKind, number>();
      const failures: string[] = [];
      const flows = s.view.elements.filter((e): e is FlowViewElement => e.type === 'flow' && e.points.length >= 2);
      expect(flows.length).toBeGreaterThan(0);
      for (const f of flows) {
        for (const g of gesturesFor(f)) {
          let plan: GesturePlan;
          try {
            plan = planGesture(planInput(s, g.gesture, g.press, g.current, { selection: new Set(g.selection), names }));
          } catch (err) {
            failures.push(`flow ${f.uid} ${g.kind}: threw ${String(err)}`);
            continue;
          }
          if (plan.commit !== 'edit') {
            continue;
          }
          commits.set(g.kind, (commits.get(g.kind) ?? 0) + 1);
          const view = planned(s, plan);
          const violations = checkFlowInvariants(view, { mode: 'strict', routed: routedOnly(s, plan, g) });
          const refs = checkReferentialIntegrity(view).filter((v) => !preexisting(s, v.arm, v.uid));
          if (violations.length > 0 || refs.length > 0) {
            failures.push(
              `flow ${f.uid} ${JSON.stringify(g)}:\n${formatFlowViolations(violations)}${formatViewViolations(refs)}`,
            );
          }
        }
      }
      expect(failures.slice(0, 3).join('\n\n')).toBe('');
      expect(FLOW_GESTURES.filter((k) => (k === 'moveTerminal' ? false : (commits.get(k) ?? 0) === 0))).toEqual([]);
    });
  }
});

// A flow translated with both terminals is not routed (support: a moved stock
// can carry both ends of an imported flow).
function routedOnly(s: Scene, plan: GesturePlan, g: { current: Pt; press: Pt }): Set<UID> {
  const d = { x: g.current.x - g.press.x, y: g.current.y - g.press.y };
  const base = new Map(s.view.elements.map((el) => [el.uid, el]));
  const out = new Set<UID>();
  for (const uid of routedFlows(s, plan)) {
    const b = base.get(uid);
    const n = plan.elements.find((el) => el.uid === uid);
    const translated =
      b?.type === 'flow' &&
      n?.type === 'flow' &&
      b.points.length === n.points.length &&
      b.points.every((p, i) => p.x + d.x === n.points[i].x && p.y + d.y === n.points[i].y);
    if (!translated) {
      out.add(uid);
    }
  }
  return out;
}

// M3 violations an imported view already carries (an orphan cloud, say) are the
// import's, not the edit's.
function preexisting(s: Scene, arm: string, uid: UID | undefined): boolean {
  return checkReferentialIntegrity(s.view).some((v) => v.arm === arm && v.uid === uid);
}

describe('corpus gesture fixtures', () => {
  it('every corpus model exists', () => {
    for (const file of CORPUS) {
      expect(`${file}: ${fs.existsSync(path.join(repoRoot, file))}`).toBe(`${file}: true`);
    }
  });
});
