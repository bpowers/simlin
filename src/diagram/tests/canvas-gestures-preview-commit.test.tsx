// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// E2 in the DOM: for every gesture that commits an edit, the last preview frame
// the Canvas renders is the frame it renders once the commit lands. The harness
// applies each commit the way the controller publishes it (synchronously, the
// committed elements becoming props.view), does not republish on selection
// changes, and its fixtures pin stock endpoints to faces as production data
// does. Rows port the audit's preview-vs-commit probes (P1-P27); the #830 family
// (P14-P17: stock-attached endpoint drags), M4 (P20: a flow drawn from an
// off-center press) and M5 (P27: a link into a flow whose end is dragged) were
// the diverging ones.
//
// The comparison is the content group's markup with the drop-target highlight
// removed (a preview-only cue) and, for a gesture that hands off to a name
// editor, the labels removed (the editor replaces the new element's label).
//
// What this establishes: preview == commit for the edit-committing gesture kinds
// (moveSelection, slideValve, offsetSegment, flowEndpoint, linkEndpoint,
// linkArc, createFlow, createLink, label). Not covered here: createElement
// (its draft becomes a name editor; canvas-gestures-elements.test.tsx checks the
// created element lands at the release point), rubberBand (it commits a
// selection, not a view), and pan (canvas-gestures-pan-zoom.test.tsx).

import { describe, it, expect } from '@rstest/core';

import type { ViewElement } from '@simlin/core/datamodel';

import { GESTURE_KINDS, type Gesture } from '../gesture-planner/types';
import {
  makeAux,
  makeCloud,
  makeFlow,
  makeLink,
  makeStock,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
  type CanvasHarness,
  type HarnessOptions,
} from './canvas-gesture-harness';

type Pt = readonly [number, number];

interface Row {
  readonly name: string;
  readonly kind: Gesture['kind'];
  readonly elements: () => readonly ViewElement[];
  readonly selection?: readonly number[];
  readonly selectedTool?: HarnessOptions['selectedTool'];
  /** The element the press lands on, or the svg itself. */
  readonly target: (h: CanvasHarness) => Element;
  readonly press: Pt;
  readonly moves: readonly Pt[];
  /** The gesture hands off to a name editor, whose overlay replaces the new element's label. */
  readonly handoff?: boolean;
}

// Stock (100,100) -> cloud (300,100), the source on the stock's right face.
function stockCloud(): ViewElement[] {
  return [
    makeStock(1, 'stock', 100, 100),
    makeCloud(2, 3, 300, 100),
    makeFlow(
      3,
      'flow',
      [
        { x: 122.5, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: 2 },
      ],
      { x: 200, y: 100 },
    ),
  ];
}

// Stock src (100,100) -> stock dst (400,100), both endpoints on faces.
function stockStock(): ViewElement[] {
  return [
    makeStock(1, 'src', 100, 100),
    makeStock(2, 'dst', 400, 100),
    makeFlow(
      3,
      'flow',
      [
        { x: 122.5, y: 100, attachedToUid: 1 },
        { x: 377.5, y: 100, attachedToUid: 2 },
      ],
      { x: 250, y: 100 },
    ),
  ];
}

const svg = (h: CanvasHarness): Element => h.svg;
const first = (selector: string) => (h: CanvasHarness) => h.query(selector)!;
const nth = (selector: string, i: number) => (h: CanvasHarness) => h.queryAll(selector)[i];
const valve = (h: CanvasHarness): Element => h.query('g.simlin-flow circle')!.parentElement!;
const sourceGrip = first('g.simlin-flow rect[fill="transparent"]');

