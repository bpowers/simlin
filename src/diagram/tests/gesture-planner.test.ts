// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// planGesture, gesture by gesture. The per-gesture rows are keyed by
// GESTURE_KINDS (a compile-checked Record over the union), and every kind runs
// the same arms: a click within the threshold, a drag past it, the drag
// read-only, and the gesture naming a subject the view lacks. Kinds with drop
// targets add the invalid-target arm. Scenes load through the production
// modelFromJson (support/gesture-fixtures), and committed plans are checked with
// the strict flow checker over the flows the plan routed plus M3.
//
// What this establishes: each gesture's plan at one pointer position, E1 (a
// click previews and commits nothing), E6 (an invalid drop commits nothing), and
// that committed geometry holds G1-G8/M3 for these scenes. What it does not:
// continuity across frames and generated scenes (gesture-planner-fuzz.test.ts),
// the Canvas rendering the plan and committing it (canvas-gestures-*.test.tsx),
// or M1/M2 after the engine applies the edit (editor-gestures-engine.test.ts).

import { describe, it, expect } from '@rstest/core';

import type { FlowViewElement, LinkViewElement, UID } from '@simlin/core/datamodel';

import { planGesture, type Gesture, type GesturePlan } from '../gesture-planner';
import { GESTURE_KINDS } from '../gesture-planner/types';
import { StockHeight, StockWidth } from '../drawing/default';
import { buildEditOps } from '../view-model-sync';
import {
  aux,
  cloud,
  committedReport,
  elementOf,
  flow,
  link,
  linkedAuxes,
  planInput,
  planned,
  routedFlows,
  scene,
  stock,
  stockToCloud,
  stockToStock,
  type Pt,
  type Scene,
} from './support/gesture-fixtures';

interface KindRow {
  readonly scene: () => Scene;
  readonly gesture: Gesture;
  readonly selection?: ReadonlySet<UID>;
  readonly press: Pt;
  /** A pointer past the click threshold. */
  readonly moved: Pt;
  /** A pointer within the threshold; defaults to press + (2, 1). */
  readonly click?: Pt;
  readonly commit: GesturePlan['commit'];
  /** The same gesture naming a subject the view lacks; undefined for gestures with no subject. */
  readonly subjectless?: Gesture;
  readonly check?: (s: Scene, plan: GesturePlan) => void;
}

