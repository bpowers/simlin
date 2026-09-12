// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A drag frame re-renders only the elements whose props change. The element
// components are memo'd, which holds only while every callback the Canvas hands
// them keeps its identity across renders. Each drawing component's inner render
// function is swapped for a counting wrapper before the Canvas mounts; the memo
// wrapper and its default shallow comparison stay the production ones, so a
// render the memo skips is a render the counter does not record.
//
// What this does not establish: how long a frame takes on a large model (the
// C-LEARN timing is measured outside the suite), or a module's double-click
// handler, which takes the same stable-callback path but is not rendered here.

import { describe, it, expect, afterAll } from '@rstest/core';

import * as React from 'react';

import { Aux } from '../drawing/Auxiliary';
import { Connector } from '../drawing/Connector';
import { Flow } from '../drawing/Flow';
import { Stock } from '../drawing/Stock';
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
} from './canvas-gesture-harness';

const renders: number[] = [];

type MemoObject = { type: (props: { element: { uid: number } }) => React.ReactNode };
const restores: Array<() => void> = [];
for (const component of [Aux, Stock, Flow, Connector] as unknown as MemoObject[]) {
  const inner = component.type;
  component.type = function Counted(props) {
    renders.push(props.element.uid);
    return inner(props);
  };
  restores.push(() => {
    component.type = inner;
  });
}

afterAll(() => {
  for (const restore of restores) {
    restore();
  }
});

describe('Canvas drag frames re-render only what they change', () => {
  it('a drag frame re-renders the dragged aux and no unchanged aux, stock, flow or link', () => {
    const h = renderCanvas({
      elements: [
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
        makeAux(10, 'a', 100, 400),
        makeAux(11, 'b', 400, 400),
        makeLink(20, 11, 1),
      ],
    });
    h.clearMountCalls();
    // Every counted component rendered at mount, so the counters are live.
    expect(new Set(renders)).toEqual(new Set([1, 3, 10, 11, 20]));

    pointerDown(h.queryAll('g.simlin-aux')[0], 100, 400);
    pointerMove(h.svg, 150, 420, { buttons: 1 });
    renders.length = 0;
    pointerMove(h.svg, 170, 430, { buttons: 1 });
    expect(renders).toContain(10);
    expect(renders.filter((uid) => uid !== 10)).toEqual([]);
    pointerUp(h.svg, 170, 430);
  });
});