const ROWS: readonly Row[] = [
  {
    name: 'P1 a single aux moved',
    kind: 'moveSelection',
    elements: () => [makeAux(10, 'a', 100, 100)],
    target: first('g.simlin-aux'),
    press: [100, 100],
    moves: [
      [130, 120],
      [160, 140],
    ],
  },
  {
    name: 'P2 a group with a stock and its attached flow, pressed on the selected aux',
    kind: 'moveSelection',
    elements: () => [...stockCloud(), makeAux(10, 'a', 200, 250)],
    selection: [1, 10],
    target: first('g.simlin-aux'),
    press: [200, 250],
    moves: [
      [230, 270],
      [260, 290],
    ],
  },
  {
    name: 'P3 a stock moved across its flow routes the flow',
    kind: 'moveSelection',
    elements: stockCloud,
    target: first('g.simlin-stock'),
    press: [100, 100],
    moves: [
      [100, 130],
      [100, 160],
    ],
  },
  {
    name: 'P4 a valve slid along its pipe',
    kind: 'slideValve',
    elements: stockCloud,
    target: valve,
    press: [200, 100],
    moves: [
      [220, 101],
      [240, 101],
    ],
  },
  {
    name: 'P5 a stock -> cloud pipe offset perpendicular',
    kind: 'offsetSegment',
    elements: stockCloud,
    target: valve,
    press: [200, 100],
    moves: [
      [200, 130],
      [202, 160],
    ],
  },
  {
    name: 'P6 a stock -> stock pipe offset into a bracket (#819)',
    kind: 'offsetSegment',
    elements: stockStock,
    target: valve,
    press: [250, 100],
    moves: [
      [250, 130],
      [251, 160],
    ],
  },
  {
    name: 'P7 an interior segment offset',
    kind: 'offsetSegment',
    elements: () => [
      makeStock(1, 'stock', 100, 100),
      makeCloud(2, 3, 500, 300),
      makeFlow(
        3,
        'flow',
        [
          { x: 122.5, y: 100, attachedToUid: 1 },
          { x: 300, y: 100, attachedToUid: undefined },
          { x: 300, y: 300, attachedToUid: undefined },
          { x: 500, y: 300, attachedToUid: 2 },
        ],
        { x: 300, y: 200 },
      ),
    ],
    target: first('path.simlin-outer'),
    press: [300, 260],
    moves: [
      [320, 261],
      [340, 261],
    ],
  },
  {
    name: 'P9 a sink cloud dragged along its axis',
    kind: 'flowEndpoint',
    elements: stockCloud,
    target: first('path.simlin-cloud'),
    press: [300, 100],
    moves: [
      [350, 100],
      [400, 100],
    ],
  },
  {
    name: 'P10 a sink cloud dragged perpendicular',
    kind: 'flowEndpoint',
    elements: stockCloud,
    target: first('path.simlin-cloud'),
    press: [300, 100],
    moves: [
      [300, 150],
      [300, 200],
    ],
  },
  {
    name: 'P11 a source cloud dragged along its axis',
    kind: 'flowEndpoint',
    elements: () => [
      makeStock(1, 'stock', 300, 100),
      makeCloud(2, 3, 100, 100),
      makeFlow(
        3,
        'flow',
        [
          { x: 100, y: 100, attachedToUid: 2 },
          { x: 277.5, y: 100, attachedToUid: 1 },
        ],
        { x: 188.75, y: 100 },
      ),
    ],
    target: sourceGrip,
    press: [110, 100],
    moves: [
      [80, 100],
      [50, 100],
    ],
  },
  {
    name: 'P12 a sink cloud dropped on an aligned stock',
    kind: 'flowEndpoint',
    elements: () => [...stockCloud(), makeStock(4, 'target', 450, 100)],
    target: first('path.simlin-cloud'),
    press: [300, 100],
    moves: [
      [400, 100],
      [450, 100],
    ],
  },
  {
    name: 'P13 a sink cloud dropped on a stock below the pipe (H5: it lands on that stock`s face)',
    kind: 'flowEndpoint',
    elements: () => [...stockCloud(), makeStock(4, 'target', 130, 250)],
    target: first('path.simlin-cloud'),
    press: [300, 100],
    moves: [
      [200, 200],
      [130, 250],
    ],
  },
  {
    name: 'P14 #830: a stock-attached sink dragged along its axis into empty space',
    kind: 'flowEndpoint',
    elements: stockStock,
    target: first('path.simlin-arrowhead-flow'),
    press: [372, 100],
    moves: [
      [340, 100],
      [300, 100],
    ],
  },
  {
    name: 'P15 #830: a stock-attached sink dragged perpendicular',
    kind: 'flowEndpoint',
    elements: stockStock,
    target: first('path.simlin-arrowhead-flow'),
    press: [372, 100],
    moves: [
      [372, 150],
      [372, 200],
    ],
  },
  {
    name: 'P16 #830: a stock-attached sink dropped on another stock',
    kind: 'flowEndpoint',
    elements: () => [...stockStock(), makeStock(4, 'other', 600, 100)],
    target: first('path.simlin-arrowhead-flow'),
    press: [372, 100],
    moves: [
      [500, 100],
      [600, 100],
    ],
  },
  {
    name: 'P17 #830: a stock-attached source dragged perpendicular',
    kind: 'flowEndpoint',
    elements: stockCloud,
    target: sourceGrip,
    press: [132, 100],
    moves: [
      [132, 150],
      [132, 200],
    ],
  },
  {
    name: 'P27 M5: a link into a flow whose sink cloud is dragged',
    kind: 'flowEndpoint',
    elements: () => [...stockCloud(), makeAux(20, 'x', 200, 20), makeLink(21, 20, 3, 30)],
    target: first('path.simlin-cloud'),
    press: [300, 100],
    moves: [
      [300, 160],
      [360, 200],
    ],
  },
  {
    name: 'P18 the flow tool from empty space into empty space',
    kind: 'createFlow',
    elements: () => [makeAux(9, 'unrelated', 700, 700)],
    selectedTool: 'flow',
    target: svg,
    press: [200, 200],
    moves: [
      [260, 200],
      [320, 200],
    ],
    handoff: true,
  },
  {
    name: 'P19 the flow tool from empty space onto a stock',
    kind: 'createFlow',
    elements: () => [makeStock(4, 'target', 400, 200)],
    selectedTool: 'flow',
    target: svg,
    press: [200, 200],
    moves: [
      [300, 200],
      [400, 200],
    ],
    handoff: true,
  },
  {
    name: 'P20 M4: the flow tool from an off-center press on a stock into empty space',
    kind: 'createFlow',
    elements: () => [makeStock(1, 'src', 100, 100)],
    selectedTool: 'flow',
    target: first('g.simlin-stock'),
    press: [115, 95],
    moves: [
      [200, 95],
      [300, 95],
    ],
    handoff: true,
  },
  {
    name: 'P21 the flow tool from a stock onto another stock',
    kind: 'createFlow',
    elements: () => [makeStock(1, 'src', 100, 100), makeStock(2, 'dst', 400, 100)],
    selectedTool: 'flow',
    target: nth('g.simlin-stock', 0),
    press: [115, 95],
    moves: [
      [250, 95],
      [400, 95],
    ],
    handoff: true,
  },
  {
    name: 'P22 the link tool from one aux to another, curving through the release',
    kind: 'createLink',
    elements: () => [makeAux(1, 'a', 100, 100), makeAux(2, 'b', 300, 100)],
    selectedTool: 'link',
    target: nth('g.simlin-aux', 0),
    press: [100, 100],
    moves: [
      [200, 130],
      [300, 106],
    ],
  },
  {
    name: 'P23 a link arrowhead reattached to another aux',
    kind: 'linkEndpoint',
    elements: () => [
      makeAux(1, 'a', 100, 100),
      makeAux(2, 'b', 300, 100),
      makeAux(3, 'c', 300, 300),
      makeLink(4, 1, 2),
    ],
    target: first('path.simlin-arrowhead-link'),
    press: [289, 100],
    moves: [
      [300, 200],
      [303, 296],
    ],
  },
  {
    name: 'P24 a link curved by its body',
    kind: 'linkArc',
    elements: () => [makeAux(1, 'a', 100, 100), makeAux(2, 'b', 300, 100), makeLink(4, 1, 2)],
    target: first('path.simlin-connector'),
    press: [200, 100],
    moves: [
      [200, 120],
      [200, 140],
    ],
  },
  {
    name: 'P25 a label dragged to the bottom',
    kind: 'label',
    elements: () => [makeAux(10, 'a', 100, 100)],
    target: first('g.simlin-aux text'),
    press: [130, 100],
    moves: [
      [120, 130],
      [100, 160],
    ],
  },
];

