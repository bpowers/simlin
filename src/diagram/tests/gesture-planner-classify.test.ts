// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// classifyPress, latchGesture and isLostRelease as tables.
//
// The arm list is the Canvas's press handling as it stood before the planner
// (handlePointerDown for the empty canvas and the lifecycle, handleSetSelection
// for elements, handleEditConnector, the label components, the module
// double-click), plus the arms docs/design-plans/2026-09-10-diagram-editing-core.md
// adds. Every arm has at least one row, and the rows' outcome kinds cover every
// PressOutcome kind. The audit's press defects are rows too (M7, P-6, P-7).
//
// What this does not establish: that the Canvas hit-tests DOM events into these
// inputs (the canvas-gestures-*.test.tsx suites drive real pointer events).

import { describe, it, expect } from '@rstest/core';

import type { UID } from '@simlin/core/datamodel';

import { classifyPress, isLostRelease, latchGesture, type PressInput, type PressOutcome } from '../gesture-planner';
import {
  linkedAuxes,
  scene,
  stock,
  cloud,
  flow,
  aux,
  link,
  stockToCloud,
  type Scene,
} from './support/gesture-fixtures';

const ARMS = [
  'nameEditor overlay commits the name',
  'presses disabled',
  'second touch pinches',
  'third pointer ignored',
  'second non-touch pointer aborts',
  'press while another pointer gesture is live aborts',
  'module double-click drills in',
  'label double-click edits the name',
  'label double-click read-only selects',
  'label drag',
  'creation tool on canvas stages a draft',
  'flow tool on canvas draws from empty',
  'touch on canvas pans',
  'shift on canvas pans',
  'plain press on canvas rubber-bands',
  'link tool on canvas rubber-bands and stays armed',
  'link tool on a named element draws a link',
  'link tool on an alias draws a link',
  'flow tool on a stock draws a flow',
  'tool on an inapplicable element is cleared',
  'flow arrowhead drags the sink',
  'flow source grip drags the source',
  'link arrowhead drags the link end',
  'modifier press toggles out without a gesture',
  'modifier press toggles in and moves',
  'unselected cloud drags its flow end',
  'sole selected cloud drags its flow end',
  'cloud in a multi-selection moves with it',
  'cloud whose flow is missing moves as an element',
  'selected element defers the single select',
  'unselected element selects and moves',
  'sole link body adjusts the arc',
  'link body in a multi-selection moves',
  'sole flow pipe waits to latch',
  'flow pipe in a multi-selection moves',
  'element missing from the view is ignored',
] as const;

type Arm = (typeof ARMS)[number];

const OUTCOME_KINDS: ReadonlyArray<PressOutcome['kind']> = [
  'ignore',
  'pinch',
  'abort',
  'commitName',
  'drill',
  'editName',
  'select',
  'start',
];

function withAlias(): Scene {
  return scene([
    aux(10, 'a', 100, 300),
    aux(11, 'b', 300, 300),
    { type: 'alias', uid: 15, aliasOfUid: 10, x: 100, y: 450 } as never,
    link(13, 10, 11),
    stock(1, 'S', 100, 100),
    cloud(2, 3, 300, 100),
    flow(3, 'F', { x: 200, y: 100 }, [
      [122.5, 100, 1],
      [300, 100, 2],
    ]),
    { type: 'module', uid: 20, name: 'm', x: 600, y: 600 } as never,
  ]);
}

function press(overrides: Partial<PressInput>): PressInput {
  return {
    view: withAlias().view,
    selection: new Set(),
    tool: undefined,
    hit: { kind: 'canvas' },
    point: { x: 500, y: 500 },
    shiftKey: false,
    toggleKey: false,
    pointerType: 'mouse',
    readOnly: false,
    pressesDisabled: false,
    pointers: 1,
    gestureLive: false,
    ...overrides,
  };
}

const set = (...uids: UID[]): ReadonlySet<UID> => new Set(uids);

interface Row {
  readonly arm: Arm;
  readonly input: PressInput;
  readonly outcome: PressOutcome;
}

