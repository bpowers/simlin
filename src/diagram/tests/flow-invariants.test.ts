// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of the committed flow-invariant checker (tests/support/flow-invariants.ts).
//
// Rows are derived from FLOW_ARMS: every arm has one row with a minimal valid
// fixture that passes strict mode and a minimal mutation strict mode reports.
// Each mutation's EXPECTED arm list is exact (a multiset), so a row pins what
// its arm reports AND that no other arm fires on it; where a mutation cannot
// violate one arm without another (an inward stub necessarily crosses the
// body), the row lists both and says why. Each row also states, literally, what
// tolerant mode reports on the same mutation.
//
// The boundary table then pins each threshold at the value the plan names:
// equality on both sides of every `<` / `<=` choice, sub-pixel defects against
// the epsilon, the room and precondition inflations at the exact touching
// distance, and one row per face or terminal wherever an arm has a per-face or
// per-terminal branch.
//
// Fixtures go through the production loader (`modelFromJson`), with stock
// endpoints pinned on faces and clouds at their endpoints as the engine stores
// them. The one exception is the non-finite row: the loader repairs NaN on load
// (issue #818), so a NaN can only exist in memory, where a planner computing it
// would put it; that row mutates the loaded view.

import { describe, it, expect } from '@rstest/core';

import type { JsonModel, JsonViewElement } from '@simlin/engine';
import { modelFromJson, type FlowViewElement, type StockFlowView, type UID } from '@simlin/core/datamodel';

import {
  ALL_FLOW_ARMS,
  checkFlowInvariants,
  CORNER_CLEARANCE,
  formatFlowViolations,
  GEOMETRY_EPSILON,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  PIPE_SPACING,
  TOLERANT_FLOW_ARMS,
  VALVE_CLAMP_MARGIN,
  type FlowArm,
  type FlowViolation,
} from './support/flow-invariants';

type P = [number, number] | [number, number, number];

function loadView(elements: JsonViewElement[]): StockFlowView {
  return modelFromJson({ name: 'main', views: [{ elements }] } as JsonModel).views[0];
}

const stock = (uid: UID, x: number, y: number): JsonViewElement => ({ type: 'stock', uid, name: `s${uid}`, x, y });
const aux = (uid: UID, x: number, y: number): JsonViewElement => ({ type: 'aux', uid, name: `a${uid}`, x, y });
const cloud = (uid: UID, flowUid: UID, x: number, y: number): JsonViewElement => ({
  type: 'cloud',
  uid,
  flowUid,
  x,
  y,
});
function flow(uid: UID, valve: [number, number], points: P[]): JsonViewElement {
  return {
    type: 'flow',
    uid,
    name: `f${uid}`,
    x: valve[0],
    y: valve[1],
    points: points.map(([x, y, attachedToUid]) => (attachedToUid === undefined ? { x, y } : { x, y, attachedToUid })),
  };
}

// Stock S1 at (100,100): faces x = 77.5 / 122.5, y = 82.5 / 117.5.
const S1 = stock(1, 100, 100);
// Straight flow out of S1's right face into a cloud, valve mid-path.
const STRAIGHT = [
  S1,
  flow(
    2,
    [200, 100],
    [
      [122.5, 100, 1],
      [300, 100, 3],
    ],
  ),
  cloud(3, 2, 300, 100),
];
// L out of S1's right face turning down into a cloud.
const L_DOWN = [
  S1,
  flow(
    2,
    [200, 175],
    [
      [122.5, 100, 1],
      [200, 100],
      [200, 250, 3],
    ],
  ),
  cloud(3, 2, 200, 250),
];
// Cloud-to-cloud straight flow.
const CLOUD_STRAIGHT = [
  flow(
    10,
    [50, 0],
    [
      [0, 0, 11],
      [100, 0, 12],
    ],
  ),
  cloud(11, 10, 0, 0),
  cloud(12, 10, 100, 0),
];

function sorted(arms: readonly string[]): string[] {
  return [...arms].sort();
}

function arms(violations: readonly FlowViolation[]): string[] {
  return sorted(violations.map((v) => v.arm));
}