function markup(h: CanvasHarness, stripLabels: boolean): string {
  let html = h.query('svg g[transform]')?.innerHTML ?? '';
  html = html.replace(/ ?targetGood| ?targetBad/g, '');
  if (stripLabels) {
    html = html.replace(/<g><text[\s\S]*?<\/text><\/g>/g, '');
  }
  return html;
}

describe('Canvas: the last preview frame is the committed frame (E2)', () => {
  it('covers every edit-committing gesture kind', () => {
    const notCommitting = new Set<Gesture['kind']>(['createElement', 'rubberBand', 'pan']);
    const covered = new Set(ROWS.map((r) => r.kind));
    expect(GESTURE_KINDS.filter((k) => !notCommitting.has(k) && !covered.has(k))).toEqual([]);
  });

  for (const row of ROWS) {
    it(`${row.kind}: ${row.name}`, () => {
      const h = renderCanvas({
        elements: row.elements(),
        selection: new Set(row.selection ?? []),
        selectedTool: row.selectedTool,
      });
      h.clearMountCalls();

      const target = row.target(h);
      pointerDown(target, row.press[0], row.press[1]);
      for (const [x, y] of row.moves) {
        pointerMove(row.kind === 'label' ? target : h.svg, x, y, { buttons: 1 });
      }
      const [x, y] = row.moves[row.moves.length - 1];
      const preview = markup(h, !!row.handoff);
      pointerUp(h.svg, x, y);

      expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
      expect(markup(h, !!row.handoff)).toBe(preview);
    });
  }
});
