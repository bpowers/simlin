// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of face attachment and terminals (flow-geometry/terminal.ts): one row
// per face of faceAttachment, each arm of faceOfEndpoint and
// nearestFaceAttachment (including the corner tie-break toward the face the
// adjacent segment is perpendicular to), and each arm of flowTerminals and
// stockTerminal. Views go through the production loader.

import { describe, it, expect } from '@rstest/core';

import type { StockFlowView } from '@simlin/core/datamodel';

import { StockHeight, StockWidth } from '../drawing/default';
import {
  FACES,
  faceAttachment,
  faceOfEndpoint,
  facePoint,
  flowTerminals,
  freeTerminal,
  nearestFaceAttachment,
  stockTerminal,
  stubTip,
  type Face,
} from '../flow-geometry';
import {
  byUidOf,
  cloudJson,
  cloudOf,
  flowJson,
  flowOf,
  loadView,
  stockJson,
  stockOf,
} from './support/flow-geometry-fixtures';

const CLEARANCE = 3;

describe('faceAttachment', () => {
  const stock = { x: 100, y: 100 };
  const lr = { lo: 100 - (StockHeight / 2 - CLEARANCE), hi: 100 + (StockHeight / 2 - CLEARANCE) };
  const tb = { lo: 100 - (StockWidth / 2 - CLEARANCE), hi: 100 + (StockWidth / 2 - CLEARANCE) };
  const ROWS: Readonly<Record<Face, readonly [string, number, number, number, number]>> = {
    left: ['x', -1, 100 - StockWidth / 2, lr.lo, lr.hi],
    right: ['x', 1, 100 + StockWidth / 2, lr.lo, lr.hi],
    top: ['y', -1, 100 - StockHeight / 2, tb.lo, tb.hi],
    bottom: ['y', 1, 100 + StockHeight / 2, tb.lo, tb.hi],
  };
  for (const face of FACES) {
    it(`${face}: normal, sign, plane, the valid range minus CORNER_CLEARANCE, and its stub tips`, () => {
      const att = faceAttachment(stock, face);
      expect([att.normal, att.sign, att.plane, att.lo, att.hi]).toEqual(ROWS[face]);
      expect(stubTip(att, 10)).toBe(ROWS[face][2] + ROWS[face][1] * 10);
      // facePoint clamps into [lo, hi] along the face.
      const far = facePoint(att, 1000);
      expect(att.normal === 'x' ? [far.x, far.y] : [far.y, far.x]).toEqual([ROWS[face][2], ROWS[face][4]]);
    });
  }
});

describe('faceOfEndpoint', () => {
  const s = { x: 0, y: 0 };
  const ROWS: ReadonlyArray<{
    readonly name: string;
    readonly p: [number, number];
    readonly adjacent?: [number, number];
    readonly want: Face | undefined;
  }> = [
    { name: 'right face', p: [22.5, 5], want: 'right' },
    { name: 'left face', p: [-22.5, 5], want: 'left' },
    { name: 'top face', p: [5, -17.5], want: 'top' },
    { name: 'bottom face', p: [5, 17.5], want: 'bottom' },
    { name: 'a corner with a vertical stub: the bottom face', p: [22.5, 17.5], adjacent: [22.5, 40], want: 'bottom' },
    { name: 'a corner with a horizontal stub: the right face', p: [22.5, 17.5], adjacent: [40, 17.5], want: 'right' },
    { name: 'a corner with no adjacent point: the left/right face first', p: [22.5, 17.5], want: 'right' },
    { name: 'off every face (outside)', p: [30, 0], want: undefined },
    { name: 'past a corner along the face line', p: [22.5, 18], want: undefined },
    { name: 'inside the body', p: [0, 0], want: undefined },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const adjacent = row.adjacent === undefined ? undefined : { x: row.adjacent[0], y: row.adjacent[1] };
      expect(faceOfEndpoint(s, { x: row.p[0], y: row.p[1] }, adjacent)).toBe(row.want);
    });
  }
});

describe('nearestFaceAttachment', () => {
  const s = { x: 0, y: 0 };
  const ROWS: ReadonlyArray<{
    readonly name: string;
    readonly p: [number, number];
    readonly adjacent?: [number, number];
    readonly want: [Face, number, number];
  }> = [
    { name: 'outside the right face: its nearest valid point', p: [40, 0], want: ['right', 22.5, 0] },
    { name: 'above the top face', p: [5, -40], want: ['top', 5, -17.5] },
    {
      name: 'a corner, no adjacent: equidistant faces keep the first (right)',
      p: [22.5, 17.5],
      want: ['right', 22.5, 14.5],
    },
    {
      name: 'a corner, vertical adjacent: the tie goes to the bottom face',
      p: [22.5, 17.5],
      adjacent: [22.5, 80],
      want: ['bottom', 19.5, 17.5],
    },
    {
      name: 'a corner, horizontal adjacent: the tie goes to the right face',
      p: [22.5, 17.5],
      adjacent: [80, 17.5],
      want: ['right', 22.5, 14.5],
    },
    {
      name: 'a clear winner is not overridden by the adjacent axis',
      p: [40, 2],
      adjacent: [40, 80],
      want: ['right', 22.5, 2],
    },
  ];
  for (const row of ROWS) {
    it(row.name, () => {
      const adjacent = row.adjacent === undefined ? undefined : { x: row.adjacent[0], y: row.adjacent[1] };
      const got = nearestFaceAttachment(s, { x: row.p[0], y: row.p[1] }, adjacent);
      expect([got.face, got.point.x, got.point.y]).toEqual(row.want);
    });
  }
});