const ROWS: Record<Gesture['kind'], KindRow> = {
  moveSelection: {
    scene: stockToCloud,
    gesture: { kind: 'moveSelection' },
    selection: new Set([1]),
    press: { x: 100, y: 100 },
    moved: { x: 100, y: 150 },
    commit: 'edit',
    check: (s, plan) => {
      expect(elementOf(plan, 1)).toMatchObject({ x: 100, y: 150 });
      expect([...routedFlows(s, plan)]).toEqual([3]);
      expect(elementOf(plan, 2)).toBe(s.view.elements.find((e) => e.uid === 2));
    },
  },
  slideValve: {
    scene: stockToCloud,
    gesture: { kind: 'slideValve', flow: 3 },
    selection: new Set([3]),
    press: { x: 200, y: 100 },
    moved: { x: 240, y: 100 },
    commit: 'edit',
    subjectless: { kind: 'slideValve', flow: 999 },
    check: (_s, plan) => {
      expect(elementOf(plan, 3)).toMatchObject({ x: 240, y: 100 });
    },
  },
  offsetSegment: {
    scene: stockToStock,
    gesture: { kind: 'offsetSegment', flow: 3, segmentIndex: 0 },
    selection: new Set([3]),
    press: { x: 250, y: 100 },
    moved: { x: 250, y: 150 },
    commit: 'edit',
    subjectless: { kind: 'offsetSegment', flow: 999, segmentIndex: 0 },
    check: (_s, plan) => {
      // #819 as a bracket: beyond the face extent (100 + 17.5 - 3) plus
      // MIN_SEGMENT, each endpoint keeps its face at the extent and a stub and a
      // riser reach the run at the pointer's y.
      const f = elementOf(plan, 3) as FlowViewElement;
      expect(f.points.length).toBe(6);
      expect(f.points[0]).toMatchObject({ x: 122.5, y: 114.5 });
      expect(f.points[5]).toMatchObject({ x: 377.5, y: 114.5 });
      expect(f.points.some((p, i) => i > 0 && p.y === 150 && f.points[i - 1].y === 150)).toBe(true);
    },
  },
  flowEndpoint: {
    scene: stockToCloud,
    gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
    selection: new Set([3]),
    press: { x: 300, y: 100 },
    moved: { x: 360, y: 160 },
    commit: 'edit',
    subjectless: { kind: 'flowEndpoint', flow: 999, end: 'sink' },
    check: (_s, plan) => {
      const f = elementOf(plan, 3) as FlowViewElement;
      expect(f.points[f.points.length - 1]).toMatchObject({ x: 360, y: 160, attachedToUid: 2 });
      expect(elementOf(plan, 2)).toMatchObject({ x: 360, y: 160 });
    },
  },
  linkEndpoint: {
    scene: linkedAuxes,
    gesture: { kind: 'linkEndpoint', link: 13 },
    selection: new Set([13]),
    press: { x: 291, y: 300 },
    moved: { x: 303, y: 447 },
    commit: 'edit',
    subjectless: { kind: 'linkEndpoint', link: 999 },
    check: (_s, plan) => {
      const l = elementOf(plan, 13) as LinkViewElement;
      expect(l.toUid).toBe(12);
      expect(Number.isFinite(l.arc)).toBe(true);
      expect(plan.target).toEqual({ uid: 12, valid: true });
    },
  },
  linkArc: {
    scene: linkedAuxes,
    gesture: { kind: 'linkArc', link: 13 },
    selection: new Set([13]),
    press: { x: 200, y: 300 },
    moved: { x: 200, y: 340 },
    commit: 'edit',
    subjectless: { kind: 'linkArc', link: 999 },
    check: (_s, plan) => {
      const l = elementOf(plan, 13) as LinkViewElement;
      expect(Number.isFinite(l.arc)).toBe(true);
      expect(l.arc).not.toBe(20);
    },
  },
  createFlow: {
    scene: linkedAuxes,
    gesture: { kind: 'createFlow', from: 'empty' },
    press: { x: 500, y: 600 },
    moved: { x: 620, y: 600 },
    commit: 'edit',
    subjectless: { kind: 'createFlow', from: { stock: 999 } },
    check: (s, plan) => {
      const uid = s.view.nextUid;
      const f = elementOf(plan, uid) as FlowViewElement;
      expect(f.name).toBe('New Flow');
      expect(f.points.map((p) => [p.x, p.y])).toEqual([
        [500, 600],
        [620, 600],
      ]);
      expect(plan.nextUid).toBe(uid + 3);
      expect(plan.selection).toEqual(new Set([uid]));
      expect(plan.handoff).toEqual({ editName: uid });
      // M1 for the created element: the committed edit creates its variable.
      const ops = buildEditOps(s.model, s.view, planned(s, plan));
      expect(ops.some((op) => op.type === 'upsertFlow')).toBe(true);
    },
  },
  createLink: {
    scene: linkedAuxes,
    gesture: { kind: 'createLink', from: 10 },
    press: { x: 100, y: 300 },
    moved: { x: 303, y: 447 },
    commit: 'edit',
    subjectless: { kind: 'createLink', from: 999 },
    check: (s, plan) => {
      const l = elementOf(plan, s.view.nextUid) as LinkViewElement;
      expect({ fromUid: l.fromUid, toUid: l.toUid }).toEqual({ fromUid: 10, toUid: 12 });
      expect(plan.selection).toEqual(new Set([s.view.nextUid]));
    },
  },
  createElement: {
    scene: stockToCloud,
    gesture: { kind: 'createElement', type: 'aux' },
    press: { x: 500, y: 500 },
    moved: { x: 560, y: 540 },
    commit: 'none',
    check: (_s, plan) => {
      expect(plan.draft).toMatchObject({ type: 'aux', x: 560, y: 540, name: 'New Variable' });
      expect(plan.handoff).toEqual({ editName: plan.draft!.uid });
    },
  },
  label: {
    scene: () => scene([{ ...aux(10, 'a', 100, 300), labelSide: 'right' } as never, aux(11, 'b', 300, 300)]),
    gesture: { kind: 'label', uid: 10 },
    selection: new Set([10]),
    press: { x: 130, y: 300 },
    moved: { x: 100, y: 260 },
    // The label component owns the threshold: a frame whose side is the label's
    // own changes nothing.
    click: { x: 132, y: 301 },
    commit: 'edit',
    subjectless: { kind: 'label', uid: 999 },
    check: (_s, plan) => {
      expect(elementOf(plan, 10)).toMatchObject({ labelSide: 'top' });
    },
  },
  rubberBand: {
    scene: linkedAuxes,
    gesture: { kind: 'rubberBand' },
    press: { x: 50, y: 250 },
    moved: { x: 350, y: 350 },
    commit: 'select',
    check: (_s, plan) => {
      expect(plan.selection).toEqual(new Set([10, 11]));
    },
  },
  pan: {
    scene: stockToCloud,
    gesture: { kind: 'pan' },
    press: { x: 500, y: 500 },
    moved: { x: 600, y: 600 },
    commit: 'none',
  },
};

