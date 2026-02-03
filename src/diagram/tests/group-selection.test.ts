// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { Point, FlowViewElement, StockViewElement, AuxViewElement } from '@system-dynamics/core/datamodel';

import { StockWidth } from '../drawing/Stock';

// Helper functions to create test elements
function makeStock(
  uid: number,
  x: number,
  y: number,
  inflows: number[] = [],
  outflows: number[] = [],
): StockViewElement {
  return new StockViewElement({
    uid,
    name: `Stock${uid}`,
    ident: `stock_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
    inflows: List(inflows),
    outflows: List(outflows),
  });
}

function makeFlow(
  uid: number,
  x: number,
  y: number,
  points: Array<{ x: number; y: number; attachedToUid?: number }>,
): FlowViewElement {
  return new FlowViewElement({
    uid,
    name: `Flow${uid}`,
    ident: `flow_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    points: List(points.map((p) => new Point({ x: p.x, y: p.y, attachedToUid: p.attachedToUid }))),
    isZeroRadius: false,
  });
}

function makeAux(uid: number, x: number, y: number): AuxViewElement {
  return new AuxViewElement({
    uid,
    name: `Aux${uid}`,
    ident: `aux_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
  });
}

// Helper to check if element center is within selection rectangle
function isInSelectionRect(
  element: { cx: number; cy: number },
  left: number,
  right: number,
  top: number,
  bottom: number,
): boolean {
  return element.cx >= left && element.cx <= right && element.cy >= top && element.cy <= bottom;
}

describe('Group Selection', () => {
  describe('Drag selection should include stocks', () => {
    it('should select a stock when its center is within the drag rectangle', () => {
      const stock = makeStock(1, 100, 100);
      const left = 50;
      const right = 150;
      const top = 50;
      const bottom = 150;

      const inRect = isInSelectionRect(stock, left, right, top, bottom);
      expect(inRect).toBe(true);
    });

    it('should not select a stock when its center is outside the drag rectangle', () => {
      const stock = makeStock(1, 200, 200);
      const left = 50;
      const right = 150;
      const top = 50;
      const bottom = 150;

      const inRect = isInSelectionRect(stock, left, right, top, bottom);
      expect(inRect).toBe(false);
    });

    it('should select multiple stocks when their centers are within the drag rectangle', () => {
      const stock1 = makeStock(1, 100, 100);
      const stock2 = makeStock(2, 120, 120);
      const stock3 = makeStock(3, 200, 200); // outside
      const left = 50;
      const right = 150;
      const top = 50;
      const bottom = 150;

      const selectedUids: number[] = [];
      for (const stock of [stock1, stock2, stock3]) {
        if (isInSelectionRect(stock, left, right, top, bottom)) {
          selectedUids.push(stock.uid);
        }
      }

      expect(selectedUids).toEqual([1, 2]);
    });
  });

  describe('Drag selection should include flows', () => {
    it('should select a flow when its valve center is within the drag rectangle', () => {
      // Flow with valve at (150, 100)
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 3 },
      ]);
      const left = 100;
      const right = 200;
      const top = 50;
      const bottom = 150;

      const inRect = isInSelectionRect(flow, left, right, top, bottom);
      expect(inRect).toBe(true);
    });

    it('should not select a flow when its valve center is outside the drag rectangle', () => {
      // Flow with valve at (150, 100)
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 3 },
      ]);
      const left = 200;
      const right = 300;
      const top = 50;
      const bottom = 150;

      const inRect = isInSelectionRect(flow, left, right, top, bottom);
      expect(inRect).toBe(false);
    });
  });

  describe('Drag selection should include auxes', () => {
    it('should select an aux when its center is within the drag rectangle', () => {
      const aux = makeAux(1, 100, 100);
      const left = 50;
      const right = 150;
      const top = 50;
      const bottom = 150;

      const inRect = isInSelectionRect(aux, left, right, top, bottom);
      expect(inRect).toBe(true);
    });
  });

  describe('Mixed element selection', () => {
    it('should select stocks, flows, and auxes all within the same drag rectangle', () => {
      const stock = makeStock(1, 100, 100);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 3 },
      ]);
      const aux = makeAux(4, 120, 80);

      const left = 50;
      const right = 200;
      const top = 50;
      const bottom = 150;

      const selectedUids: number[] = [];
      for (const element of [stock, flow, aux]) {
        if (isInSelectionRect(element, left, right, top, bottom)) {
          selectedUids.push(element.uid);
        }
      }

      expect(selectedUids).toEqual([1, 2, 4]);
    });
  });
});