interface ArmRow {
  readonly arm: FlowArm;
  readonly valid: JsonViewElement[];
  readonly broken: () => StockFlowView;
  /** Every arm strict mode reports on `broken`, one entry per occurrence. */
  readonly expected: readonly FlowArm[];
  /** Every arm tolerant mode reports on `broken`, one entry per occurrence. */
  readonly tolerant: readonly FlowArm[];
  readonly uid: UID;
  readonly numbers?: Readonly<Record<string, number>>;
  readonly why?: string;
}

const ROWS: readonly ArmRow[] = [
  {
    arm: 'G1.minPoints',
    valid: STRAIGHT,
    broken: () => loadView([S1, flow(2, [122.5, 100], [[122.5, 100, 1]]), cloud(3, 2, 300, 100)]),
    expected: ['G1.minPoints'],
    tolerant: ['G1.minPoints'],
    uid: 2,
    numbers: { points: 1 },
  },
  {
    arm: 'G1.nonFinite',
    valid: STRAIGHT,
    broken: () => mapFlow(loadView(STRAIGHT), 2, (f) => ({ ...f, x: NaN })),
    expected: ['G1.nonFinite'],
    tolerant: ['G1.nonFinite'],
    uid: 2,
    numbers: { count: 1 },
  },
  {
    arm: 'G1.unattachedEndpoint',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [122.5, 100, 1],
            [300, 100],
          ],
        ),
        cloud(3, 2, 300, 100),
      ]),
    expected: ['G1.unattachedEndpoint'],
    tolerant: [],
    uid: 2,
    numbers: { endIndex: 1 },
  },
  {
    arm: 'G1.danglingAttachment',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [122.5, 100, 1],
            [300, 100, 99],
          ],
        ),
        cloud(3, 2, 300, 100),
      ]),
    expected: ['G1.danglingAttachment'],
    tolerant: ['G1.danglingAttachment'],
    uid: 2,
    numbers: { endIndex: 1, attachedToUid: 99 },
  },
  {
    arm: 'G1.attachmentKind',
    valid: [...STRAIGHT, aux(4, 400, 400)],
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [122.5, 100, 1],
            [300, 100, 4],
          ],
        ),
        cloud(3, 2, 300, 100),
        aux(4, 300, 100),
      ]),
    expected: ['G1.attachmentKind'],
    tolerant: ['G1.attachmentKind'],
    uid: 2,
    numbers: { attachedToUid: 4 },
  },
  {
    arm: 'G1.foreignCloud',
    valid: [...STRAIGHT, ...CLOUD_STRAIGHT],
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [122.5, 100, 1],
            [300, 100, 3],
          ],
        ),
        cloud(3, 10, 300, 100),
        ...CLOUD_STRAIGHT,
      ]),
    expected: ['G1.foreignCloud'],
    tolerant: ['G1.foreignCloud'],
    uid: 2,
    numbers: { cloudUid: 3, cloudFlowUid: 10 },
  },
  {
    arm: 'G1.interiorAttached',
    valid: L_DOWN,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 175],
          [
            [122.5, 100, 1],
            [200, 100, 1],
            [200, 250, 3],
          ],
        ),
        cloud(3, 2, 200, 250),
      ]),
    expected: ['G1.interiorAttached'],
    tolerant: ['G1.interiorAttached'],
    uid: 2,
    numbers: { index: 1, attachedToUid: 1 },
  },
  {
    arm: 'G1.nonPositiveUid',
    valid: [...STRAIGHT, aux(4, 400, 400)],
    // -3 is the Canvas's faux-target sentinel, the kind of uid that must never
    // reach a committed view.
    broken: () => loadView([...STRAIGHT, aux(-3, 400, 400)]),
    expected: ['G1.nonPositiveUid'],
    tolerant: [],
    uid: -3,
    numbers: { uid: -3 },
  },
  {
    arm: 'G1.sourceIsSink',
    valid: [
      S1,
      stock(4, 300, 100),
      flow(
        2,
        [200, 100],
        [
          [122.5, 100, 1],
          [277.5, 100, 4],
        ],
      ),
    ],
    // Out of S1's right face, around, and back into its top face. The terminals
    // overlap themselves, so the G3 minima and G6 are exempt; only G1 reports.
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [160, 70],
          [
            [122.5, 100, 1],
            [160, 100],
            [160, 40],
            [100, 40],
            [100, 82.5, 1],
          ],
        ),
      ]),
    expected: ['G1.sourceIsSink'],
    tolerant: [],
    uid: 2,
    numbers: { attachedToUid: 1 },
  },
  {
    arm: 'G2.diagonal',
    valid: CLOUD_STRAIGHT,
    broken: () =>
      loadView([
        flow(
          10,
          [50, 1.5],
          [
            [0, 0, 11],
            [100, 3, 12],
          ],
        ),
        cloud(11, 10, 0, 0),
        cloud(12, 10, 100, 3),
      ]),
    expected: ['G2.diagonal'],
    tolerant: [],
    uid: 10,
    numbers: { segment: 0, dx: 100, dy: 3 },
  },
  {
    arm: 'G3.zeroLength',
    valid: L_DOWN,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 175],
          [
            [122.5, 100, 1],
            [200, 100],
            [200, 100],
            [200, 250, 3],
          ],
        ),
        cloud(3, 2, 200, 250),
      ]),
    expected: ['G3.zeroLength'],
    tolerant: [],
    uid: 2,
    numbers: { segment: 1 },
  },
  {
    arm: 'G3.collinear',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [122.5, 100, 1],
            [200, 100],
            [300, 100, 3],
          ],
        ),
        cloud(3, 2, 300, 100),
      ]),
    expected: ['G3.collinear'],
    tolerant: [],
    uid: 2,
    numbers: { segment: 1 },
  },
  {
    arm: 'G3.shortStub',
    valid: [
      flow(
        10,
        [50, 175],
        [
          [0, 100, 11],
          [50, 100],
          [50, 250, 12],
        ],
      ),
      cloud(11, 10, 0, 100),
      cloud(12, 10, 50, 250),
    ],
    broken: () =>
      loadView([
        flow(
          10,
          [5, 175],
          [
            [0, 100, 11],
            [5, 100],
            [5, 250, 12],
          ],
        ),
        cloud(11, 10, 0, 100),
        cloud(12, 10, 5, 250),
      ]),
    expected: ['G3.shortStub'],
    tolerant: [],
    uid: 10,
    numbers: { segment: 0, length: 5, minimum: 10 },
  },
  {
    arm: 'G3.shortRiser',
    valid: [
      flow(
        10,
        [50, 100],
        [
          [0, 100, 11],
          [100, 100],
          [100, 150],
          [200, 150, 12],
        ],
      ),
      cloud(11, 10, 0, 100),
      cloud(12, 10, 200, 150),
    ],
    broken: () =>
      loadView([
        flow(
          10,
          [50, 100],
          [
            [0, 100, 11],
            [100, 100],
            [100, 104],
            [200, 104, 12],
          ],
        ),
        cloud(11, 10, 0, 100),
        cloud(12, 10, 200, 104),
      ]),
    expected: ['G3.shortRiser'],
    tolerant: [],
    uid: 10,
    numbers: { segment: 1, length: 4, minimum: 10 },
  },
  {
    arm: 'G3.shortSink',
    valid: [
      S1,
      flow(
        2,
        [160, 100],
        [
          [122.5, 100, 1],
          [200, 100],
          [200, 250, 3],
        ],
      ),
      cloud(3, 2, 200, 250),
    ],
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [160, 100],
          [
            [122.5, 100, 1],
            [200, 100],
            [200, 110, 3],
          ],
        ),
        cloud(3, 2, 200, 110),
      ]),
    expected: ['G3.shortSink'],
    tolerant: [],
    uid: 2,
    numbers: { segment: 1, length: 10, minimum: 15.5 },
  },
  {
    arm: 'G4.offFace',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [130, 100, 1],
            [300, 100, 3],
          ],
        ),
        cloud(3, 2, 300, 100),
      ]),
    expected: ['G4.offFace'],
    tolerant: [],
    uid: 2,
    numbers: { endIndex: 0, dx: 30, dy: 0 },
  },
  {
    arm: 'G4.cornerClearance',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 83.5],
          [
            [122.5, 83.5, 1],
            [300, 83.5, 3],
          ],
        ),
        cloud(3, 2, 300, 83.5),
      ]),
    expected: ['G4.cornerClearance'],
    tolerant: [],
    uid: 2,
    numbers: { endIndex: 0, clearance: 1, minimum: 3 },
  },
  {
    arm: 'G5.notPerpendicular',
    valid: [
      S1,
      flow(
        2,
        [160, 100],
        [
          [122.5, 100, 1],
          [200, 100],
          [200, 40, 3],
        ],
      ),
      cloud(3, 2, 200, 40),
    ],
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 40],
          [
            [122.5, 100, 1],
            [122.5, 40],
            [300, 40, 3],
          ],
        ),
        cloud(3, 2, 300, 40),
      ]),
    expected: ['G5.notPerpendicular'],
    tolerant: [],
    uid: 2,
    numbers: { endIndex: 0, dx: 0, dy: -60 },
  },
  {
    arm: 'G5.inward',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [110, 200],
          [
            [122.5, 100, 1],
            [110, 100],
            [110, 300, 3],
          ],
        ),
        cloud(3, 2, 110, 300),
      ]),
    expected: ['G5.inward', 'G6.segmentThroughTerminal', 'G6.segmentThroughTerminal'],
    tolerant: [],
    uid: 2,
    numbers: { endIndex: 0, dx: -12.5, dy: 0 },
    why: 'a stub pointing into the stock necessarily crosses its body (segments 0 and 1), so G6 fires too',
  },
  {
    arm: 'G6.segmentThroughTerminal',
    valid: [
      S1,
      flow(
        2,
        [60, 200],
        [
          [122.5, 110, 1],
          [140, 110],
          [140, 60],
          [60, 60],
          [60, 300, 3],
        ],
      ),
      cloud(3, 2, 60, 300),
    ],
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [60, 200],
          [
            [122.5, 110, 1],
            [140, 110],
            [140, 95],
            [60, 95],
            [60, 300, 3],
          ],
        ),
        cloud(3, 2, 60, 300),
      ]),
    expected: ['G6.segmentThroughTerminal'],
    tolerant: [],
    uid: 2,
    numbers: { segment: 2, stockUid: 1 },
  },
  {
    arm: 'G6.cloudInsideStock',
    valid: [
      S1,
      flow(
        10,
        [300, 100],
        [
          [200, 100, 11],
          [400, 100, 12],
        ],
      ),
      cloud(11, 10, 200, 100),
      cloud(12, 10, 400, 100),
    ],
    broken: () =>
      loadView([
        S1,
        flow(
          10,
          [200, 100],
          [
            [100, 100, 11],
            [300, 100, 12],
          ],
        ),
        cloud(11, 10, 100, 100),
        cloud(12, 10, 300, 100),
      ]),
    expected: ['G6.cloudInsideStock'],
    tolerant: [],
    uid: 10,
    numbers: { cloudUid: 11, stockUid: 1 },
  },
  {
    arm: 'G7.cloudOffEndpoint',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 100],
          [
            [122.5, 100, 1],
            [300, 100, 3],
          ],
        ),
        cloud(3, 2, 305, 100),
      ]),
    expected: ['G7.cloudOffEndpoint'],
    tolerant: [],
    uid: 2,
    numbers: { endIndex: 1, distance: 5 },
  },
  {
    arm: 'G8.valveOffPath',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [200, 105],
          [
            [122.5, 100, 1],
            [300, 100, 3],
          ],
        ),
        cloud(3, 2, 300, 100),
      ]),
    expected: ['G8.valveOffPath'],
    tolerant: [],
    uid: 2,
    numbers: { distance: 5 },
  },
  {
    arm: 'G8.valveMargin',
    valid: STRAIGHT,
    broken: () =>
      loadView([
        S1,
        flow(
          2,
          [295, 100],
          [
            [122.5, 100, 1],
            [300, 100, 3],
          ],
        ),
        cloud(3, 2, 300, 100),
      ]),
    expected: ['G8.valveMargin'],
    tolerant: [],
    uid: 2,
    numbers: { arcPosition: 172.5, pathLength: 177.5, margin: 10 },
  },
];

