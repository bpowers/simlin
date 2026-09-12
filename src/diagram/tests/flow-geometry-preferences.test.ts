// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The routing preferences and tie rules of flow-geometry.ts that no invariant
// forces: the constants (pinned against literals and against the checker's own
// literals, so drift in any of the three is caught), the PIPE_SPACING slot a
// flow newly landing on a face takes, and length ties within GEOMETRY_EPSILON.

import { describe, it, expect } from '@rstest/core';

import type { FlowViewElement } from '@simlin/core/datamodel';

import { FlowArrowheadRadius } from '../drawing/default';
import {
  CORNER_CLEARANCE,
  flowTerminals,
  freeTerminal,
  GEOMETRY_EPSILON,
  MIN_SEGMENT,
  MIN_SINK_SEGMENT,
  PIPE_SPACING,
  route,
  stockTerminal,
  VALVE_CLAMP_MARGIN,
  type XY,
} from '../flow-geometry';
import * as checker from './support/flow-invariants';
import {
  applyGeometry,
  byUidOf,
  flowJson,
  flowOf,
  fmtFlow,
  loadView,
  stockJson,
  stockOf,
  strictReport,
} from './support/flow-geometry-fixtures';

describe('constants', () => {
  it('match the plan`s units and the checker`s independently pinned literals', () => {
    const core = {
      CORNER_CLEARANCE,
      MIN_SEGMENT,
      VALVE_CLAMP_MARGIN,
      MIN_SINK_SEGMENT,
      PIPE_SPACING,
      GEOMETRY_EPSILON,
    };
    expect(core).toEqual({
      CORNER_CLEARANCE: 3,
      MIN_SEGMENT: 10,
      VALVE_CLAMP_MARGIN: 10,
      MIN_SINK_SEGMENT: FlowArrowheadRadius + 7.5,
      PIPE_SPACING: 10,
      GEOMETRY_EPSILON: 1e-6,
    });
    expect(core).toEqual({
      CORNER_CLEARANCE: checker.CORNER_CLEARANCE,
      MIN_SEGMENT: checker.MIN_SEGMENT,
      VALVE_CLAMP_MARGIN: checker.VALVE_CLAMP_MARGIN,
      MIN_SINK_SEGMENT: checker.MIN_SINK_SEGMENT,
      PIPE_SPACING: checker.PIPE_SPACING,
      GEOMETRY_EPSILON: checker.GEOMETRY_EPSILON,
    });
  });
});

describe('the PIPE_SPACING slot of a flow newly landing on a face', () => {
  // Stock S at the origin; the pointer up and to the right, where the best
  // route is an L out of S's right face (valid range y in [-14.5, 14.5]) and the
  // endpoint's position along the face is the slot preference.
  const view = loadView([stockJson(1, 0, 0)]);
  const draft = flowOf(loadView([flowJson(10, { x: 0, y: 0 }, [], {})]), 10);
  const S = stockOf(view, 1);
  const on = (y: number): XY => ({ x: 22.5, y });

  const ROWS: ReadonlyArray<{ readonly name: string; readonly occupied: XY[]; readonly want: number }> = [
    { name: 'an empty face: the center', occupied: [], want: 0 },
    { name: 'the center taken: the nearest spaced slot', occupied: [on(0)], want: -10 },
    { name: 'two taken: still the nearest spaced slot', occupied: [on(0), on(10)], want: -10 },
    {
      name: 'no spaced slot left: the position farthest from its nearest neighbor',
      occupied: [on(-14.5), on(-5), on(5), on(14.5)],
      want: 0,
    },
    {
      name: 'endpoints on other faces do not count',
      occupied: [
        { x: 0, y: -17.5 },
        { x: -22.5, y: 0 },
      ],
      want: 0,
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const g = route(stockTerminal(S), freeTerminal({ x: 60, y: -100 }), { flow: draft, occupied: row.occupied });
      expect(`${fmtFlow(g.flow)}`).toBe(
        fmtFlow({
          ...g.flow,
          points: [
            { x: 22.5, y: row.want, attachedToUid: 1 },
            { x: 60, y: row.want, attachedToUid: undefined },
            { x: 60, y: -100, attachedToUid: undefined },
          ],
        }),
      );
    });
  }

  it('no spaced slot left, asymmetric: the widest gap, not the center', () => {
    // The widest gap's midpoint (nearest neighbor 5.5 away) is not the center
    // (nearest neighbor 2 away). The pointer is down and to the right, where the
    // right face's L stays the shortest route with the endpoint below the center.
    const g = route(stockTerminal(S), freeTerminal({ x: 60, y: 100 }), {
      flow: draft,
      occupied: [on(-14), on(-6), on(2), on(13)],
    });
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [22.5, 7.5],
      [60, 7.5],
      [60, 100],
    ]);
  });

  it('a straight route is exempt: its endpoint aligns with the pointer whatever is occupied', () => {
    const g = route(stockTerminal(S), freeTerminal({ x: 100, y: 3 }), { flow: draft, occupied: [on(3)] });
    expect(g.flow.points.map((p) => [p.x, p.y])).toEqual([
      [22.5, 3],
      [100, 3],
    ]);
  });
});

describe('length ties within GEOMETRY_EPSILON', () => {
  it('a Z riser holds its base corner frame after frame instead of flipping on float noise', () => {
    // A Z from stock A's right face into stock B's left face, rerouted with
    // route() while B is dragged in non-integer steps. Stickiness keeps both base
    // faces (an off-base L is only one bend better), and every Z joining them has
    // the same length whatever its riser holds, up to float noise in the summed
    // segments; the base corner's hold (x = 100) is the first candidate and must
    // win every frame.
    const view = loadView([
      stockJson(1, 0, 0),
      stockJson(2, 200, 60),
      flowJson(
        10,
        { x: 60, y: 0 },
        [
          { x: 22.5, y: 0 },
          { x: 100, y: 0 },
          { x: 100, y: 60 },
          { x: 177.5, y: 60 },
        ],
        { source: 1, sink: 2 },
      ),
    ]);
    const f: FlowViewElement = flowOf(view, 10);
    const t = flowTerminals(f, byUidOf(view));
    const B = stockOf(view, 2);
    const holds: string[] = [];
    let zFrames = 0;
    for (let k = 1; k <= 96; k++) {
      const moved = { ...B, x: 200 + (6.4234917 * k) / 96, y: 60 + (44.8912337 * k) / 96 };
      const g = route(t.source, stockTerminal(moved, f.points[3], f.points[2], B), { flow: f });
      expect(strictReport(applyGeometry(view, g, [moved]), [10])).toBe('');
      if (g.flow.points.length === 4) {
        zFrames++;
        holds.push(`k ${k}: ${g.flow.points[1].x}`);
      }
    }
    expect(zFrames).toBeGreaterThan(20);
    expect(holds.filter((h) => !h.endsWith(': 100'))).toEqual([]);
  });
});
