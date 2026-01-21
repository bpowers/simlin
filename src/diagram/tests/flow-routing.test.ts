// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { Point, FlowViewElement, StockViewElement } from '@system-dynamics/core/datamodel';

import {
  computeFlowRoute,
  UpdateStockAndFlows,
  UpdateFlow,
  moveSegment,
  findClickedSegment,
  getSegments,
} from '../drawing/Flow';
import { StockWidth, StockHeight } from '../drawing/Stock';

function makeStock(
  uid: number,
  x: number,
  y: number,
  inflows: number[] = [],
  outflows: number[] = [],
): StockViewElement {
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

      it('should preserve off-center valve position when stock moves on straight flow', () => {
        // Horizontal flow with valve positioned off-center (closer to anchor)
        const stock = makeStock(stockUid, 100, 100);
        // Valve at x=180 (near anchor at x=200), not at midpoint x=161.25
        const flow = makeFlow(flowUid, 180, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge at x=122.5
          { x: 200, y: 100, attachedToUid: cloudUid },
        ]);

        // Move stock slightly (within bounds to keep flow straight)
        const newStockY = 100 - 10;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Flow should still be straight
        expect(result.points.size).toBe(2);

        // Valve should preserve its x position (clamped to segment bounds)
        // The valve was at x=180, which is still valid on the new segment
        expect(result.cx).toBe(180);
        expect(result.cy).toBe(100);
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

      it('should preserve off-center valve position when stock moves on L-shaped flow', () => {
        // L-shaped flow with valve positioned near the anchor (not at midpoint)
        // Flow: stock at top -> corner -> anchor at right (horizontal anchor segment)
        const stock = makeStock(stockUid, 100, 50);
        // Valve at (180, 100) is on the horizontal segment near the anchor
        const flow = makeFlow(flowUid, 180, 100, [
          { x: 100, y: 50 + StockHeight / 2, attachedToUid: stockUid }, // stock bottom
          { x: 100, y: 100 }, // corner
          { x: 200, y: 100, attachedToUid: cloudUid }, // anchor
        ]);

        // Move stock further up - this changes the vertical segment length
        const newStockY = 30;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Flow should still be L-shaped
        expect(result.points.size).toBe(3);

        // Valve should preserve its position on the horizontal segment
        // It was at (180, 100) which is still valid on the anchor segment
        expect(result.cx).toBe(180);
        expect(result.cy).toBe(100);
      });

      it('should clamp valve to nearest segment when straight flow becomes L-shaped', () => {
        // Horizontal flow with valve near the center
        const stock = makeStock(stockUid, 100, 100);
        // Valve at (160, 100) on the horizontal segment
        const flow = makeFlow(flowUid, 160, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge at x=122.5
          { x: 200, y: 100, attachedToUid: cloudUid },
        ]);

        // Move stock down significantly to create L-shape
        const newStockY = 100 + 50;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Flow should become L-shaped
        expect(result.points.size).toBe(3);

        // Valve was at (160, 100). The new L-shape has:
        // - Vertical segment from stock at (100, 132.5) to corner at (100, 100)
        // - Horizontal segment from corner at (100, 100) to anchor at (200, 100)
        // The valve (160, 100) is on the horizontal segment, so it should stay there
        expect(result.cx).toBe(160);
        expect(result.cy).toBe(100);
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

    describe('multi-point flow preservation', () => {
      it('should preserve non-adjacent points and adjust adjacent corner on 4+ point flow', () => {
        // 4-point flow: stock -> corner1 -> corner2 -> cloud
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge
          { x: 150, y: 100 }, // corner1 (adjacent to stock - will be adjusted)
          { x: 150, y: 200 }, // corner2 (not adjacent - preserved)
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock up
        const newStockY = 80;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Should preserve all 4 points
        expect(result.points.size).toBe(4);

        // Stock endpoint should be on stock's actual edge
        const stockPoint = result.points.get(0)!;
        expect(stockPoint.attachedToUid).toBe(stockUid);
        expect(stockPoint.y).toBe(newStockY);

        // Corner1 is adjacent to stock - its Y is adjusted to maintain horizontal segment
        const corner1 = result.points.get(1)!;
        expect(corner1.x).toBe(150);
        expect(corner1.y).toBe(newStockY); // Adjusted to match stock edge

        // Corner2 is not adjacent to stock - fully preserved
        const corner2 = result.points.get(2)!;
        expect(corner2.x).toBe(150);
        expect(corner2.y).toBe(200);

        // Anchor should be unchanged
        const anchor = result.points.get(3)!;
        expect(anchor.x).toBe(200);
        expect(anchor.y).toBe(200);
        expect(anchor.attachedToUid).toBe(cloudUid);
      });

      it('should preserve intermediate points when stock is at end of 4+ point flow', () => {
        // 4-point flow: cloud -> corner1 -> corner2 -> stock
        const stock = makeStock(stockUid, 200, 200);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100, y: 100, attachedToUid: cloudUid }, // cloud
          { x: 150, y: 100 }, // corner1
          { x: 150, y: 200 }, // corner2
          { x: 200 - StockWidth / 2, y: 200, attachedToUid: stockUid }, // stock left edge
        ]);

        // Move stock right
        const newStockX = 250;
        const result = computeFlowRoute(flow, stock, newStockX, 200);

        // Should preserve all 4 points
        expect(result.points.size).toBe(4);

        // Anchor should be unchanged
        const anchor = result.points.get(0)!;
        expect(anchor.x).toBe(100);
        expect(anchor.attachedToUid).toBe(cloudUid);

        // Intermediate points should be preserved
        const corner1 = result.points.get(1)!;
        const corner2 = result.points.get(2)!;
        expect(corner1.x).toBe(150);
        expect(corner1.y).toBe(100);
        expect(corner2.x).toBe(150);
        expect(corner2.y).toBe(200);

        // Stock endpoint should be updated
        const stockPoint = result.points.get(3)!;
        expect(stockPoint.attachedToUid).toBe(stockUid);
      });

      it('should update valve position when moving stock on 4+ point flow', () => {
        // 4-point flow with valve on middle segment
        // Segments: [stock-corner1], [corner1-corner2], [corner2-cloud]
        const stock = makeStock(stockUid, 100, 100);
        // Valve at (150, 150) is on segment 1 (corner1-corner2)
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge
          { x: 150, y: 100 }, // corner1
          { x: 150, y: 200 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock - this shouldn't affect the valve since it's on segment 1
        const result = computeFlowRoute(flow, stock, 100, 80);

        // Valve should still be clamped to a valid segment
        // The segments are still the same, so valve should be on segment 1
        expect(result.cx).toBe(150);
        expect(result.cy).toBe(150);
      });

      it('should clamp valve to nearest segment when stock moves significantly', () => {
        // 4-point flow with valve on the middle vertical segment
        const stock = makeStock(stockUid, 100, 100);
        // Valve at (150, 150) is on segment 1 (corner1-corner2, vertical at x=150)
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge (122.5, 100)
          { x: 150, y: 100 }, // corner1
          { x: 150, y: 200 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock right - this changes the first segment but not the middle one
        const newStockX = 130;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // Stock endpoint should be updated to new right edge
        const stockPoint = result.points.get(0)!;
        expect(stockPoint.x).toBe(newStockX + StockWidth / 2); // 152.5
        expect(stockPoint.y).toBe(100);
        expect(stockPoint.attachedToUid).toBe(stockUid);

        // Valve was at (150, 150) on segment 1 (vertical from corner1 to corner2)
        // Segment 1 is still vertical at x=150 from y=100 to y=200
        // The valve should still be at (150, 150) since it's on an unaffected segment
        expect(result.cx).toBe(150);
        expect(result.cy).toBe(150);
      });

      it('should preserve horizontal orientation when stock moves beyond 45 degree threshold', () => {
        // 4-point flow with horizontal first segment: stock -> corner1 (horizontal)
        // This tests that the segment orientation is determined from the existing geometry,
        // not from the direction to the adjacent point (which would flip at 45 degrees)
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge at (122.5, 100)
          { x: 150, y: 100 }, // corner1 at y=100 (horizontal segment)
          { x: 150, y: 200 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock way down - dy (100) > dx (27.5), which would flip to vertical
        // if we used the naive Math.abs(dx) > Math.abs(dy) heuristic
        const newStockY = 200;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // First segment should STILL be horizontal (Y values match)
        const stockPoint = result.points.get(0)!;
        const corner1 = result.points.get(1)!;
        expect(stockPoint.y).toBe(corner1.y);

        // Corner2 should be unchanged (no diagonal created)
        const corner2 = result.points.get(2)!;
        expect(corner2.x).toBe(150);
        expect(corner2.y).toBe(200);

        // The first segment is horizontal, so corner1's X is preserved, Y is adjusted
        expect(corner1.x).toBe(150);
        expect(corner1.y).toBe(newStockY); // Adjusted to match stock

        // Second segment (corner1 to corner2) should be vertical
        expect(corner1.x).toBe(corner2.x);
      });

      it('should keep first segment horizontal when stock moves vertically on 4+ point flow', () => {
        // 4-point flow with horizontal first segment: stock -> corner1 (horizontal)
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid }, // stock right edge at y=100
          { x: 150, y: 100 }, // corner1 at y=100 (horizontal segment)
          { x: 150, y: 200 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock vertically - endpoint stays on stock edge, corner1 adjusts to maintain horizontal
        const newStockY = 120;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // First segment (stock to corner1) should remain horizontal
        const stockPoint = result.points.get(0)!;
        const corner1 = result.points.get(1)!;

        // Endpoint stays on stock's actual edge (y = newStockY)
        expect(stockPoint.y).toBe(newStockY);
        // Corner1's Y is adjusted to match, preserving horizontal segment
        expect(corner1.y).toBe(newStockY);
        // Same Y values = horizontal segment
        expect(stockPoint.y).toBe(corner1.y);
      });

      it('should keep first segment vertical when stock moves horizontally on 4+ point flow', () => {
        // 4-point flow with vertical first segment: stock -> corner1 (vertical)
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid }, // stock bottom edge at x=100
          { x: 100, y: 150 }, // corner1 at x=100 (vertical segment)
          { x: 200, y: 150 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock horizontally - endpoint stays on stock edge, corner1 adjusts to maintain vertical
        const newStockX = 120;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // First segment (stock to corner1) should remain vertical
        const stockPoint = result.points.get(0)!;
        const corner1 = result.points.get(1)!;

        // Endpoint stays on stock's actual edge (x = newStockX)
        expect(stockPoint.x).toBe(newStockX);
        // Corner1's X is adjusted to match, preserving vertical segment
        expect(corner1.x).toBe(newStockX);
        // Same X values = vertical segment
        expect(stockPoint.x).toBe(corner1.x);
      });

      it('should preserve vertical orientation when stock moves beyond 45 degree threshold', () => {
        // 4-point flow with vertical first segment: stock -> corner1 (vertical)
        // This tests that the segment orientation is determined from the existing geometry,
        // not from the direction to the adjacent point (which would flip at 45 degrees)
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid }, // stock bottom edge at (100, 117.5)
          { x: 100, y: 150 }, // corner1 at x=100 (vertical segment)
          { x: 200, y: 150 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Move stock way right - dx (100) > dy (32.5), which would flip to horizontal
        // if we used the naive Math.abs(dx) > Math.abs(dy) heuristic
        const newStockX = 200;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // First segment should STILL be vertical (X values match)
        const stockPoint = result.points.get(0)!;
        const corner1 = result.points.get(1)!;
        expect(stockPoint.x).toBe(corner1.x);

        // Corner2 should be unchanged (no diagonal created)
        const corner2 = result.points.get(2)!;
        expect(corner2.x).toBe(200);
        expect(corner2.y).toBe(150);

        // The first segment is vertical, so corner1's Y is preserved, X is adjusted
        expect(corner1.y).toBe(150);
        expect(corner1.x).toBe(newStockX); // Adjusted to match stock

        // Second segment (corner1 to corner2) should be horizontal
        expect(corner1.y).toBe(corner2.y);
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

    it('should clamp valve to midpoint on very short horizontal segment', () => {
      // Very short horizontal flow (15px, less than 2 * margin of 20px)
      const flow = makeFlow(flowUid, 107.5, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 115, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 115, 100);

      // Try to move valve - should clamp to midpoint since segment is too short
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: -50, y: 0 });

      // Valve should be at segment midpoint
      expect(newFlow.cy).toBe(100);
      expect(newFlow.cx).toBe(107.5); // midpoint of 100 to 115
    });

    it('should clamp valve to midpoint on very short vertical segment', () => {
      // Very short vertical flow (15px, less than 2 * margin of 20px)
      const flow = makeFlow(flowUid, 100, 107.5, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 100, y: 115, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 100, 115);

      // Try to move valve - should clamp to midpoint since segment is too short
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: 0, y: -50 });

      // Valve should be at segment midpoint
      expect(newFlow.cx).toBe(100);
      expect(newFlow.cy).toBe(107.5); // midpoint of 100 to 115
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

    it('should re-clamp valve when dragging adjacent segment that shares a corner', () => {
      // 5-point flow with valve on segment 1
      // Segments: [cloud-corner1], [corner1-corner2], [corner2-corner3], [corner3-stock]
      // Valve is on segment 1 (corner1-corner2), segment 2 is dragged
      const flow = makeFlow(flowUid, 100, 150, [
        { x: 100, y: 100, attachedToUid: cloudUid }, // cloud
        { x: 100, y: 200 }, // corner1
        { x: 150, y: 200 }, // corner2 (shared by segments 1 and 2)
        { x: 150, y: 300 }, // corner3
        { x: 200, y: 300, attachedToUid: stockUid }, // stock
      ]);
      const stock = makeStock(stockUid, 200, 300);

      // Valve at (100, 150) is on segment 0 (vertical from cloud to corner1)
      // Drag segment 1 (horizontal corner1-corner2) down - this moves corner1 and corner2
      const [newFlow] = UpdateFlow(flow, List([stock]), { x: 0, y: -20 }, 1);

      // Segment 1 moved: corner1 and corner2 moved down by 20
      expect(newFlow.points.get(1)!.y).toBe(220);
      expect(newFlow.points.get(2)!.y).toBe(220);

      // The valve was on segment 0 (cloud at y=100 to corner1 at y=200, vertical at x=100)
      // Now segment 0 goes from y=100 to y=220 (longer), still vertical at x=100
      // The valve should still be clamped to segment 0 since it's closest
      expect(newFlow.cx).toBe(100);
      // Valve y should still be within the segment (100+margin to 220-margin)
      expect(newFlow.cy).toBeGreaterThanOrEqual(100);
      expect(newFlow.cy).toBeLessThanOrEqual(220);
    });
  });

  describe('findClickedSegment', () => {
    it('should return undefined when clicking on the valve', () => {
      // 4-point flow with valve at (150, 100)
      const points = List([
        new Point({ x: 100, y: 200, attachedToUid: cloudUid }),
        new Point({ x: 100, y: 100, attachedToUid: undefined }),
        new Point({ x: 200, y: 100, attachedToUid: undefined }),
        new Point({ x: 200, y: 50, attachedToUid: stockUid }),
      ]);
      const valveCx = 150;
      const valveCy = 100;

      // Click exactly on the valve
      const result = findClickedSegment(150, 100, valveCx, valveCy, points);
      expect(result).toBeUndefined();

      // Click near the valve (within tolerance)
      const result2 = findClickedSegment(155, 103, valveCx, valveCy, points);
      expect(result2).toBeUndefined();
    });

    it('should return undefined for single-segment (straight) flows', () => {
      // Straight horizontal flow - only 2 points
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: cloudUid }),
        new Point({ x: 200, y: 100, attachedToUid: stockUid }),
      ]);
      const valveCx = 150;
      const valveCy = 100;

      // Click away from the valve on the segment
      const result = findClickedSegment(120, 100, valveCx, valveCy, points);
      expect(result).toBeUndefined();
    });

    it('should return undefined for L-shaped flow segments with attached endpoints', () => {
      // L-shaped flow: both segments have one attached endpoint
      // Segment 0 has attached first point, segment 1 has attached last point
      const points = List([
        new Point({ x: 100, y: 200, attachedToUid: cloudUid }),
        new Point({ x: 100, y: 100, attachedToUid: undefined }), // corner
        new Point({ x: 200, y: 100, attachedToUid: stockUid }),
      ]);
      const valveCx = 150;
      const valveCy = 100;

      // Click on the vertical segment (segment 0) - has attached first point
      const result = findClickedSegment(100, 150, valveCx, valveCy, points);
      expect(result).toBeUndefined();

      // Click on the horizontal segment (segment 1) - has attached last point
      const result2 = findClickedSegment(180, 100, valveCx, valveCy, points);
      expect(result2).toBeUndefined();
    });

    it('should return segment index for middle segment of 4-point flow', () => {
      // 4-point flow: 3 segments, middle segment has no attached endpoints
      // Segment 0: attached -> corner1 (has attached endpoint)
      // Segment 1: corner1 -> corner2 (no attached endpoints - CAN drag)
      // Segment 2: corner2 -> attached (has attached endpoint)
      const points = List([
        new Point({ x: 100, y: 200, attachedToUid: cloudUid }),
        new Point({ x: 100, y: 100, attachedToUid: undefined }), // corner1
        new Point({ x: 200, y: 100, attachedToUid: undefined }), // corner2
        new Point({ x: 200, y: 50, attachedToUid: stockUid }),
      ]);
      const valveCx = 150;
      const valveCy = 100;

      // Click on the middle horizontal segment (segment 1)
      const result = findClickedSegment(150, 100 + 20, valveCx, valveCy, points);
      expect(result).toBe(1);

      // Click on segment 0 (has attached endpoint) - should return undefined
      const result2 = findClickedSegment(100, 150, valveCx, valveCy, points);
      expect(result2).toBeUndefined();

      // Click on segment 2 (has attached endpoint) - should return undefined
      const result3 = findClickedSegment(200, 75, valveCx, valveCy, points);
      expect(result3).toBeUndefined();
    });

    it('should return undefined for empty points list', () => {
      const points = List<Point>();
      const result = findClickedSegment(100, 100, 100, 100, points);
      expect(result).toBeUndefined();
    });

    it('should return undefined for diagonal segments (from imported models)', () => {
      // 4-point flow with a diagonal middle segment (shouldn't exist in valid geometry,
      // but could appear in imported models). Diagonal segments can't be dragged
      // because moveSegment assumes axis-aligned segments.
      const points = List([
        new Point({ x: 100, y: 200, attachedToUid: cloudUid }),
        new Point({ x: 100, y: 100, attachedToUid: undefined }), // corner1
        new Point({ x: 200, y: 150, attachedToUid: undefined }), // corner2 - diagonal from corner1!
        new Point({ x: 200, y: 50, attachedToUid: stockUid }),
      ]);
      const valveCx = 150;
      const valveCy = 125;

      // Click on the diagonal middle segment (segment 1) - should return undefined
      const result = findClickedSegment(150, 125, valveCx, valveCy, points);
      expect(result).toBeUndefined();
    });
  });

  describe('getSegments', () => {
    it('should identify horizontal segments', () => {
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: undefined }),
        new Point({ x: 200, y: 100, attachedToUid: undefined }),
      ]);
      const segments = getSegments(points);

      expect(segments.length).toBe(1);
      expect(segments[0].isHorizontal).toBe(true);
      expect(segments[0].isVertical).toBe(false);
      expect(segments[0].isDiagonal).toBe(false);
    });

    it('should identify vertical segments', () => {
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: undefined }),
        new Point({ x: 100, y: 200, attachedToUid: undefined }),
      ]);
      const segments = getSegments(points);

      expect(segments.length).toBe(1);
      expect(segments[0].isHorizontal).toBe(false);
      expect(segments[0].isVertical).toBe(true);
      expect(segments[0].isDiagonal).toBe(false);
    });

    it('should identify diagonal segments', () => {
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: undefined }),
        new Point({ x: 200, y: 200, attachedToUid: undefined }),
      ]);
      const segments = getSegments(points);

      expect(segments.length).toBe(1);
      expect(segments[0].isHorizontal).toBe(false);
      expect(segments[0].isVertical).toBe(false);
      expect(segments[0].isDiagonal).toBe(true);
    });

    it('should handle mixed segment types', () => {
      // Path: horizontal -> diagonal -> vertical
      const points = List([
        new Point({ x: 100, y: 100, attachedToUid: undefined }),
        new Point({ x: 200, y: 100, attachedToUid: undefined }),
        new Point({ x: 250, y: 150, attachedToUid: undefined }),
        new Point({ x: 250, y: 250, attachedToUid: undefined }),
      ]);
      const segments = getSegments(points);

      expect(segments.length).toBe(3);
      expect(segments[0].isHorizontal).toBe(true);
      expect(segments[0].isDiagonal).toBe(false);
      expect(segments[1].isHorizontal).toBe(false);
      expect(segments[1].isDiagonal).toBe(true);
      expect(segments[2].isVertical).toBe(true);
      expect(segments[2].isDiagonal).toBe(false);
    });

    it('should return empty array for single point', () => {
      const points = List([new Point({ x: 100, y: 100, attachedToUid: undefined })]);
      const segments = getSegments(points);
      expect(segments.length).toBe(0);
    });

    it('should return empty array for empty points list', () => {
      const points = List<Point>();
      const segments = getSegments(points);
      expect(segments.length).toBe(0);
    });
  });
});