function mapFlow(view: StockFlowView, uid: UID, fn: (f: FlowViewElement) => FlowViewElement): StockFlowView {
  return { ...view, elements: view.elements.map((el) => (el.type === 'flow' && el.uid === uid ? fn(el) : el)) };
}

describe('units', () => {
  // The checker's constants are imported by the generator and the rows, so a
  // changed constant would move every consumer together; this pins the plan's
  // literals independently of all of them.
  it('match the plan', () => {
    expect({
      CORNER_CLEARANCE,
      MIN_SEGMENT,
      VALVE_CLAMP_MARGIN,
      MIN_SINK_SEGMENT,
      PIPE_SPACING,
      GEOMETRY_EPSILON,
    }).toEqual({
      CORNER_CLEARANCE: 3,
      MIN_SEGMENT: 10,
      VALVE_CLAMP_MARGIN: 10,
      MIN_SINK_SEGMENT: 15.5,
      PIPE_SPACING: 10,
      GEOMETRY_EPSILON: 1e-6,
    });
  });
});

describe('checkFlowInvariants arm table', () => {
  it('has exactly one row per enumerated arm', () => {
    expect(sorted(ROWS.map((r) => r.arm))).toEqual(sorted(ALL_FLOW_ARMS));
  });

  for (const row of ROWS) {
    describe(row.arm, () => {
      it('valid fixture passes strict mode', () => {
        const violations = checkFlowInvariants(loadView(row.valid), { mode: 'strict' });
        expect(formatFlowViolations(violations)).toBe('');
      });

      it(`mutation reports exactly ${row.expected.join(', ')}${row.why ? ` (${row.why})` : ''}`, () => {
        const violations = checkFlowInvariants(row.broken(), { mode: 'strict' });
        expect(arms(violations)).toEqual(sorted(row.expected));
        const target = violations.find((v) => v.arm === row.arm);
        expect(target?.uid).toBe(row.uid);
        if (row.numbers !== undefined) {
          for (const [key, value] of Object.entries(row.numbers)) {
            expect(target?.numbers[key]).toBeCloseTo(value, 9);
          }
        }
      });

      it(`tolerant mode reports exactly [${row.tolerant.join(', ')}]`, () => {
        const violations = checkFlowInvariants(row.broken(), { mode: 'tolerant' });
        expect(arms(violations)).toEqual(sorted(row.tolerant));
      });
    });
  }
});