const ROWS: readonly Row[] = [
  {
    arm: 'nameEditor overlay commits the name',
    input: press({ hit: { kind: 'nameEditor' }, pressesDisabled: true }),
    outcome: { kind: 'commitName' },
  },
  { arm: 'presses disabled', input: press({ pressesDisabled: true }), outcome: { kind: 'ignore' } },
  {
    arm: 'presses disabled',
    input: press({ pressesDisabled: true, hit: { kind: 'element', uid: 10, part: 'body' } }),
    outcome: { kind: 'ignore' },
  },
  { arm: 'second touch pinches', input: press({ pointerType: 'touch', pointers: 2 }), outcome: { kind: 'pinch' } },
  { arm: 'third pointer ignored', input: press({ pointerType: 'touch', pointers: 3 }), outcome: { kind: 'ignore' } },
  {
    arm: 'second non-touch pointer aborts',
    input: press({ pointerType: 'pen', pointers: 2 }),
    outcome: { kind: 'abort' },
  },
  {
    arm: 'press while another pointer gesture is live aborts',
    input: press({ gestureLive: true }),
    outcome: { kind: 'abort' },
  },
  {
    // Navigation is not an edit: it need not wait for a queued undo.
    arm: 'module double-click drills in',
    input: press({ hit: { kind: 'moduleDoubleClick', uid: 20 }, pressesDisabled: true }),
    outcome: { kind: 'drill', uid: 20 },
  },
  {
    arm: 'label double-click edits the name',
    input: press({ hit: { kind: 'labelDoubleClick', uid: 10 }, tool: 'link', selection: set(10) }),
    outcome: { kind: 'editName', uid: 10, selection: set(10), clearTool: true },
  },
  {
    arm: 'label double-click read-only selects',
    input: press({ hit: { kind: 'labelDoubleClick', uid: 10 }, readOnly: true }),
    outcome: { kind: 'select', selection: set(10), clearTool: false },
  },
  {
    arm: 'label drag',
    input: press({ hit: { kind: 'labelDrag', uid: 11 }, selection: set(10) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'label', uid: 11 },
      selection: set(11),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  ...(['aux', 'stock', 'module'] as const).map(
    (tool): Row => ({
      arm: 'creation tool on canvas stages a draft',
      input: press({ tool, selection: set(10) }),
      outcome: {
        kind: 'start',
        gesture: { kind: 'createElement', type: tool },
        selection: set(),
        clickSelection: undefined,
        clearTool: false,
      },
    }),
  ),
  {
    arm: 'flow tool on canvas draws from empty',
    input: press({ tool: 'flow' }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'createFlow', from: 'empty' },
      selection: undefined,
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'touch on canvas pans',
    input: press({ pointerType: 'touch' }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'pan' },
      selection: undefined,
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'shift on canvas pans',
    input: press({ shiftKey: true }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'pan' },
      selection: undefined,
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'plain press on canvas rubber-bands',
    input: press({ selection: set(10) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'rubberBand' },
      selection: undefined,
      clickSelection: set(),
      clearTool: false,
    },
  },
  {
    arm: 'link tool on canvas rubber-bands and stays armed',
    input: press({ tool: 'link' }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'rubberBand' },
      selection: undefined,
      clickSelection: set(),
      clearTool: false,
    },
  },
  {
    arm: 'link tool on a named element draws a link',
    input: press({ tool: 'link', hit: { kind: 'element', uid: 3, part: 'body' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'createLink', from: 3 },
      selection: undefined,
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'link tool on an alias draws a link',
    input: press({ tool: 'link', hit: { kind: 'element', uid: 15, part: 'body' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'createLink', from: 15 },
      selection: undefined,
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'flow tool on a stock draws a flow',
    input: press({ tool: 'flow', hit: { kind: 'element', uid: 1, part: 'body' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'createFlow', from: { stock: 1 } },
      selection: undefined,
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'tool on an inapplicable element is cleared',
    input: press({ tool: 'flow', hit: { kind: 'element', uid: 10, part: 'body' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: set(10),
      clickSelection: set(10),
      clearTool: true,
    },
  },
  {
    // P-6: the link tool on a cloud clears the tool and drags the flow's end.
    arm: 'tool on an inapplicable element is cleared',
    input: press({ tool: 'link', hit: { kind: 'element', uid: 2, part: 'body' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
      selection: set(3),
      clickSelection: undefined,
      clearTool: true,
    },
  },
  {
    arm: 'flow arrowhead drags the sink',
    input: press({ hit: { kind: 'element', uid: 3, part: 'arrowhead' }, selection: set(10, 3), shiftKey: true }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
      selection: set(3),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'flow source grip drags the source',
    input: press({ hit: { kind: 'element', uid: 3, part: 'source' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'flowEndpoint', flow: 3, end: 'source' },
      selection: set(3),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'link arrowhead drags the link end',
    input: press({ hit: { kind: 'element', uid: 13, part: 'arrowhead' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'linkEndpoint', link: 13 },
      selection: set(13),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    // P-7: a modifier press on a selected element toggles it out and starts no drag.
    arm: 'modifier press toggles out without a gesture',
    input: press({ hit: { kind: 'element', uid: 10, part: 'body' }, selection: set(10, 11), toggleKey: true }),
    outcome: { kind: 'select', selection: set(11), clearTool: false },
  },
  {
    arm: 'modifier press toggles in and moves',
    input: press({ hit: { kind: 'element', uid: 11, part: 'body' }, selection: set(10), shiftKey: true }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: set(10, 11),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'unselected cloud drags its flow end',
    input: press({ hit: { kind: 'element', uid: 2, part: 'body' }, selection: set(10) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
      selection: set(3),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    // M7: a cloud's press does not depend on whether it was already the sole selection.
    arm: 'sole selected cloud drags its flow end',
    input: press({ hit: { kind: 'element', uid: 2, part: 'body' }, selection: set(2) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'flowEndpoint', flow: 3, end: 'sink' },
      selection: set(3),
      clickSelection: undefined,
      clearTool: false,
    },
  },
  {
    arm: 'cloud in a multi-selection moves with it',
    input: press({ hit: { kind: 'element', uid: 2, part: 'body' }, selection: set(2, 1) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: undefined,
      clickSelection: set(2),
      clearTool: false,
    },
  },
  {
    arm: 'cloud whose flow is missing moves as an element',
    input: press({
      view: scene([stock(1, 'S', 100, 100), cloud(2, 99, 300, 100)]).view,
      hit: { kind: 'element', uid: 2, part: 'body' },
    }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: set(2),
      clickSelection: set(2),
      clearTool: false,
    },
  },
  {
    arm: 'selected element defers the single select',
    input: press({ hit: { kind: 'element', uid: 10, part: 'body' }, selection: set(10, 11) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: undefined,
      clickSelection: set(10),
      clearTool: false,
    },
  },
  {
    arm: 'unselected element selects and moves',
    input: press({ hit: { kind: 'element', uid: 1, part: 'body' }, selection: set(10) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: set(1),
      clickSelection: set(1),
      clearTool: false,
    },
  },
  {
    arm: 'sole link body adjusts the arc',
    input: press({ hit: { kind: 'element', uid: 13, part: 'body' } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'linkArc', link: 13 },
      selection: set(13),
      clickSelection: set(13),
      clearTool: false,
    },
  },
  {
    arm: 'link body in a multi-selection moves',
    input: press({ hit: { kind: 'element', uid: 13, part: 'body' }, selection: set(13, 10) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: undefined,
      clickSelection: set(13),
      clearTool: false,
    },
  },
  {
    arm: 'sole flow pipe waits to latch',
    input: press({ hit: { kind: 'element', uid: 3, part: 'body' }, point: { x: 160, y: 101 } }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'pipe', flow: 3, segmentIndex: 0 },
      selection: set(3),
      clickSelection: set(3),
      clearTool: false,
    },
  },
  {
    arm: 'flow pipe in a multi-selection moves',
    input: press({ hit: { kind: 'element', uid: 3, part: 'body' }, selection: set(3, 1) }),
    outcome: {
      kind: 'start',
      gesture: { kind: 'moveSelection' },
      selection: undefined,
      clickSelection: set(3),
      clearTool: false,
    },
  },
  {
    arm: 'element missing from the view is ignored',
    input: press({ hit: { kind: 'element', uid: 999, part: 'body' } }),
    outcome: { kind: 'ignore' },
  },
];

describe('classifyPress', () => {
  it('has a row for every arm and every outcome kind', () => {
    expect(ARMS.filter((arm) => !ROWS.some((r) => r.arm === arm))).toEqual([]);
    expect(OUTCOME_KINDS.filter((kind) => !ROWS.some((r) => r.outcome.kind === kind))).toEqual([]);
  });

  for (const row of ROWS) {
    it(row.arm, () => {
      expect(classifyPress(row.input)).toEqual(row.outcome);
    });
  }

  it('a pipe press picks the segment under the pointer', () => {
    const s = scene([
      stock(1, 'S', 100, 100),
      cloud(2, 3, 300, 300),
      flow(3, 'F', { x: 200, y: 100 }, [
        [122.5, 100, 1],
        [300, 100],
        [300, 300, 2],
      ]),
    ]);
    const at = (point: { x: number; y: number }) =>
      classifyPress(press({ view: s.view, hit: { kind: 'element', uid: 3, part: 'body' }, point }));
    expect(at({ x: 200, y: 102 })).toMatchObject({ gesture: { kind: 'pipe', segmentIndex: 0 } });
    expect(at({ x: 297, y: 250 })).toMatchObject({ gesture: { kind: 'pipe', segmentIndex: 1 } });
  });
});

describe('latchGesture', () => {
  const s = stockToCloud();
  const pipe = { kind: 'pipe', flow: 3, segmentIndex: 0 } as const;
  const latch = (current: { x: number; y: number }, zoom = 1, gesture = pipe as never) =>
    latchGesture(gesture, { view: s.view, press: { x: 200, y: 100 }, current, zoom });

  it.each([
    ['within the threshold: still a pipe press', { x: 203, y: 102 }, pipe],
    [
      'perpendicular-dominant: offset the pressed segment',
      { x: 204, y: 130 },
      { kind: 'offsetSegment', flow: 3, segmentIndex: 0 },
    ],
    ['along-dominant: slide the valve', { x: 240, y: 110 }, { kind: 'slideValve', flow: 3 }],
    ['a diagonal tie slides the valve', { x: 220, y: 120 }, { kind: 'slideValve', flow: 3 }],
  ])('%s', (_name, current, expected) => {
    expect(latch(current)).toEqual(expected);
  });

  it('measures the threshold in screen pixels', () => {
    expect(latch({ x: 200, y: 102 }, 1)).toEqual(pipe);
    expect(latch({ x: 200, y: 102 }, 4)).toEqual({ kind: 'offsetSegment', flow: 3, segmentIndex: 0 });
  });

  it('a flow gone from the view or a stale segment index slides (the planner then finds no subject)', () => {
    expect(latch({ x: 200, y: 140 }, 1, { kind: 'pipe', flow: 999, segmentIndex: 0 } as never)).toEqual({
      kind: 'slideValve',
      flow: 999,
    });
    expect(latch({ x: 200, y: 140 }, 1, { kind: 'pipe', flow: 3, segmentIndex: 4 } as never)).toEqual({
      kind: 'slideValve',
      flow: 3,
    });
  });

  it('every other gesture passes through unchanged', () => {
    const g = { kind: 'moveSelection' } as const;
    expect(latch({ x: 400, y: 400 }, 1, g as never)).toBe(g);
  });
});

describe('isLostRelease', () => {
  it.each([
    ['mouse', 0, true],
    ['mouse', 1, false],
    ['touch', 0, false],
    ['pen', 0, false],
  ] as const)('%s with buttons %d: %s', (pointerType, buttons, expected) => {
    expect(isLostRelease(pointerType, buttons)).toBe(expected);
  });
});

describe('link sources', () => {
  it('a link cannot start at a cloud or a link, so the tool is cleared', () => {
    const s = linkedAuxes();
    expect(
      classifyPress(press({ view: s.view, tool: 'link', hit: { kind: 'element', uid: 13, part: 'body' } })),
    ).toMatchObject({
      kind: 'start',
      gesture: { kind: 'linkArc' },
      clearTool: true,
    });
  });
});