describe('flowTerminals and stockTerminal', () => {
  const view: StockFlowView = loadView([
    stockJson(1, 100, 100),
    cloudJson(3, 10, 300, 100),
    { type: 'aux', uid: 4, name: 'a4', x: 0, y: 0 },
    flowJson(
      10,
      { x: 200, y: 95 },
      [
        { x: 122.5, y: 95 },
        { x: 300, y: 95 },
      ],
      { source: 1, sink: 3 },
    ),
    flowJson(
      20,
      { x: 50, y: 50 },
      [
        { x: 0, y: 50 },
        { x: 100, y: 50 },
      ],
      { source: 99, sink: 4 },
    ),
    flowJson(
      30,
      { x: 50, y: 70 },
      [
        { x: 0, y: 70 },
        { x: 100, y: 70 },
      ],
      {},
    ),
    // A corner endpoint (bottom-right of stock 1) whose stub runs down: the bottom face.
    flowJson(
      40,
      { x: 122.5, y: 160 },
      [
        { x: 122.5, y: 117.5 },
        { x: 122.5, y: 200 },
      ],
      { source: 1 },
    ),
    // The same corner with a stub running right: the right face.
    flowJson(
      50,
      { x: 160, y: 117.5 },
      [
        { x: 122.5, y: 117.5 },
        { x: 200, y: 117.5 },
      ],
      { source: 1 },
    ),
  ]);
  const byUid = byUidOf(view);

  it('a stock endpoint: the base face and the offset from the face center', () => {
    expect(flowTerminals(flowOf(view, 10), byUid).source).toEqual({
      kind: 'stock',
      stock: stockOf(view, 1),
      face: 'right',
      offset: -5,
    });
  });
  it('a stock endpoint at a corner: the face its adjacent segment leaves perpendicular to', () => {
    expect(flowTerminals(flowOf(view, 40), byUid).source).toEqual({
      kind: 'stock',
      stock: stockOf(view, 1),
      face: 'bottom',
      offset: 22.5,
    });
    expect(flowTerminals(flowOf(view, 50), byUid).source).toEqual({
      kind: 'stock',
      stock: stockOf(view, 1),
      face: 'right',
      offset: 17.5,
    });
  });
  it('a cloud endpoint: the cloud, at its center', () => {
    expect(flowTerminals(flowOf(view, 10), byUid).sink).toEqual({
      kind: 'free',
      point: { x: 300, y: 100 },
      cloud: cloudOf(view, 3),
    });
  });
  it('a dangling or non-stock/cloud attachment: a free point with no cloud', () => {
    const t = flowTerminals(flowOf(view, 20), byUid);
    expect([t.source, t.sink]).toEqual([
      { kind: 'free', point: { x: 0, y: 50 } },
      { kind: 'free', point: { x: 100, y: 50 } },
    ]);
  });
  it('an unattached endpoint: a free point with no cloud', () => {
    expect(flowTerminals(flowOf(view, 30), byUid).source).toEqual({ kind: 'free', point: { x: 0, y: 70 } });
  });
  it('stockTerminal reads the base attachment against the base position when a stock moves', () => {
    const stock = stockOf(view, 1);
    const moved = { ...stock, x: 400, y: 400 };
    expect(stockTerminal(moved, { x: 122.5, y: 95 }, { x: 300, y: 95 }, stock)).toEqual({
      kind: 'stock',
      stock: moved,
      face: 'right',
      offset: -5,
    });
    expect(stockTerminal(stock)).toEqual({ kind: 'stock', stock });
    expect(stockTerminal(stock, { x: NaN, y: 0 })).toEqual({ kind: 'stock', stock });
    // An off-face endpoint attaches to the nearest face.
    expect(stockTerminal(stock, { x: 140, y: 101 })).toEqual({ kind: 'stock', stock, face: 'right', offset: 1 });
  });
  it('freeTerminal copies only the coordinates', () => {
    expect(freeTerminal({ x: 1, y: 2, extra: 3 } as { x: number; y: number })).toEqual({
      kind: 'free',
      point: { x: 1, y: 2 },
    });
  });
});