type Face = 'left' | 'right' | 'top' | 'bottom';
const FACES: readonly Face[] = ['left', 'right', 'top', 'bottom'];

// A diagonal first segment that still moves away from the face: the outward
// test must demand an axis-aligned segment, not only the outward sign.
const DIAGONAL_EXIT: Record<Face, { start: [number, number]; end: [number, number] }> = {
  left: { start: [77.5, 100], end: [37.5, 80] },
  right: { start: [122.5, 100], end: [162.5, 80] },
  top: { start: [100, 82.5], end: [80, 42.5] },
  bottom: { start: [100, 117.5], end: [80, 157.5] },
};

// G6's precondition inflates both terminals by MIN_SEGMENT. S1's inflated body
// starts at x = 67.5 and a sink cloud at cx inflated by MIN_SEGMENT ends at
// cx + 10, so cx = 57.5 touches (the precondition holds) and cx = 57.6 overlaps.
// The cloud crowds S1 for G3's room either way, so only G6 can report.
function g6Precondition(cx: number): JsonViewElement[] {
  return [
    S1,
    flow(
      2,
      [100, 95],
      [
        [122.5, 110, 1],
        [140, 110],
        [140, 95],
        [cx, 95],
        [cx, 100, 3],
      ],
    ),
    cloud(3, 2, cx, 100),
  ];
}