describe('planGesture per gesture kind', () => {
  it('has a row for every gesture kind', () => {
    expect(Object.keys(ROWS).sort()).toEqual([...GESTURE_KINDS].sort());
  });

  for (const kind of GESTURE_KINDS) {
    const row = ROWS[kind];
    const plan = (s: Scene, current: Pt, overrides = {}): GesturePlan =>
      planGesture(
        planInput(s, row.gesture, row.press, current, { selection: row.selection ?? new Set(), ...overrides }),
      );

    it(`${kind}: a drag past the threshold commits '${row.commit}' holding the invariants`, () => {
      const s = row.scene();
      const p = plan(s, row.moved);
      expect(p.commit).toBe(row.commit);
      if (p.commit === 'edit') {
        expect(committedReport(s, p)).toBe('');
      }
      row.check?.(s, p);
    });

    it(`${kind}: a click within the threshold previews and commits nothing (E1)`, () => {
      const s = row.scene();
      const p = plan(s, row.click ?? { x: row.press.x + 2, y: row.press.y + 1 });
      expect(p.commit).not.toBe('edit');
      if (kind === 'createElement') {
        // The exception: an armed creation tool places its draft at the press.
        expect(p.elements.slice(0, -1)).toEqual(s.view.elements);
        expect(p.draft).toMatchObject({ x: row.press.x, y: row.press.y });
      } else {
        expect(p.elements).toBe(s.view.elements);
      }
    });

    it(`${kind}: read-only, the drag previews and commits no edit`, () => {
      const s = row.scene();
      const p = plan(s, row.moved, { readOnly: true });
      expect(p.commit).not.toBe('edit');
      expect(p.elements).toBe(s.view.elements);
      expect(p.draft).toBeUndefined();
    });

    it(`${kind}: a subject the view lacks changes nothing`, () => {
      const s = row.scene();
      if (row.subjectless === undefined) {
        // moveSelection's subject is its selection; the rest have none.
        if (kind === 'moveSelection') {
          const p = plan(s, row.moved, { selection: new Set([999]) });
          expect(p.commit).toBe('none');
          expect(p.elements).toBe(s.view.elements);
        }
        return;
      }
      const p = planGesture(planInput(s, row.subjectless, row.press, row.moved, { selection: row.selection }));
      expect(p.commit).toBe('none');
      expect(p.elements).toBe(s.view.elements);
    });
  }
});

interface TargetRow {
  readonly name: string;
  readonly scene: () => Scene;
  readonly gesture: Gesture;
  readonly press: Pt;
  readonly current: Pt;
  readonly target: GesturePlan['target'];
}

