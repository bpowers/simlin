// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { Point, FlowViewElement, StockViewElement } from '@system-dynamics/core/datamodel';

import { computeFlowRoute, UpdateStockAndFlows, UpdateFlow, moveSegment } from '../drawing/Flow';
import { StockWidth, StockHeight } from '../drawing/Stock';

function makeStock(uid: number, x: number, y: number, inflows: number[] = [], outflows: number[] = []): StockViewElement {
  return new StockViewElement({
    uid,
    name: 'TestStock',
    ident: 'test_stock',
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
    name: 'TestFlow',
    ident: 'test_flow',
    var: undefined,
    x,
    y,
    labelSide: 'center',
    points: List(points.map((p) => new Point({ x: p.x, y: p.y, attachedToUid: p.attachedToUid }))),
    isZeroRadius: false,
  });
}

describe('Flow routing', () => {
  const stockUid = 1;
  const flowUid = 2;
  const cloudUid = 3;

  describe('computeFlowRoute', () => {
    describe('straight horizontal flows', () => {
      it('should keep flow straight when stock moves within vertical bounds', () => {
        // Stock at (100, 100), anchor/cloud at (200, 100) - horizontal flow
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 200, y: 100, attachedToUid: cloudUid },
        ]);

        // Move stock slightly up (within StockHeight/2 = 17.5)
        const newStockY = 100 - 10;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Should still be 2 points (straight)
        expect(result.points.size).toBe(2);
        // Stock point should be at stock's right edge, at anchor's Y
        const stockPoint = result.points.first()!;
        expect(stockPoint.x).toBe(100 + StockWidth / 2);
        expect(stockPoint.y).toBe(100); // anchor Y
      });

      it('should create L-shape when stock moves vertically beyond bounds', () => {
        // Stock at (100, 100), anchor/cloud at (200, 100) - horizontal flow
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 200, y: 100, attachedToUid: cloudUid },
        ]);

        // Move stock down significantly (beyond StockHeight/2 = 17.5)
        const newStockY = 100 + 50;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Should be 3 points (L-shape)
        expect(result.points.size).toBe(3);

        const stockPoint = result.points.get(0)!;
        const corner = result.points.get(1)!;
        const anchor = result.points.get(2)!;

        // Stock should attach at TOP (since anchor is above)
        expect(stockPoint.x).toBe(100);
        expect(stockPoint.y).toBe(newStockY - StockHeight / 2);

        // Corner should create vertical-then-horizontal L
        expect(corner.x).toBe(stockPoint.x); // same X as stock (vertical segment)
        expect(corner.y).toBe(anchor.y); // same Y as anchor (horizontal segment)

        // Anchor unchanged
        expect(anchor.x).toBe(200);
        expect(anchor.y).toBe(100);
      });

      it('should attach to bottom when stock moves up (anchor below)', () => {
        // Stock at (100, 100), anchor/cloud at (200, 100) - horizontal flow
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 200, y: 100, attachedToUid: cloudUid },
        ]);

        // Move stock up significantly
        const newStockY = 100 - 50;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        expect(result.points.size).toBe(3);

        const stockPoint = result.points.get(0)!;
        // Stock should attach at BOTTOM (since anchor is below)
        expect(stockPoint.y).toBe(newStockY + StockHeight / 2);
      });
    });

    describe('straight vertical flows', () => {
      it('should keep flow straight when stock moves within horizontal bounds', () => {
        // Stock at (100, 100), anchor/cloud at (100, 200) - vertical flow
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 100, 150, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid },
          { x: 100, y: 200, attachedToUid: cloudUid },
        ]);

        // Move stock slightly right (within StockWidth/2 = 22.5)
        const newStockX = 100 + 10;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // Should still be 2 points (straight)
        expect(result.points.size).toBe(2);
      });

      it('should create L-shape when stock moves horizontally beyond bounds', () => {
        // Stock at (100, 100), anchor/cloud at (100, 200) - vertical flow
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 100, 150, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid },
          { x: 100, y: 200, attachedToUid: cloudUid },
        ]);

        // Move stock right significantly (beyond StockWidth/2 = 22.5)
        const newStockX = 100 + 50;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // Should be 3 points (L-shape)
        expect(result.points.size).toBe(3);

        const stockPoint = result.points.get(0)!;
        const corner = result.points.get(1)!;
        const anchor = result.points.get(2)!;

        // Stock should attach at LEFT (since anchor is to the left)
        expect(stockPoint.x).toBe(newStockX - StockWidth / 2);
        expect(stockPoint.y).toBe(100);

        // Corner should create horizontal-then-vertical L
        expect(corner.x).toBe(anchor.x); // same X as anchor (vertical segment)
        expect(corner.y).toBe(stockPoint.y); // same Y as stock (horizontal segment)
      });
    });

    describe('L-shaped flows maintain direction', () => {
      it('should preserve horizontal anchor segment direction for existing L-shape', () => {
        // Existing L-shaped flow: stock at top, corner in middle, anchor at right
        // This represents a flow that was originally horizontal and bent
        const stock = makeStock(stockUid, 100, 50);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 100, y: 50 + StockHeight / 2, attachedToUid: stockUid }, // stock bottom
          { x: 100, y: 100 }, // corner
          { x: 200, y: 100, attachedToUid: cloudUid }, // anchor
        ]);

        // Move stock further up - anchor segment (corner-anchor) is horizontal
        const newStockY = 30;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        expect(result.points.size).toBe(3);

        // The anchor-side segment should remain horizontal
        const corner = result.points.get(1)!;
        const anchor = result.points.get(2)!;
        expect(corner.y).toBe(anchor.y); // horizontal segment preserved
      });

      it('should preserve vertical anchor segment direction for existing L-shape', () => {
        // Existing L-shaped flow: stock at right, corner in middle, anchor at bottom
        // This represents a flow that was originally vertical and bent
        const stock = makeStock(stockUid, 150, 100);
        const flow = makeFlow(flowUid, 125, 150, [
          { x: 150 - StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock left
          { x: 100, y: 100 }, // corner
          { x: 100, y: 200, attachedToUid: cloudUid }, // anchor
        ]);

        // Move stock further right - anchor segment (corner-anchor) is vertical
        const newStockX = 180;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        expect(result.points.size).toBe(3);

        // The anchor-side segment should remain vertical
        const corner = result.points.get(1)!;
        const anchor = result.points.get(2)!;
        expect(corner.x).toBe(anchor.x); // vertical segment preserved
      });

      it('should revert L-shape to straight when stock returns to valid position', () => {
        // Existing L-shaped flow from horizontal original
        const stock = makeStock(stockUid, 100, 150);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 100, y: 150 - StockHeight / 2, attachedToUid: stockUid }, // stock top
          { x: 100, y: 100 }, // corner
          { x: 200, y: 100, attachedToUid: cloudUid }, // anchor
        ]);

        // Move stock back to anchor's Y level (within bounds)
        const newStockY = 100;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Should revert to 2 points (straight)
        expect(result.points.size).toBe(2);

        const stockPoint = result.points.get(0)!;
        const anchor = result.points.get(1)!;

        // Should be a straight horizontal flow again
        expect(stockPoint.y).toBe(anchor.y);
      });
    });

    describe('stock as sink (last point)', () => {
      it('should handle stock as sink correctly for horizontal flow', () => {
        // Flow from cloud to stock: cloud at left, stock at right
        const stock = makeStock(stockUid, 200, 100);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 100, y: 100, attachedToUid: cloudUid }, // anchor (cloud)
          { x: 200 - StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock left edge
        ]);

        // Move stock down
        const newStockY = 100 + 50;
        const result = computeFlowRoute(flow, stock, 200, newStockY);

        expect(result.points.size).toBe(3);

        const anchor = result.points.get(0)!;
        const corner = result.points.get(1)!;
        const stockPoint = result.points.get(2)!;

        // Anchor unchanged
        expect(anchor.x).toBe(100);
        expect(anchor.y).toBe(100);

        // Stock attaches at top
        expect(stockPoint.y).toBe(newStockY - StockHeight / 2);

        // Corner creates proper L
        expect(corner.y).toBe(anchor.y); // horizontal from anchor
        expect(corner.x).toBe(stockPoint.x); // vertical to stock
      });
    });

    describe('edge cases', () => {
      it('should return unchanged flow if stock not attached', () => {
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 100, [
          { x: 50, y: 100, attachedToUid: 99 }, // different uid
          { x: 200, y: 100, attachedToUid: cloudUid },
        ]);

        const result = computeFlowRoute(flow, stock, 100, 150);

        // Should be unchanged
        expect(result.points.equals(flow.points)).toBe(true);
      });

      it('should return unchanged flow if fewer than 2 points', () => {
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 100, 100, [{ x: 100, y: 100, attachedToUid: stockUid }]);

        const result = computeFlowRoute(flow, stock, 100, 150);

        expect(result.points.size).toBe(1);
      });
    });
  });

  describe('UpdateStockAndFlows', () => {
    it('should update stock position and re-route all connected flows', () => {
      const stock = makeStock(stockUid, 100, 100, [flowUid], []);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100, attachedToUid: cloudUid },
      ]);

      // Move stock down by 50 (moveDelta is inverted: negative delta = positive movement)
      const [newStock, newFlows] = UpdateStockAndFlows(stock, List([flow]), { x: 0, y: -50 });

      // Stock should have moved
      expect(newStock.cx).toBe(100);
      expect(newStock.cy).toBe(150);

      // Flow should be L-shaped
      expect(newFlows.size).toBe(1);
      expect(newFlows.get(0)!.points.size).toBe(3);
    });

    it('should handle multiple flows attached to one stock', () => {
      const inflowUid = 2;
      const outflowUid = 3;
      const stock = makeStock(stockUid, 100, 100, [inflowUid], [outflowUid]);

      // Inflow from left
      const inflow = makeFlow(inflowUid, 50, 100, [
        { x: 0, y: 100, attachedToUid: 4 }, // cloud
        { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
      ]);

      // Outflow to right
      const outflow = makeFlow(outflowUid, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100, attachedToUid: 5 }, // cloud
      ]);

      const [newStock, newFlows] = UpdateStockAndFlows(stock, List([inflow, outflow]), { x: 0, y: -50 });

      expect(newStock.cy).toBe(150);
      expect(newFlows.size).toBe(2);

      // Both flows should be L-shaped
      expect(newFlows.get(0)!.points.size).toBe(3);
      expect(newFlows.get(1)!.points.size).toBe(3);
    });
  });

  describe('UpdateFlow - valve movement', () => {
    it('should move valve along horizontal segment', () => {
      // Horizontal flow from cloud to stock
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 200, 100);

      // Move valve to the right along the segment
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: -20, y: 0 });

      // Valve should move along the horizontal segment (Y stays same)
      expect(newFlow.cy).toBe(100);
      expect(newFlow.cx).toBe(170);
    });

    it('should move valve along vertical segment', () => {
      // Vertical flow from cloud to stock
      const flow = makeFlow(flowUid, 100, 150, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 100, y: 200, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 100, 200);

      // Move valve down along the segment
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: 0, y: -20 });

      // Valve should move along the vertical segment (X stays same)
      expect(newFlow.cx).toBe(100);
      expect(newFlow.cy).toBe(170);
    });

    it('should constrain valve to segment bounds', () => {
      // Short horizontal flow
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 200, 100);

      // Try to move valve way past the segment end
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: -500, y: 0 });

      // Valve should be clamped to segment bounds (with margin)
      expect(newFlow.cy).toBe(100);
      expect(newFlow.cx).toBeLessThanOrEqual(190); // margin from end
      expect(newFlow.cx).toBeGreaterThanOrEqual(110); // margin from start
    });

    it('should move valve on L-shaped flow along closest segment', () => {
      // L-shaped flow: horizontal then vertical
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid }, // anchor
        { x: 200, y: 100 }, // corner
        { x: 200, y: 200, attachedToUid: stockUid }, // stock
      ]);
      const stock = makeStock(stockUid, 200, 200);

      // Valve at (150, 100) is on the horizontal segment
      // Move it along that segment
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: -30, y: 0 });

      // Should stay on horizontal segment
      expect(newFlow.cy).toBe(100);
      expect(newFlow.cx).toBe(180);
    });
  });

  describe('moveSegment', () => {
    it('should move horizontal segment up/down', () => {
      // L-shaped flow with horizontal middle concept:
      // Actually for a simple test, let's use a 3-point L
      const points = List([
        new Point({ x: 100, y: 200, attachedToUid: cloudUid }),
        new Point({ x: 100, y: 100, attachedToUid: undefined }), // corner
        new Point({ x: 200, y: 100, attachedToUid: stockUid }),
      ]);

      // Move segment 1 (horizontal: corner to stock) up by 20
      const newPoints = moveSegment(points, 1, { x: 0, y: 20 });

      // The corner should move up (it's not an endpoint)
      expect(newPoints.get(1)!.y).toBe(80);
      // The stock endpoint should NOT move (it's attached)
      expect(newPoints.get(2)!.y).toBe(100);
      // The cloud endpoint should stay
      expect(newPoints.get(0)!.y).toBe(200);
    });

    it('should move vertical segment left/right', () => {
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: cloudUid }),
        new Point({ x: 100, y: 200, attachedToUid: undefined }), // corner
        new Point({ x: 200, y: 200, attachedToUid: stockUid }),
      ]);

      // Move segment 0 (vertical: cloud to corner) right by 20
      const newPoints = moveSegment(points, 0, { x: -20, y: 0 });

      // The corner should move right (it's not an endpoint)
      expect(newPoints.get(1)!.x).toBe(120);
      // The cloud endpoint should NOT move (it's attached)
      expect(newPoints.get(0)!.x).toBe(100);
      // The stock endpoint should stay
      expect(newPoints.get(2)!.x).toBe(200);
    });

    it('should not move attached endpoints', () => {
      // Simple 2-point horizontal flow
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: cloudUid }),
        new Point({ x: 200, y: 100, attachedToUid: stockUid }),
      ]);

      // Try to move the only segment
      const newPoints = moveSegment(points, 0, { x: 0, y: -50 });

      // Both endpoints are attached, so neither should move
      expect(newPoints.get(0)!.y).toBe(100);
      expect(newPoints.get(1)!.y).toBe(100);
    });
  });

  describe('UpdateFlow - segment movement', () => {
    it('should move a segment when segmentIndex is provided', () => {
      // L-shaped flow
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 200, attachedToUid: cloudUid },
        { x: 100, y: 100 }, // corner
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 200, 100);

      // Move segment 1 (horizontal) up
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: 0, y: 20 }, 1);

      // Corner should have moved up
      expect(newFlow.points.get(1)!.y).toBe(80);
      // Endpoints should not have moved
      expect(newFlow.points.get(0)!.y).toBe(200);
      expect(newFlow.points.get(2)!.y).toBe(100);
    });
  });
});