// G3's room inflates the source by MIN_SEGMENT (S1 to x = 132.5) and the sink
// by MIN_SINK_SEGMENT (a cloud at cx from cx - 15.5), so cx = 148 touches (room:
// the 10px final segment reports) and cx = 147.9 overlaps (exempt).
function g3Room(cx: number): JsonViewElement[] {
  return [
    S1,
    flow(
      2,
      [135, 100],
      [
        [122.5, 100, 1],
        [cx, 100],
        [cx, 110, 3],
      ],
    ),
    cloud(3, 2, cx, 110),
  ];
}

interface BoundaryRow {
  readonly name: string;
  readonly elements: JsonViewElement[];
  readonly expected: readonly FlowArm[];
}

const BOUNDARY_ROWS: readonly BoundaryRow[] = [
  {
    name: 'G1: uid 0 is non-positive',
    elements: [...STRAIGHT, aux(0, 400, 400)],
    expected: ['G1.nonPositiveUid'],
  },
  {
    name: 'G2: a 1e-3px drift across the axis is diagonal',
    elements: [
      flow(
        10,
        [50, 0.0005],
        [
          [0, 0, 11],
          [100, 0.001, 12],
        ],
      ),
      cloud(11, 10, 0, 0),
      cloud(12, 10, 100, 0.001),
    ],
    expected: ['G2.diagonal'],
  },
  {
    name: 'G3: two consecutive vertical segments are collinear',
    elements: [
      flow(
        10,
        [0, 25],
        [
          [0, 0, 11],
          [0, 50],
          [0, 100, 12],
        ],
      ),
      cloud(11, 10, 0, 0),
      cloud(12, 10, 0, 100),
    ],
    expected: ['G3.collinear'],
  },
  { name: 'G3: room when the inflated terminals exactly touch', elements: g3Room(148), expected: ['G3.shortSink'] },
  { name: 'G3: no room when they overlap by 0.1px', elements: g3Room(147.9), expected: [] },
  {
    name: 'G3: a missing terminal cannot crowd, so the minima still apply',
    elements: [
      S1,
      flow(
        2,
        [160, 100],
        [
          [122.5, 100, 1],
          [200, 100],
          [200, 110],
        ],
      ),
    ],
    expected: ['G1.unattachedEndpoint', 'G3.shortSink'],
  },
  {
    name: 'G3: a final segment of exactly MIN_SINK_SEGMENT is long enough',
    elements: [
      S1,
      flow(
        2,
        [160, 100],
        [
          [122.5, 100, 1],
          [200, 100],
          [200, 115.5, 3],
        ],
      ),
      cloud(3, 2, 200, 115.5),
    ],
    expected: [],
  },
  {
    name: 'G3: a riser of exactly MIN_SEGMENT is long enough',
    elements: [
      flow(
        10,
        [50, 100],
        [
          [0, 100, 11],
          [100, 100],
          [100, 110],
          [200, 110, 12],
        ],
      ),
      cloud(11, 10, 0, 100),
      cloud(12, 10, 200, 110),
    ],
    expected: [],
  },
  {
    name: 'G4: an endpoint on the face line 2.5px past the corner is off the face',
    elements: [
      S1,
      flow(
        2,
        [200, 120],
        [
          [122.5, 120, 1],
          [300, 120, 3],
        ],
      ),
      cloud(3, 2, 300, 120),
    ],
    expected: ['G4.offFace'],
  },
  ...FACES.map((face) => ({
    name: `G5: a diagonal segment moving away from the ${face} face is not an outward exit`,
    elements: [
      S1,
      flow(
        2,
        [
          (DIAGONAL_EXIT[face].start[0] + DIAGONAL_EXIT[face].end[0]) / 2,
          (DIAGONAL_EXIT[face].start[1] + DIAGONAL_EXIT[face].end[1]) / 2,
        ],
        [
          [...DIAGONAL_EXIT[face].start, 1],
          [...DIAGONAL_EXIT[face].end, 3],
        ],
      ),
      cloud(3, 2, ...DIAGONAL_EXIT[face].end),
    ],
    expected: ['G2.diagonal', 'G5.notPerpendicular'] as const,
  })),
  {
    name: 'G6: a segment through the SINK terminal reports',
    elements: [
      S1,
      flow(
        2,
        [60, 200],
        [
          [60, 300, 3],
          [60, 95],
          [140, 95],
          [140, 110],
          [122.5, 110, 1],
        ],
      ),
      cloud(3, 2, 60, 300),
    ],
    expected: ['G6.segmentThroughTerminal'],
  },
  {
    name: 'G6: the precondition holds when the bodies inflated by MIN_SEGMENT exactly touch',
    elements: g6Precondition(57.5),
    expected: ['G6.segmentThroughTerminal'],
  },
  {
    name: 'G6: overlapping by 0.1px exempts the crossing',
    elements: g6Precondition(57.6),
    expected: [],
  },
  {
    name: 'G6: a segment 1.5px inside the body crosses it',
    elements: [
      S1,
      flow(
        2,
        [60, 200],
        [
          [122.5, 110, 1],
          [140, 110],
          [140, 84],
          [60, 84],
          [60, 300, 3],
        ],
      ),
      cloud(3, 2, 60, 300),
    ],
    expected: ['G6.segmentThroughTerminal'],
  },
  {
    name: 'G6: a cloud exactly on a stock edge is not inside it',
    elements: [
      S1,
      flow(
        10,
        [200, 100],
        [
          [122.5, 100, 11],
          [300, 100, 12],
        ],
      ),
      cloud(11, 10, 122.5, 100),
      cloud(12, 10, 300, 100),
    ],
    expected: [],
  },
  {
    name: 'G7: a cloud 1e-3px off its endpoint is off',
    elements: [...STRAIGHT.slice(0, 2), cloud(3, 2, 300.001, 100)],
    expected: ['G7.cloudOffEndpoint'],
  },
  {
    name: 'G8: a valve 1e-3px off the path is off',
    elements: [
      S1,
      flow(
        2,
        [200, 100.001],
        [
          [122.5, 100, 1],
          [300, 100, 3],
        ],
      ),
      cloud(3, 2, 300, 100),
    ],
    expected: ['G8.valveOffPath'],
  },
  {
    name: 'G8: a valve exactly VALVE_CLAMP_MARGIN from an end is far enough',
    elements: [
      flow(
        10,
        [10, 0],
        [
          [0, 0, 11],
          [100, 0, 12],
        ],
      ),
      cloud(11, 10, 0, 0),
      cloud(12, 10, 100, 0),
    ],
    expected: [],
  },
];