// The kinds with drop targets, each with the invalid arms the plan names.
const INVALID_TARGETS: readonly TargetRow[] = [
  {
    name: 'flowEndpoint R12: a sink cloud over the flow`s own source stock',
    scene: stockToCloud,
    gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
    press: { x: 300, y: 100 },
    current: { x: 100, y: 100 },
    target: { uid: 1, valid: false },
  },
  {
    name: 'flowEndpoint R14: a source dragged over the flow`s own sink stock',
    scene: stockToStock,
    gesture: { kind: 'flowEndpoint', flow: 3, end: 'source' },
    press: { x: 132, y: 100 },
    current: { x: 400, y: 105 },
    target: { uid: 2, valid: false },
  },
  {
    name: 'flowEndpoint M6: a stock whose variable does not exist',
    scene: () =>
      scene(
        [
          stock(1, 'S', 100, 100),
          stock(4, 'T', 400, 100),
          cloud(2, 3, 300, 100),
          flow(3, 'F', { x: 200, y: 100 }, [
            [122.5, 100, 1],
            [300, 100, 2],
          ]),
        ],
        ['T'],
      ),
    gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
    press: { x: 300, y: 100 },
    current: { x: 400, y: 100 },
    target: { uid: 4, valid: false },
  },
  {
    name: 'createFlow R13/P36: released inside its own source stock',
    scene: stockToCloud,
    gesture: { kind: 'createFlow', from: { stock: 1 } },
    press: { x: 100, y: 100 },
    current: { x: 105, y: 108 },
    target: { uid: 1, valid: false },
  },
  {
    name: 'createLink: a second link between the same two elements',
    scene: linkedAuxes,
    gesture: { kind: 'createLink', from: 10 },
    press: { x: 100, y: 300 },
    current: { x: 302, y: 302 },
    target: { uid: 11, valid: false },
  },
  {
    name: 'linkEndpoint: onto an element the source already links to',
    scene: () =>
      scene([
        aux(10, 'a', 100, 300),
        aux(11, 'b', 300, 300),
        aux(12, 'c', 300, 450),
        link(13, 10, 11),
        link(14, 10, 12),
      ]),
    gesture: { kind: 'linkEndpoint', link: 13 },
    press: { x: 291, y: 300 },
    current: { x: 300, y: 450 },
    target: { uid: 12, valid: false },
  },
];

describe('planGesture invalid drop targets (E6)', () => {
  for (const row of INVALID_TARGETS) {
    it(row.name, () => {
      const s = row.scene();
      const p = planGesture(planInput(s, row.gesture, row.press, row.current));
      expect(p.target).toEqual(row.target);
      expect(p.commit).toBe('none');
      expect(p.handoff).toBeUndefined();
    });
  }
});