describe('checkFlowInvariants boundaries', () => {
  for (const row of BOUNDARY_ROWS) {
    it(`${row.name}: reports [${row.expected.join(', ')}]`, () => {
      expect(arms(checkFlowInvariants(loadView(row.elements), { mode: 'strict' }))).toEqual(sorted(row.expected));
    });
  }
});

// The preconditions and definitional choices each arm relies on. These are the
// places the plan's wording needed a reading; each row states it.
describe('checkFlowInvariants definitions', () => {
  it('G3: crowded terminals exempt the segment minima (12.5px straight flow between stocks)', () => {
    const view = loadView([
      stock(1, 100, 100),
      stock(2, 157.5, 100),
      flow(
        4,
        [128.75, 100],
        [
          [122.5, 100, 1],
          [135, 100, 2],
        ],
      ),
    ]);
    expect(formatFlowViolations(checkFlowInvariants(view, { mode: 'strict' }))).toBe('');
  });

  it('G6: overlapping inflated terminal bodies exempt a body crossing', () => {
    // The G6 row's crossing route, with the sink cloud moved to within MIN_SEGMENT of the stock.
    const view = loadView([
      S1,
      flow(
        2,
        [60, 110],
        [
          [122.5, 110, 1],
          [140, 110],
          [140, 95],
          [60, 95],
          [60, 125, 3],
        ],
      ),
      cloud(3, 2, 60, 125),
    ]);
    expect(formatFlowViolations(checkFlowInvariants(view, { mode: 'strict' }))).toBe('');
  });

  it('G5 is undefined for an off-face endpoint: only G4.offFace reports', () => {
    const view = loadView([
      S1,
      flow(
        2,
        [130, 180],
        [
          [130, 60, 1],
          [130, 300, 3],
        ],
      ),
      cloud(3, 2, 130, 300),
    ]);
    expect(arms(checkFlowInvariants(view, { mode: 'strict' }))).toEqual(['G4.offFace']);
  });

  it('G5 reads the first segment of positive length past a coincident point', () => {
    const view = loadView([
      S1,
      flow(
        2,
        [200, 40],
        [
          [122.5, 100, 1],
          [122.5, 100],
          [122.5, 40],
          [300, 40, 3],
        ],
      ),
      cloud(3, 2, 300, 40),
    ]);
    expect(arms(checkFlowInvariants(view, { mode: 'strict' }))).toEqual(['G3.zeroLength', 'G5.notPerpendicular']);
  });

  it('G4/G5: an endpoint exactly on a corner reports the clearance but not the exit', () => {
    const view = loadView([
      S1,
      flow(
        2,
        [200, 82.5],
        [
          [122.5, 82.5, 1],
          [300, 82.5, 3],
        ],
      ),
      cloud(3, 2, 300, 82.5),
    ]);
    const violations = checkFlowInvariants(view, { mode: 'strict' });
    expect(arms(violations)).toEqual(['G4.cornerClearance']);
    expect(violations[0].numbers.clearance).toBe(0);
  });

  it('G8: a path shorter than two margins has no margin requirement', () => {
    const view = loadView([
      flow(
        10,
        [2, 0],
        [
          [0, 0, 11],
          [15, 0, 12],
        ],
      ),
      cloud(11, 10, 0, 0),
      cloud(12, 10, 15, 0),
    ]);
    expect(formatFlowViolations(checkFlowInvariants(view, { mode: 'strict' }))).toBe('');
  });

  it('G8: a path of exactly two margins already has the requirement', () => {
    // 20px cloud-to-cloud path (the G3 minima are exempt: the clouds crowd each
    // other), valve 5px from the source end.
    const view = loadView([
      flow(
        10,
        [5, 0],
        [
          [0, 0, 11],
          [20, 0, 12],
        ],
      ),
      cloud(11, 10, 0, 0),
      cloud(12, 10, 20, 0),
    ]);
    expect(arms(checkFlowInvariants(view, { mode: 'strict' }))).toEqual(['G8.valveMargin']);
  });

  it('G8: the margin is arc length from the path ends, not from segment ends', () => {
    // Valve 4px past the L's corner: 4px from its segment's start, 81.5px along the path.
    const view = loadView([
      S1,
      flow(
        2,
        [200, 104],
        [
          [122.5, 100, 1],
          [200, 100],
          [200, 250, 3],
        ],
      ),
      cloud(3, 2, 200, 250),
    ]);
    expect(formatFlowViolations(checkFlowInvariants(view, { mode: 'strict' }))).toBe('');
  });

  it('routed: strict arms apply only to routed flows; unrouted flows keep the tolerant arms', () => {
    const view = loadView([
      // Flow 2 (routed): cloud 5px off its endpoint.
      S1,
      flow(
        2,
        [200, 100],
        [
          [122.5, 100, 1],
          [300, 100, 3],
        ],
      ),
      cloud(3, 2, 305, 100),
      // Flow 10 (not routed): the same geometric defect plus a dangling sink.
      flow(
        10,
        [50, 300],
        [
          [0, 300, 11],
          [100, 300, 99],
        ],
      ),
      cloud(11, 10, 3, 300),
    ]);
    const violations = checkFlowInvariants(view, { mode: 'strict', routed: new Set([2]) });
    expect(violations.map((v) => `${v.uid}:${v.arm}`).sort()).toEqual([
      '10:G1.danglingAttachment',
      '2:G7.cloudOffEndpoint',
    ]);
  });

  it('routed: an empty routed set still demands positive uids in a committed view', () => {
    const view = loadView([...STRAIGHT, aux(-3, 400, 400)]);
    expect(arms(checkFlowInvariants(view, { mode: 'strict', routed: new Set() }))).toEqual(['G1.nonPositiveUid']);
    expect(arms(checkFlowInvariants(view, { mode: 'tolerant' }))).toEqual([]);
  });

  it('tolerant mode accepts an unattached flow (Vensim fallback flows import this way)', () => {
    const view = loadView([
      flow(
        10,
        [50, 0],
        [
          [0, 0],
          [100, 0],
        ],
      ),
    ]);
    expect(arms(checkFlowInvariants(view, { mode: 'tolerant' }))).toEqual([]);
    expect(arms(checkFlowInvariants(view, { mode: 'strict' }))).toEqual([
      'G1.unattachedEndpoint',
      'G1.unattachedEndpoint',
    ]);
  });

  it('the tolerant arm set is exactly the structural arms heal cannot repair and imports never carry', () => {
    expect(sorted([...TOLERANT_FLOW_ARMS])).toEqual([
      'G1.attachmentKind',
      'G1.danglingAttachment',
      'G1.foreignCloud',
      'G1.interiorAttached',
      'G1.minPoints',
      'G1.nonFinite',
    ]);
  });
});