describe('planGesture audit repros and pinned behaviors', () => {
  it('H1/P28: a click on a link arrowhead never deletes the link', () => {
    const s = linkedAuxes();
    const p = planGesture(planInput(s, { kind: 'linkEndpoint', link: 13 }, { x: 286, y: 300 }, { x: 286, y: 300 }));
    expect(p.commit).toBe('none');
    expect(p.elements).toBe(s.view.elements);
  });

  it('L-e/P35: a link arrowhead dropped on its own source aborts (no target, no commit)', () => {
    const s = linkedAuxes();
    const p = planGesture(planInput(s, { kind: 'linkEndpoint', link: 13 }, { x: 291, y: 300 }, { x: 100, y: 300 }));
    expect(p.target).toBeUndefined();
    expect(p.commit).toBe('none');
  });

  it('a link arrowhead dropped on empty space aborts instead of deleting the link', () => {
    const s = linkedAuxes();
    const p = planGesture(planInput(s, { kind: 'linkEndpoint', link: 13 }, { x: 291, y: 300 }, { x: 700, y: 700 }));
    expect(p.commit).toBe('none');
    expect(p.elements.some((e) => e.uid === 13)).toBe(true);
  });

  it.each([
    ['C0: the source grip', 'source', { x: 132, y: 100 }],
    ['T8: the arrowhead', 'sink', { x: 296, y: 100 }],
  ] as const)('%s of a flow: a click commits nothing and detaches nothing', (_name, end, at) => {
    const s = stockToCloud();
    const p = planGesture(planInput(s, { kind: 'flowEndpoint', flow: 3, end }, at, { x: at.x + 1, y: at.y + 2 }));
    expect(p.commit).toBe('none');
    expect(p.elements).toBe(s.view.elements);
  });

  it('E6 for moves: a routed flow that cannot hold G2-G6 (its cloud dragged into another stock) commits nothing', () => {
    const s = scene([
      stock(1, 'S', 100, 100),
      cloud(2, 3, 300, 100),
      flow(3, 'F', { x: 200, y: 100 }, [
        [122.5, 100, 1],
        [300, 100, 2],
      ]),
      aux(10, 'a', 600, 600),
      stock(4, 'T', 300, 300),
    ]);
    const p = planGesture(
      planInput(s, { kind: 'moveSelection' }, { x: 300, y: 100 }, { x: 300, y: 300 }, { selection: new Set([2, 10]) }),
    );
    expect(elementOf(p, 2)).toMatchObject({ x: 300, y: 300 });
    expect(p.commit).toBe('none');
    const clear = planGesture(
      planInput(s, { kind: 'moveSelection' }, { x: 300, y: 100 }, { x: 300, y: 200 }, { selection: new Set([2, 10]) }),
    );
    expect(clear.commit).toBe('edit');
  });

  // G6 is best effort when the terminal bodies, each inflated by MIN_SEGMENT,
  // overlap, so a stock dragged onto or up to its flow's other terminal still
  // commits, holding G1-G5. Only a violation the plan does not excuse refuses a
  // move: a cloud center inside a stock while the flow's terminals are apart.
  describe('a move commits unless a routed flow breaks an invariant the plan does not excuse', () => {
    const cloudBesideStock = (): Scene =>
      scene([
        stock(1, 'S', 100, 100),
        cloud(2, 3, 300, 100),
        flow(3, 'F', { x: 200, y: 100 }, [
          [122.5, 100, 1],
          [300, 100, 2],
        ]),
        stock(4, 'T', 300, 300),
      ]);
    const ROWS: ReadonlyArray<{
      name: string;
      scene: () => Scene;
      selection: number[];
      press: Pt;
      current: Pt;
      commit: GesturePlan['commit'];
    }> = [
      {
        name: 'a stock dragged onto its flow`s other stock (bodies overlap) commits',
        scene: stockToStock,
        selection: [1],
        press: { x: 100, y: 100 },
        current: { x: 380, y: 110 },
        commit: 'edit',
      },
      {
        name: 'a stock dragged 5px short of its flow`s other stock (inflated bodies overlap) commits',
        scene: stockToStock,
        selection: [1],
        press: { x: 100, y: 100 },
        current: { x: 350, y: 100 },
        commit: 'edit',
      },
      {
        name: 'a stock dragged onto its own flow`s sink cloud (bodies overlap) commits',
        scene: stockToCloud,
        selection: [1],
        press: { x: 100, y: 100 },
        current: { x: 300, y: 100 },
        commit: 'edit',
      },
      {
        name: 'a cloud dragged into another stock while the flow`s terminals are apart refuses',
        scene: cloudBesideStock,
        selection: [2],
        press: { x: 300, y: 100 },
        current: { x: 300, y: 300 },
        commit: 'none',
      },
    ];
    for (const row of ROWS) {
      it(row.name, () => {
        const s = row.scene();
        const p = planGesture(
          planInput(s, { kind: 'moveSelection' }, row.press, row.current, { selection: new Set(row.selection) }),
        );
        expect(p.commit).toBe(row.commit);
        if (row.commit === 'edit') {
          // The strict checker excuses exactly what the plan excuses (G6 and the G3
          // minima without room), so this is G1-G5 plus G7/G8 and M3.
          expect(committedReport(s, p)).toBe('');
        }
      });
    }
  });

  it('Vensim fallback flows: routing a flow with unattached ends attaches each to a new cloud', () => {
    const s = scene([
      aux(10, 'a', 600, 600),
      flow(3, 'F', { x: 200, y: 100 }, [
        [100, 100],
        [300, 100],
      ]),
    ]);
    const p = planGesture(
      planInput(
        s,
        { kind: 'slideValve', flow: 3 },
        { x: 200, y: 100 },
        { x: 240, y: 100 },
        { selection: new Set([3]) },
      ),
    );
    expect(p.commit).toBe('edit');
    const f = elementOf(p, 3) as FlowViewElement;
    expect([f.points[0].attachedToUid, f.points[1].attachedToUid]).toEqual([s.view.nextUid, s.view.nextUid + 1]);
    expect(p.nextUid).toBe(s.view.nextUid + 2);
    expect(committedReport(s, p)).toBe('');
  });

  it('#832: grabbing a source cloud without moving leaves an off-center valve where it is', () => {
    const s = scene([
      cloud(2, 3, 100, 100),
      stock(1, 'S', 300, 100),
      flow(3, 'F', { x: 130, y: 100 }, [
        [100, 100, 2],
        [277.5, 100, 1],
      ]),
    ]);
    const p = planGesture(
      planInput(s, { kind: 'flowEndpoint', flow: 3, end: 'source' }, { x: 100, y: 100 }, { x: 100, y: 100 }),
    );
    expect(p.elements).toBe(s.view.elements);
  });

  it('H5/audit lead A: a sink cloud dropped on a column-aligned stock lands on that stock`s face', () => {
    const s = scene([
      stock(1, 'S', 100, 100),
      cloud(2, 3, 300, 100),
      stock(4, 'T', 130, 250),
      flow(3, 'F', { x: 200, y: 100 }, [
        [122.5, 100, 1],
        [300, 100, 2],
      ]),
    ]);
    const p = planGesture(
      planInput(s, { kind: 'flowEndpoint', flow: 3, end: 'sink' }, { x: 300, y: 100 }, { x: 130, y: 250 }),
    );
    expect(p.commit).toBe('edit');
    expect(p.target).toEqual({ uid: 4, valid: true });
    const f = elementOf(p, 3) as FlowViewElement;
    const end = f.points[f.points.length - 1];
    expect(end.attachedToUid).toBe(4);
    expect(Math.abs(end.x - 130) <= StockWidth / 2 + 1e-6 && Math.abs(end.y - 250) <= StockHeight / 2 + 1e-6).toBe(
      true,
    );
    expect(p.elements.some((e) => e.uid === 2)).toBe(false);
    expect(committedReport(s, p)).toBe('');
  });

  it('R13b/M4: a flow drawn from an off-center press on a stock ends exactly at the pointer', () => {
    const s = scene([stock(1, 'S', 200, 200)]);
    const current = { x: 315, y: 210 };
    const p = planGesture(planInput(s, { kind: 'createFlow', from: { stock: 1 } }, { x: 215, y: 210 }, current));
    expect(p.commit).toBe('edit');
    const f = elementOf(p, s.view.nextUid) as FlowViewElement;
    expect(f.points[f.points.length - 1]).toMatchObject(current);
    expect(f.points[0]).toMatchObject({ x: 222.5, attachedToUid: 1 });
    expect(committedReport(s, p)).toBe('');
  });

  it('M5/P27: a link into a flow whose valve moves follows it in the planned elements', () => {
    const s = scene([
      stock(1, 'S', 100, 100),
      cloud(2, 3, 300, 100),
      flow(3, 'F', { x: 200, y: 100 }, [
        [122.5, 100, 1],
        [300, 100, 2],
      ]),
      aux(20, 'x', 200, 20),
      link(21, 20, 3, 30),
    ]);
    const p = planGesture(planInput(s, { kind: 'slideValve', flow: 3 }, { x: 200, y: 100 }, { x: 260, y: 100 }));
    const l = elementOf(p, 21) as LinkViewElement;
    expect(l.arc).not.toBe(30);
    expect(Number.isFinite(l.arc)).toBe(true);
  });

  it('#818: a flow whose stored valve is not finite routes to finite geometry', () => {
    const s = stockToCloud();
    const f = s.view.elements.find((e) => e.uid === 3) as FlowViewElement;
    const broken: Scene = {
      ...s,
      view: { ...s.view, elements: s.view.elements.map((e) => (e === f ? { ...f, x: NaN } : e)) },
    };
    const p = planGesture(
      planInput(broken, { kind: 'flowEndpoint', flow: 3, end: 'sink' }, { x: 300, y: 100 }, { x: 400, y: 100 }),
    );
    const routed = elementOf(p, 3) as FlowViewElement;
    expect([routed.x, routed.y, ...routed.points.flatMap((pt) => [pt.x, pt.y])].every(Number.isFinite)).toBe(true);
  });

  it('#720: a flow whose source and sink are one stock neither throws nor breaks the move', () => {
    const s = scene([
      stock(1, 'S', 100, 100),
      flow(3, 'F', { x: 100, y: 60 }, [
        [100, 82.5, 1],
        [100, 60],
        [140, 60],
        [140, 100],
        [122.5, 100, 1],
      ]),
    ]);
    const move = planGesture(
      planInput(s, { kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 140, y: 140 }, { selection: new Set([1]) }),
    );
    expect(elementOf(move, 3)).toMatchObject({ x: 140, y: 100 });
    expect(() =>
      planGesture(planInput(s, { kind: 'flowEndpoint', flow: 3, end: 'sink' }, { x: 122, y: 100 }, { x: 300, y: 300 })),
    ).not.toThrow();
  });

  it('touch-straight links: a touch link is always straight, a mouse link curves through the pointer', () => {
    const s = linkedAuxes();
    const at = (pointerType: string) =>
      elementOf(
        planGesture(
          planInput(s, { kind: 'createLink', from: 10 }, { x: 100, y: 300 }, { x: 300, y: 448 }, { pointerType }),
        ),
        s.view.nextUid,
      ) as LinkViewElement;
    expect(at('touch').arc).toBeUndefined();
    expect(Number.isFinite(at('mouse').arc)).toBe(true);
  });

  it('wobble-is-a-click: a sub-threshold wobble on a selected element settles its click selection and opens details', () => {
    const s = linkedAuxes();
    const p = planGesture(
      planInput(
        s,
        { kind: 'moveSelection' },
        { x: 100, y: 300 },
        { x: 102, y: 302 },
        {
          selection: new Set([10, 11]),
          clickSelection: new Set([10]),
        },
      ),
    );
    expect(p).toMatchObject({ commit: 'select', details: true });
    expect(p.selection).toEqual(new Set([10]));
  });

  it('the click threshold is in screen pixels: 2 model px at zoom 4 is a drag', () => {
    const s = stockToCloud();
    const input = planInput(
      s,
      { kind: 'moveSelection' },
      { x: 100, y: 100 },
      { x: 100, y: 102 },
      { selection: new Set([1]) },
    );
    expect(planGesture(input).commit).toBe('none');
    expect(planGesture({ ...input, zoom: 4 }).commit).toBe('edit');
  });

  it('E4: elements the gesture does not route are the very same objects', () => {
    const s = scene([
      stock(1, 'S', 100, 100),
      cloud(2, 3, 300, 100),
      flow(3, 'F', { x: 200, y: 100 }, [
        [122.5, 100, 1],
        [300, 100, 2],
      ]),
      aux(10, 'a', 600, 600),
      stock(11, 'U', 600, 300),
    ]);
    const p = planGesture(
      planInput(s, { kind: 'moveSelection' }, { x: 100, y: 100 }, { x: 100, y: 160 }, { selection: new Set([1]) }),
    );
    for (const uid of [2, 10, 11]) {
      expect(elementOf(p, uid)).toBe(s.view.elements.find((e) => e.uid === uid));
    }
  });
});
