// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { Point, FlowViewElement, StockViewElement, CloudViewElement } from '@simlin/core/datamodel';

import {
  computeFlowRoute,
  UpdateStockAndFlows,
  UpdateCloudAndFlow,
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

function makeCloud(uid: number, flowUid: number, x: number, y: number): CloudViewElement {
  return new CloudViewElement({
    uid,
    flowUid,
    x,
    y,
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

      it('should preserve valve fractional position when stock moves along flow axis', () => {
        // Horizontal flow: stock at x=100, cloud at x=200
        // Valve starts at x=150 (roughly at the midpoint of the segment)
        const stock = makeStock(stockUid, 100, 100);
        const stockEdgeX = 100 + StockWidth / 2; // 122.5
        const anchorX = 200;
        const valveX = 150;

        const flow = makeFlow(flowUid, valveX, 100, [
          { x: stockEdgeX, y: 100, attachedToUid: stockUid },
          { x: anchorX, y: 100, attachedToUid: cloudUid },
        ]);

        // Calculate the valve's fractional position on the original segment
        // Fraction = (valve - anchor) / (stockEdge - anchor) = (150 - 200) / (122.5 - 200) = 0.645
        const originalFraction = (valveX - anchorX) / (stockEdgeX - anchorX);

        // Move stock right along the flow axis (toward anchor)
        // This makes the segment shorter
        const newStockX = 160;
        const newStockEdgeX = newStockX + StockWidth / 2; // 182.5
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // Flow should still be straight (2 points)
        expect(result.points.size).toBe(2);

        // Verify the new segment bounds
        expect(result.points.get(0)!.x).toBe(newStockEdgeX);
        expect(result.points.get(1)!.x).toBe(anchorX);

        // The valve should preserve its fractional position along the segment.
        // New valve x = anchor + fraction * (newStockEdge - anchor)
        // = 200 + 0.645 * (182.5 - 200) = 200 + 0.645 * (-17.5) = 200 - 11.29 ≈ 188.7
        const expectedValveX = anchorX + originalFraction * (newStockEdgeX - anchorX);
        expect(result.cx).toBeCloseTo(expectedValveX, 1);
        expect(result.cy).toBe(100);
      });

      it('should preserve valve fractional position when stock moves past valve position', () => {
        // This is a more extreme case where the old valve position is outside the new segment.
        // Horizontal flow: stock at x=100, cloud at x=200
        // Valve starts at x=150 (roughly at the midpoint of the segment)
        const stock = makeStock(stockUid, 100, 100);
        const stockEdgeX = 100 + StockWidth / 2; // 122.5
        const anchorX = 200;
        const valveX = 150;

        const flow = makeFlow(flowUid, valveX, 100, [
          { x: stockEdgeX, y: 100, attachedToUid: stockUid },
          { x: anchorX, y: 100, attachedToUid: cloudUid },
        ]);

        // Calculate the valve's fractional position on the original segment
        const originalFraction = (valveX - anchorX) / (stockEdgeX - anchorX);

        // Move stock PAST the old valve position (stock center at 170 means edge at 192.5)
        const newStockX = 170;
        const newStockEdgeX = newStockX + StockWidth / 2; // 192.5
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // Flow should still be straight (2 points)
        expect(result.points.size).toBe(2);

        // The valve should preserve its fractional position.
        // Even though the old valve position (150) is now outside the new segment [192.5, 200],
        // the fractional position places it correctly within the new segment.
        // New valve x = 200 + 0.645 * (192.5 - 200) = 200 - 4.84 ≈ 195.16
        const expectedValveX = anchorX + originalFraction * (newStockEdgeX - anchorX);
        expect(result.cx).toBeCloseTo(expectedValveX, 1);
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

      it('should preserve valve fraction from second segment when L-shape straightens', () => {
        // L-shaped flow where the valve is on segment 1 (horizontal, near anchor),
        // not segment 0 (vertical, near stock).
        // When the L-shape straightens, the valve fraction should be computed from
        // the segment the valve was actually on, not always from segment 0.
        const stock = makeStock(stockUid, 100, 150);
        // L-shape: stock bottom -> corner -> anchor
        // Segment 0: vertical from (100, 132.5) to (100, 100) - stock to corner
        // Segment 1: horizontal from (100, 100) to (200, 100) - corner to anchor
        // Valve at (180, 100) is on segment 1, at fraction 0.8 along that segment
        const flow = makeFlow(flowUid, 180, 100, [
          { x: 100, y: 150 - StockHeight / 2, attachedToUid: stockUid }, // stock top at y=132.5
          { x: 100, y: 100 }, // corner
          { x: 200, y: 100, attachedToUid: cloudUid }, // anchor
        ]);

        // Calculate valve's fractional position on OLD segment 1 (horizontal)
        // Segment 1 goes from corner (100, 100) to anchor (200, 100)
        const oldCornerX = 100;
        const anchorX = 200;
        const valveX = 180;
        const oldFraction = (valveX - oldCornerX) / (anchorX - oldCornerX); // = 0.8

        // Move stock back to anchor's Y level to straighten the L-shape
        const newStockY = 100;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Should revert to 2 points (straight)
        expect(result.points.size).toBe(2);

        // New segment goes from stock edge (122.5, 100) to anchor (200, 100)
        const newStockEdgeX = 100 + StockWidth / 2; // 122.5
        // The valve should preserve its fraction (0.8) along the new segment
        // Expected X = newStockEdgeX + 0.8 * (anchorX - newStockEdgeX)
        // = 122.5 + 0.8 * 77.5 = 122.5 + 62 = 184.5
        const expectedValveX = newStockEdgeX + oldFraction * (anchorX - newStockEdgeX);
        expect(result.cx).toBeCloseTo(expectedValveX, 1);
        expect(result.cy).toBe(100);
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

        // Move stock down but NOT to y=200 (which would make corner1 colinear with corner2).
        // At y=180, dy (80) > dx (27.5), which would flip to vertical if we used
        // the naive Math.abs(dx) > Math.abs(dy) heuristic.
        const newStockY = 180;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // Should still be 4 points (no colinear segments to remove)
        expect(result.points.size).toBe(4);

        // First segment should STILL be horizontal (Y values match)
        const stockPoint = result.points.get(0)!;
        const corner1 = result.points.get(1)!;
        expect(stockPoint.y).toBe(corner1.y);
        expect(stockPoint.y).toBe(newStockY);

        // Corner2 should be unchanged
        const corner2 = result.points.get(2)!;
        expect(corner2.x).toBe(150);
        expect(corner2.y).toBe(200);

        // The first segment is horizontal, so corner1's X is preserved, Y is adjusted
        expect(corner1.x).toBe(150);
        expect(corner1.y).toBe(newStockY);

        // Second segment (corner1 to corner2) should be vertical
        expect(corner1.x).toBe(corner2.x);
      });

      it('should normalize to remove colinear segments when stock aligns with corner (horizontal)', () => {
        // When stock moves to the same Y as corner2, corner1 becomes colinear
        // and should be removed by normalization.
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 150, y: 100 }, // corner1
          { x: 150, y: 200 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid },
        ]);

        // Move stock to y=200 - same as corner2 and cloud
        const newStockY = 200;
        const result = computeFlowRoute(flow, stock, 100, newStockY);

        // After normalization, the flow should be straight (2 points)
        // because all segments become colinear (all at y=200)
        expect(result.points.size).toBe(2);

        const stockPoint = result.points.get(0)!;
        const anchor = result.points.get(1)!;
        expect(stockPoint.y).toBe(200);
        expect(anchor.y).toBe(200);
      });

      it('should normalize to remove colinear segments when stock aligns with corner (vertical)', () => {
        // When stock moves to the same X as corner2, corner1 becomes colinear
        // and should be removed by normalization.
        const stock = makeStock(stockUid, 100, 100);
        const flow = makeFlow(flowUid, 150, 150, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid }, // stock bottom edge
          { x: 100, y: 150 }, // corner1 at x=100 (vertical)
          { x: 200, y: 150 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid },
        ]);

        // Move stock to x=200 - same as corner2 and cloud
        const newStockX = 200;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // After normalization, corner1 becomes (200, 150) same as corner2,
        // so it's removed. The flow becomes 3 points: stock -> corner2 -> cloud
        // Then since stock is also at x=200, we get a vertical line plus horizontal.
        // Actually, let's trace through:
        // - stockPoint at (200, 117.5) - bottom edge of stock
        // - corner1 adjusted to (200, 150) to keep vertical - but that's same as corner2!
        // - corner2 at (200, 150)
        // - cloud at (200, 200)
        // Normalization removes corner1 (zero-length segment with corner2)
        // Then we have: stock(200,117.5) -> corner2(200,150) -> cloud(200,200)
        // All at x=200, so corner2 is also removed (colinear)
        // Final: stock(200,117.5) -> cloud(200,200) = 2 points (straight vertical)
        expect(result.points.size).toBe(2);

        const stockPoint = result.points.get(0)!;
        const anchor = result.points.get(1)!;
        expect(stockPoint.x).toBe(200);
        expect(anchor.x).toBe(200);
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

      it('should preserve valve fraction when stock moves along first segment on 4+ point flow', () => {
        // 4-point flow with horizontal first segment
        // Valve is on the first segment with a specific fractional position
        const stock = makeStock(stockUid, 100, 100);
        const stockEdgeX = 100 + StockWidth / 2; // 122.5
        const corner1X = 150;
        // Valve at x=140 is at fraction (140-122.5)/(150-122.5) = 17.5/27.5 = 0.636 along segment
        const valveX = 140;
        const flow = makeFlow(flowUid, valveX, 100, [
          { x: stockEdgeX, y: 100, attachedToUid: stockUid }, // stock right edge
          { x: corner1X, y: 100 }, // corner1 (horizontal segment)
          { x: corner1X, y: 200 }, // corner2
          { x: 200, y: 200, attachedToUid: cloudUid }, // cloud
        ]);

        // Calculate original valve fraction along first segment
        const originalFraction = (valveX - stockEdgeX) / (corner1X - stockEdgeX);

        // Move stock left - this makes the first segment longer
        const newStockX = 70;
        const newStockEdgeX = newStockX + StockWidth / 2; // 92.5
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // New segment goes from 92.5 to 150 (longer than before)
        // Expected valve X = newStockEdgeX + fraction * (corner1X - newStockEdgeX)
        // = 92.5 + 0.636 * (150 - 92.5) = 92.5 + 36.6 = 129.1
        const expectedValveX = newStockEdgeX + originalFraction * (corner1X - newStockEdgeX);
        expect(result.cx).toBeCloseTo(expectedValveX, 1);
        expect(result.cy).toBe(100);
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

        // Move stock right but NOT to x=200 (which would make corner1 colinear with corner2).
        // dx (80) > dy (32.5), which would flip to horizontal if we used the naive
        // Math.abs(dx) > Math.abs(dy) heuristic.
        const newStockX = 180;
        const result = computeFlowRoute(flow, stock, newStockX, 100);

        // First segment should STILL be vertical (X values match)
        const stockPoint = result.points.get(0)!;
        const corner1 = result.points.get(1)!;
        expect(stockPoint.x).toBe(corner1.x);

        // The first segment is vertical, so corner1's Y is preserved, X is adjusted
        expect(corner1.y).toBe(150);
        expect(corner1.x).toBe(newStockX); // Adjusted to match stock

        // Should still have 4 points (no colinear segments created)
        expect(result.points.size).toBe(4);

        // Corner2 should be unchanged (no diagonal created)
        const corner2 = result.points.get(2)!;
        expect(corner2.x).toBe(200);
        expect(corner2.y).toBe(150);

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

    describe('flow spreading - multiple flows on same side', () => {
      it('should spread two flows on the bottom side at 1/3 and 2/3 positions', () => {
        const inflowUid = 2;
        const outflowUid = 3;
        const stock = makeStock(stockUid, 100, 100, [inflowUid], [outflowUid]);

        // Stock is at (100, 100), will move up to (100, 50)
        // Inflow from left (cloud at x=0) - will attach to bottom
        const inflow = makeFlow(inflowUid, 50, 100, [
          { x: 0, y: 100, attachedToUid: 4 }, // cloud on left
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
        ]);

        // Outflow to right (cloud at x=200) - will also attach to bottom
        const outflow = makeFlow(outflowUid, 150, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 200, y: 100, attachedToUid: 5 }, // cloud on right
        ]);

        // Move stock up by 50 so both flows become L-shaped and attach to bottom
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([inflow, outflow]), { x: 0, y: 50 });

        expect(newStock.cy).toBe(50);

        // Both flows should be L-shaped and attach to bottom
        const newInflow = newFlows.get(0)!;
        const newOutflow = newFlows.get(1)!;

        expect(newInflow.points.size).toBe(3);
        expect(newOutflow.points.size).toBe(3);

        // Get the stock attachment points
        const inflowStockPt = newInflow.points.get(newInflow.points.size - 1)!;
        const outflowStockPt = newOutflow.points.get(0)!;

        // Both should be at the bottom of the stock (y = 50 + StockHeight/2)
        const bottomY = 50 + StockHeight / 2;
        expect(inflowStockPt.y).toBe(bottomY);
        expect(outflowStockPt.y).toBe(bottomY);

        // The inflow (adjacent point at x=0) should be on the left (1/3)
        // The outflow (adjacent point at x=200) should be on the right (2/3)
        // Stock X range: 100 - StockWidth/2 to 100 + StockWidth/2 = 77.5 to 122.5
        const leftEdge = 100 - StockWidth / 2;
        const oneThird = leftEdge + StockWidth / 3;
        const twoThirds = leftEdge + (2 * StockWidth) / 3;

        expect(inflowStockPt.x).toBeCloseTo(oneThird, 1);
        expect(outflowStockPt.x).toBeCloseTo(twoThirds, 1);
      });

      it('should order flows on bottom side by adjacent point X coordinate', () => {
        // Three flows that will all attach to the bottom
        const flow1Uid = 2;
        const flow2Uid = 3;
        const flow3Uid = 4;
        const stock = makeStock(stockUid, 100, 100, [], [flow1Uid, flow2Uid, flow3Uid]);

        // Flow 1: adjacent point at x=150 (middle)
        const flow1 = makeFlow(flow1Uid, 125, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 150, y: 100, attachedToUid: 5 },
        ]);

        // Flow 2: adjacent point at x=50 (leftmost)
        const flow2 = makeFlow(flow2Uid, 75, 100, [
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 50, y: 100, attachedToUid: 6 },
        ]);

        // Flow 3: adjacent point at x=250 (rightmost)
        const flow3 = makeFlow(flow3Uid, 175, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 250, y: 100, attachedToUid: 7 },
        ]);

        // Move stock up so all flows become L-shaped
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([flow1, flow2, flow3]), { x: 0, y: 50 });

        expect(newStock.cy).toBe(50);

        // Get the stock attachment X coordinates for each flow
        const getStockX = (flow: typeof newFlows extends List<infer T> ? T : never) => {
          const stockIsFirst = flow.points.get(0)!.attachedToUid === stockUid;
          return stockIsFirst ? flow.points.get(0)!.x : flow.points.get(flow.points.size - 1)!.x;
        };

        const flow1StockX = getStockX(newFlows.get(0)!);
        const flow2StockX = getStockX(newFlows.get(1)!);
        const flow3StockX = getStockX(newFlows.get(2)!);

        // Order should be: flow2 (x=50) < flow1 (x=150) < flow3 (x=250)
        // So: flow2 at 1/4, flow1 at 2/4 (1/2), flow3 at 3/4
        const leftEdge = 100 - StockWidth / 2;
        const quarter1 = leftEdge + StockWidth / 4;
        const quarter2 = leftEdge + StockWidth / 2;
        const quarter3 = leftEdge + (3 * StockWidth) / 4;

        expect(flow2StockX).toBeCloseTo(quarter1, 1);
        expect(flow1StockX).toBeCloseTo(quarter2, 1);
        expect(flow3StockX).toBeCloseTo(quarter3, 1);
      });

      it('should spread two flows on the top side at 1/3 and 2/3 positions', () => {
        const inflowUid = 2;
        const outflowUid = 3;
        const stock = makeStock(stockUid, 100, 100, [inflowUid], [outflowUid]);

        // Stock at (100, 100), will move down to (100, 150)
        // Inflow from left cloud - will attach to top
        const inflow = makeFlow(inflowUid, 50, 100, [
          { x: 0, y: 100, attachedToUid: 4 },
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
        ]);

        // Outflow to right cloud - will also attach to top
        const outflow = makeFlow(outflowUid, 150, 100, [
          { x: 100 + StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 200, y: 100, attachedToUid: 5 },
        ]);

        // Move stock down
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([inflow, outflow]), { x: 0, y: -50 });

        expect(newStock.cy).toBe(150);

        const newInflow = newFlows.get(0)!;
        const newOutflow = newFlows.get(1)!;

        // Get the stock attachment points
        const inflowStockPt = newInflow.points.get(newInflow.points.size - 1)!;
        const outflowStockPt = newOutflow.points.get(0)!;

        // Both should be at the top of the stock
        const topY = 150 - StockHeight / 2;
        expect(inflowStockPt.y).toBe(topY);
        expect(outflowStockPt.y).toBe(topY);

        // Left flow should be at 1/3, right flow at 2/3
        const leftEdge = 100 - StockWidth / 2;
        const oneThird = leftEdge + StockWidth / 3;
        const twoThirds = leftEdge + (2 * StockWidth) / 3;

        expect(inflowStockPt.x).toBeCloseTo(oneThird, 1);
        expect(outflowStockPt.x).toBeCloseTo(twoThirds, 1);
      });

      it('should spread two flows on the left side by Y coordinate', () => {
        const flow1Uid = 2;
        const flow2Uid = 3;
        const stock = makeStock(stockUid, 100, 100, [flow1Uid], [flow2Uid]);

        // Stock at (100, 100), will move right to (150, 100)
        // Flow 1: vertical flow from above (cloud at y=50) - will attach to left, upper position
        const flow1 = makeFlow(flow1Uid, 100, 75, [
          { x: 100, y: 50, attachedToUid: 4 },
          { x: 100, y: 100 - StockHeight / 2, attachedToUid: stockUid },
        ]);

        // Flow 2: vertical flow from below (cloud at y=150) - will attach to left, lower position
        const flow2 = makeFlow(flow2Uid, 100, 125, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid },
          { x: 100, y: 150, attachedToUid: 5 },
        ]);

        // Move stock right so both flows become L-shaped and attach to left
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([flow1, flow2]), { x: -50, y: 0 });

        expect(newStock.cx).toBe(150);

        const newFlow1 = newFlows.get(0)!;
        const newFlow2 = newFlows.get(1)!;

        // Get the stock attachment points
        const flow1StockPt = newFlow1.points.get(newFlow1.points.size - 1)!;
        const flow2StockPt = newFlow2.points.get(0)!;

        // Both should be at the left of the stock
        const leftX = 150 - StockWidth / 2;
        expect(flow1StockPt.x).toBe(leftX);
        expect(flow2StockPt.x).toBe(leftX);

        // Flow 1 (adjacent at y=50) should be at 1/3 (upper)
        // Flow 2 (adjacent at y=150) should be at 2/3 (lower)
        const topEdge = 100 - StockHeight / 2;
        const oneThird = topEdge + StockHeight / 3;
        const twoThirds = topEdge + (2 * StockHeight) / 3;

        expect(flow1StockPt.y).toBeCloseTo(oneThird, 1);
        expect(flow2StockPt.y).toBeCloseTo(twoThirds, 1);
      });

      it('should keep single flow centered when only one flow on a side', () => {
        const inflowUid = 2;
        const outflowUid = 3;
        const stock = makeStock(stockUid, 100, 100, [inflowUid], [outflowUid]);

        // Inflow from left - will attach to bottom
        const inflow = makeFlow(inflowUid, 50, 100, [
          { x: 0, y: 100, attachedToUid: 4 },
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
        ]);

        // Outflow to above - will attach to top (different side)
        const outflow = makeFlow(outflowUid, 100, 75, [
          { x: 100, y: 100 - StockHeight / 2, attachedToUid: stockUid },
          { x: 100, y: 50, attachedToUid: 5 },
        ]);

        // Move stock up so inflow becomes L-shaped (attaches to bottom)
        // Outflow remains vertical (attaches to top)
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([inflow, outflow]), { x: 0, y: 50 });

        expect(newStock.cy).toBe(50);

        const newInflow = newFlows.get(0)!;
        const newOutflow = newFlows.get(1)!;

        // Inflow: should be centered on bottom (only one flow on bottom)
        const inflowStockPt = newInflow.points.get(newInflow.points.size - 1)!;
        expect(inflowStockPt.x).toBe(100); // centered
        expect(inflowStockPt.y).toBe(50 + StockHeight / 2); // bottom

        // Outflow: should be centered on top (only one flow on top)
        const outflowStockPt = newOutflow.points.get(0)!;
        expect(outflowStockPt.x).toBe(100); // centered
        expect(outflowStockPt.y).toBe(50 - StockHeight / 2); // top
      });

      it('should not apply spreading to straight flows - they separate by anchor position', () => {
        // Two horizontal flows that both REMAIN STRAIGHT after stock moves
        // Both go to the left side, but at different Y coordinates (based on their anchors)
        const flow1Uid = 2;
        const flow2Uid = 3;
        const stock = makeStock(stockUid, 100, 100, [], [flow1Uid, flow2Uid]);

        // Flow 1: horizontal flow to cloud at y=95 (within stock's vertical extent)
        const flow1 = makeFlow(flow1Uid, 60, 95, [
          { x: 100 - StockWidth / 2, y: 95, attachedToUid: stockUid },
          { x: 20, y: 95, attachedToUid: 4 },
        ]);

        // Flow 2: horizontal flow to cloud at y=105 (also within stock's vertical extent)
        const flow2 = makeFlow(flow2Uid, 60, 105, [
          { x: 100 - StockWidth / 2, y: 105, attachedToUid: stockUid },
          { x: 20, y: 105, attachedToUid: 5 },
        ]);

        // Move stock slightly - both flows should remain straight
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([flow1, flow2]), { x: -10, y: 0 });

        expect(newStock.cx).toBe(110);

        const newFlow1 = newFlows.get(0)!;
        const newFlow2 = newFlows.get(1)!;

        // Both flows should remain 2-point (straight)
        expect(newFlow1.points.size).toBe(2);
        expect(newFlow2.points.size).toBe(2);

        // Stock attachment points should maintain their anchor's Y coordinate
        // (not shifted by spreading offset)
        const flow1StockPt = newFlow1.points.get(0)!;
        const flow2StockPt = newFlow2.points.get(0)!;

        // Y coordinates should match the anchors' Y (not shifted)
        expect(flow1StockPt.y).toBe(95);
        expect(flow2StockPt.y).toBe(105);

        // X should be at the left edge of the new stock position
        expect(flow1StockPt.x).toBe(110 - StockWidth / 2);
        expect(flow2StockPt.x).toBe(110 - StockWidth / 2);
      });

      it('should order pre-existing L-shaped flows by anchor position, not corner', () => {
        // Two pre-existing L-shaped flows that both attach to the bottom.
        // Their corners have the same X coordinate (at stock center), so ordering
        // by corner would give undefined results. Ordering by anchor avoids this.
        const flow1Uid = 2;
        const flow2Uid = 3;
        const stock = makeStock(stockUid, 100, 100, [], [flow1Uid, flow2Uid]);

        // Flow 1: L-shaped, corner at (100, 150), anchor at x=200 (right side)
        // Stock at bottom (100, 100 + StockHeight/2) -> corner (100, 150) -> anchor (200, 150)
        const flow1 = makeFlow(flow1Uid, 100, 125, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid },
          { x: 100, y: 150 }, // corner
          { x: 200, y: 150, attachedToUid: 4 }, // anchor on RIGHT
        ]);

        // Flow 2: L-shaped, corner at (100, 160), anchor at x=0 (left side)
        // Stock at bottom -> corner (100, 160) -> anchor (0, 160)
        const flow2 = makeFlow(flow2Uid, 100, 130, [
          { x: 100, y: 100 + StockHeight / 2, attachedToUid: stockUid },
          { x: 100, y: 160 }, // corner - note both corners have x=100
          { x: 0, y: 160, attachedToUid: 5 }, // anchor on LEFT
        ]);

        // Move stock slightly - both L-shaped flows stay L-shaped
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([flow1, flow2]), { x: 0, y: -10 });

        expect(newStock.cy).toBe(110);

        const newFlow1 = newFlows.get(0)!;
        const newFlow2 = newFlows.get(1)!;

        // Get stock attachment X coordinates
        const flow1StockPt = newFlow1.points.get(0)!;
        const flow2StockPt = newFlow2.points.get(0)!;

        // Flow 2 (anchor at x=0, left) should attach to the LEFT of the bottom edge
        // Flow 1 (anchor at x=200, right) should attach to the RIGHT of the bottom edge
        // So flow2StockPt.x < flow1StockPt.x
        expect(flow2StockPt.x).toBeLessThan(flow1StockPt.x);

        // More specifically, with 2 flows: 1/3 and 2/3 positions
        const leftEdge = 100 - StockWidth / 2;
        const oneThird = leftEdge + StockWidth / 3;
        const twoThirds = leftEdge + (2 * StockWidth) / 3;

        expect(flow2StockPt.x).toBeCloseTo(oneThird, 1); // left anchor -> left position
        expect(flow1StockPt.x).toBeCloseTo(twoThirds, 1); // right anchor -> right position
      });

      it('should include straight flows in spacing to avoid overlap with L-shaped', () => {
        // Scenario: 1 straight flow + 2 L-shaped flows on the left side
        // Without including straight flow in count: L-shaped get 1/3 and 2/3
        // With including straight flow: 3 flows total → slots at 1/4, 2/4, 3/4
        // L-shaped flows get 1/4 and 3/4, avoiding the middle where straight might be
        const straightUid = 2;
        const lshape1Uid = 3;
        const lshape2Uid = 4;
        const stock = makeStock(stockUid, 100, 100, [], [straightUid, lshape1Uid, lshape2Uid]);

        // Straight vertical flow: anchor at y=100 (within stock's extent, so stays straight)
        // Will attach to left side at y = anchor.y = 100 (center of stock)
        const straightFlow = makeFlow(straightUid, 75, 100, [
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 50, y: 100, attachedToUid: 5 },
        ]);

        // L-shaped flow 1: anchor at y=50 (above stock, so becomes L-shaped)
        const lshape1 = makeFlow(lshape1Uid, 75, 75, [
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 50, y: 50, attachedToUid: 6 },
        ]);

        // L-shaped flow 2: anchor at y=150 (below stock, so becomes L-shaped)
        const lshape2 = makeFlow(lshape2Uid, 75, 125, [
          { x: 100 - StockWidth / 2, y: 100, attachedToUid: stockUid },
          { x: 50, y: 150, attachedToUid: 7 },
        ]);

        // Move stock right so all flows attach to left side
        const [newStock, newFlows] = UpdateStockAndFlows(stock, List([straightFlow, lshape1, lshape2]), {
          x: -50,
          y: 0,
        });

        expect(newStock.cx).toBe(150);

        const newStraight = newFlows.get(0)!;
        const newLshape1 = newFlows.get(1)!;
        const newLshape2 = newFlows.get(2)!;

        // Straight flow should remain 2-point
        expect(newStraight.points.size).toBe(2);
        // L-shaped flows should be 3-point
        expect(newLshape1.points.size).toBe(3);
        expect(newLshape2.points.size).toBe(3);

        // Get the Y coordinates of stock attachment points
        const straightY = newStraight.points.get(0)!.y;
        const lshape1Y = newLshape1.points.get(0)!.y;
        const lshape2Y = newLshape2.points.get(0)!.y;

        // Straight flow stays at its anchor Y = 100
        expect(straightY).toBe(100);

        // L-shaped flows should be spread to 1/4 and 3/4, NOT 1/3 and 2/3
        // (because straight flow is included in the count)
        const topEdge = 100 - StockHeight / 2;
        const quarter1 = topEdge + StockHeight / 4;
        const quarter3 = topEdge + (3 * StockHeight) / 4;

        // lshape1 (anchor y=50, topmost) should be at 1/4
        // lshape2 (anchor y=150, bottommost) should be at 3/4
        expect(lshape1Y).toBeCloseTo(quarter1, 1);
        expect(lshape2Y).toBeCloseTo(quarter3, 1);

        // Verify no overlap: all three Y coordinates should be distinct
        expect(straightY).not.toBeCloseTo(lshape1Y, 0);
        expect(straightY).not.toBeCloseTo(lshape2Y, 0);
        expect(lshape1Y).not.toBeCloseTo(lshape2Y, 0);
      });
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

    it('should allow valve to cross corners when dragged past them on L-shaped flow', () => {
      // L-shaped flow: horizontal segment then vertical segment
      // Valve starts at (150, 100) on the horizontal segment
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid }, // anchor
        { x: 200, y: 100 }, // corner
        { x: 200, y: 200, attachedToUid: stockUid }, // stock
      ]);
      const stock = makeStock(stockUid, 200, 200);
      const cloud = makeCloud(cloudUid, flowUid, 100, 100);

      // Drag valve past the corner to (200, 150) on the vertical segment.
      // In UpdateFlow, proposedValve = currentValve - moveDelta, so:
      // moveDelta = { x: 150 - 200, y: 100 - 150 } = { x: -50, y: -50 }
      const [newFlow] = UpdateFlow(flow, List([cloud, stock]), { x: -50, y: -50 });

      // Valve should have crossed to the vertical segment at (200, 150)
      expect(newFlow.cx).toBe(200);
      expect(newFlow.cy).toBe(150);
    });

    it('should allow perpendicular offset on straight horizontal flow with cloud', () => {
      // Straight horizontal flow: cloud to stock
      // This tests the ability to offset a straight flow to avoid overlap
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid }, // cloud
        { x: 200, y: 100, attachedToUid: stockUid }, // stock
      ]);
      const stock = makeStock(stockUid, 200, 100);
      const cloud = makeCloud(cloudUid, flowUid, 100, 100);

      // Drag perpendicular (up) - this should convert to L-shape
      // moveDelta.y = 30 means dragging up (toward lower Y)
      const [newFlow, updatedClouds] = UpdateFlow(flow, List([cloud, stock]), { x: 0, y: 30 });

      // The flow should now be L-shaped (3 points) to accommodate the offset
      expect(newFlow.points.size).toBe(3);

      // Stock endpoint should stay fixed
      const stockPoint = newFlow.points.get(newFlow.points.size - 1)!;
      expect(stockPoint.x).toBe(200);
      expect(stockPoint.y).toBe(100);

      // Cloud endpoint should have moved up
      const cloudPoint = newFlow.points.get(0)!;
      expect(cloudPoint.y).toBe(70); // moved up by 30

      // There should be a corner connecting them
      const corner = newFlow.points.get(1)!;
      expect(corner.y).toBe(70); // same Y as cloud (horizontal segment)
      expect(corner.x).toBe(200); // same X as stock (vertical segment)

      // Cloud position should be updated
      expect(updatedClouds.size).toBe(1);
      expect(updatedClouds.get(0)!.cy).toBe(70);
    });

    it('should allow perpendicular offset on straight vertical flow with cloud', () => {
      // Straight vertical flow: cloud to stock
      const flow = makeFlow(flowUid, 100, 150, [
        { x: 100, y: 100, attachedToUid: cloudUid }, // cloud
        { x: 100, y: 200, attachedToUid: stockUid }, // stock
      ]);
      const stock = makeStock(stockUid, 100, 200);
      const cloud = makeCloud(cloudUid, flowUid, 100, 100);

      // Drag perpendicular (right) - this should convert to L-shape
      // moveDelta.x = -30 means dragging right (toward higher X)
      const [newFlow, updatedClouds] = UpdateFlow(flow, List([cloud, stock]), { x: -30, y: 0 });

      // The flow should now be L-shaped (3 points)
      expect(newFlow.points.size).toBe(3);

      // Stock endpoint should stay fixed
      const stockPoint = newFlow.points.get(newFlow.points.size - 1)!;
      expect(stockPoint.x).toBe(100);
      expect(stockPoint.y).toBe(200);

      // Cloud endpoint should have moved right
      const cloudPoint = newFlow.points.get(0)!;
      expect(cloudPoint.x).toBe(130); // moved right by 30

      // Cloud position should be updated
      expect(updatedClouds.size).toBe(1);
      expect(updatedClouds.get(0)!.cx).toBe(130);
    });

    it('should keep valve on flow when converting straight to L-shape', () => {
      // Straight horizontal flow with valve at midpoint
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 200, 100);
      const cloud = makeCloud(cloudUid, flowUid, 100, 100);

      // Drag perpendicular - converts to L-shape
      const [newFlow] = UpdateFlow(flow, List([cloud, stock]), { x: 0, y: 30 });

      // Valve should be clamped to the closest segment of the new L-shape
      const segments = getSegments(newFlow.points);
      expect(segments.length).toBe(2);

      // Valve should be on one of the segments (either the horizontal or vertical part)
      const valveOnHorizontal = newFlow.cy === 70; // on the horizontal segment at y=70
      const valveOnVertical = newFlow.cx === 200; // on the vertical segment at x=200
      expect(valveOnHorizontal || valveOnVertical).toBe(true);
    });

    it('should not reroute on small perpendicular movement (threshold check)', () => {
      // Straight horizontal flow: cloud to stock
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 200, 100);
      const cloud = makeCloud(cloudUid, flowUid, 100, 100);

      // Small perpendicular movement (below threshold of 5px) should not reroute
      const [newFlow, updatedClouds] = UpdateFlow(flow, List([cloud, stock]), { x: -20, y: 3 });

      // Flow should remain straight (2 points) - not converted to L-shape
      expect(newFlow.points.size).toBe(2);
      // Cloud should not be updated
      expect(updatedClouds.size).toBe(0);
      // Valve should move along the segment
      expect(newFlow.cy).toBe(100);
      expect(newFlow.cx).toBe(170); // moved along segment by x delta
    });

    it('should not reroute when parallel movement is dominant', () => {
      // Straight horizontal flow: cloud to stock
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: cloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);
      const stock = makeStock(stockUid, 200, 100);
      const cloud = makeCloud(cloudUid, flowUid, 100, 100);

      // Even with significant perpendicular movement, if parallel is larger, don't reroute
      const [newFlow, updatedClouds] = UpdateFlow(flow, List([cloud, stock]), { x: -30, y: 20 });

      // Flow should remain straight - parallel movement (30) > perpendicular (20)
      expect(newFlow.points.size).toBe(2);
      expect(updatedClouds.size).toBe(0);
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

  describe('UpdateCloudAndFlow - multi-segment flows', () => {
    it('should not move valve on interior segment when arrowhead moves vertically', () => {
      // 3-point L-shaped flow: source -> corner -> arrowhead (cloud)
      // Segment 0 is horizontal (source to corner), valve is on segment 0
      // Segment 1 is vertical (corner to arrowhead)
      // Moving the arrowhead vertically should NOT move the valve, since it's
      // on segment 0 (the horizontal segment) which doesn't change.
      const sourceUid = 1;
      const cloudUid = 3;
      const sourceX = 100;
      const sourceEdgeX = sourceX + StockWidth / 2;
      const cornerX = 200;
      const cornerY = 100;
      const arrowheadX = cornerX;
      const arrowheadY = 200;

      // Valve at (150, 100) on the horizontal segment
      const valveX = 150;
      const valveY = cornerY;

      const flow = makeFlow(flowUid, valveX, valveY, [
        { x: sourceEdgeX, y: cornerY, attachedToUid: sourceUid }, // source edge
        { x: cornerX, y: cornerY }, // corner
        { x: arrowheadX, y: arrowheadY, attachedToUid: cloudUid }, // arrowhead
      ]);

      // Cloud at arrowhead position
      const cloud = makeCloud(cloudUid, flowUid, arrowheadX, arrowheadY);

      // Move arrowhead down by 50 (moveDelta is inverted, so delta.y = 50 moves down)
      // The original cloud position is (200, 200), new position will be (200, 250)
      const moveDelta = { x: 0, y: -50 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Valve should NOT have moved vertically since it's on a horizontal segment
      // that wasn't affected by the vertical arrowhead movement
      expect(newFlow.cx).toBe(valveX);
      expect(newFlow.cy).toBe(valveY);

      // Cloud should have moved
      expect(newCloud.cy).toBe(arrowheadY + 50);
    });

    it('should preserve valve position on segment 0 when arrowhead segment changes', () => {
      // Same setup as above, but testing that valve fraction is preserved if
      // the arrowhead movement affects segment 0
      const sourceUid = 1;
      const cloudUid = 3;
      const sourceX = 100;
      const sourceEdgeX = sourceX + StockWidth / 2;
      const cornerX = 200;
      const cornerY = 100;
      const arrowheadX = cornerX;
      const arrowheadY = 200;

      // Valve at corner on the horizontal segment
      const valveX = cornerX;
      const valveY = cornerY;

      const flow = makeFlow(flowUid, valveX, valveY, [
        { x: sourceEdgeX, y: cornerY, attachedToUid: sourceUid },
        { x: cornerX, y: cornerY },
        { x: arrowheadX, y: arrowheadY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, arrowheadX, arrowheadY);

      // Move arrowhead vertically
      const moveDelta = { x: 0, y: -50 };
      const [, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Valve on segment 0 should stay put
      expect(newFlow.cy).toBe(valveY);
    });

    it('should update valve when it is on the segment adjacent to the moving arrowhead', () => {
      // 3-point L-shaped flow with valve on segment 1 (adjacent to arrowhead)
      const sourceUid = 1;
      const cloudUid = 3;
      const sourceX = 100;
      const sourceEdgeX = sourceX + StockWidth / 2;
      const cornerX = 200;
      const cornerY = 100;
      const arrowheadX = cornerX;
      const arrowheadY = 200;

      // Valve at (200, 150) on the vertical segment (segment 1)
      const valveX = cornerX;
      const valveY = 150;

      const flow = makeFlow(flowUid, valveX, valveY, [
        { x: sourceEdgeX, y: cornerY, attachedToUid: sourceUid },
        { x: cornerX, y: cornerY },
        { x: arrowheadX, y: arrowheadY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, arrowheadX, arrowheadY);

      // Move arrowhead down by 50
      const moveDelta = { x: 0, y: -50 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Valve is on segment 1 (the segment adjacent to arrowhead), so it should
      // preserve its fractional position. Original segment: corner(200,100) to
      // arrowhead(200,200), length=100. Valve at (200,150) is 50% along.
      // New segment: corner(200,100) to arrowhead(200,250), length=150.
      // New valve should be at 50% = (200, 100 + 0.5*150) = (200, 175)
      expect(newFlow.cx).toBe(valveX);
      expect(newFlow.cy).toBeCloseTo(175, 0);

      // Cloud should have moved down
      expect(newCloud.cy).toBe(250);
    });
  });

  describe('UpdateCloudAndFlow - perpendicular offset', () => {
    it('should create L-shape when cloud dragged perpendicular to horizontal 2-point flow', () => {
      // 2-point horizontal flow: stock -> cloud
      // Drag cloud upward (perpendicular) -> should create 3-point L-shape
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockEdgeX = stockX + StockWidth / 2;
      const cloudX = 200;
      const flowY = 100;

      const flow = makeFlow(flowUid, 150, flowY, [
        { x: stockEdgeX, y: flowY, attachedToUid: stockUid },
        { x: cloudX, y: flowY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, cloudX, flowY);

      // Drag cloud up by 30 (perpendicular to horizontal flow)
      // moveDelta is inverted, so positive y means moving up (to lower Y)
      const moveDelta = { x: 0, y: 30 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should now be L-shaped (3 points)
      expect(newFlow.points.size).toBe(3);

      // Stock endpoint should stay fixed
      const stockPoint = newFlow.points.get(0)!;
      expect(stockPoint.x).toBe(stockEdgeX);
      expect(stockPoint.y).toBe(flowY);
      expect(stockPoint.attachedToUid).toBe(stockUid);

      // Cloud endpoint should have moved up
      const cloudPoint = newFlow.points.get(2)!;
      expect(cloudPoint.y).toBe(flowY - 30);
      expect(cloudPoint.attachedToUid).toBe(cloudUid);

      // Corner should connect them orthogonally
      const corner = newFlow.points.get(1)!;
      expect(corner.y).toBe(cloudPoint.y); // same Y as cloud (horizontal to cloud)
      expect(corner.x).toBe(stockEdgeX); // same X as stock (vertical from stock)

      // Cloud position should be updated
      expect(newCloud.cy).toBe(flowY - 30);
    });

    it('should create L-shape when cloud dragged perpendicular to vertical 2-point flow', () => {
      // 2-point vertical flow: stock -> cloud
      // Drag cloud leftward (perpendicular) -> should create 3-point L-shape
      const stockUid = 1;
      const cloudUid = 3;
      const stockY = 100;
      const stockEdgeY = stockY + StockHeight / 2;
      const cloudY = 200;
      const flowX = 100;

      const flow = makeFlow(flowUid, flowX, 150, [
        { x: flowX, y: stockEdgeY, attachedToUid: stockUid },
        { x: flowX, y: cloudY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, flowX, cloudY);

      // Drag cloud right by 30 (perpendicular to vertical flow)
      // moveDelta is inverted, so negative x means moving right (to higher X)
      const moveDelta = { x: -30, y: 0 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should now be L-shaped (3 points)
      expect(newFlow.points.size).toBe(3);

      // Stock endpoint should stay fixed
      const stockPoint = newFlow.points.get(0)!;
      expect(stockPoint.x).toBe(flowX);
      expect(stockPoint.y).toBe(stockEdgeY);
      expect(stockPoint.attachedToUid).toBe(stockUid);

      // Cloud endpoint should have moved right
      const cloudPoint = newFlow.points.get(2)!;
      expect(cloudPoint.x).toBe(flowX + 30);
      expect(cloudPoint.attachedToUid).toBe(cloudUid);

      // Corner should connect them orthogonally
      const corner = newFlow.points.get(1)!;
      expect(corner.x).toBe(cloudPoint.x); // same X as cloud (vertical to cloud)
      expect(corner.y).toBe(stockEdgeY); // same Y as stock (horizontal from stock)

      // Cloud position should be updated
      expect(newCloud.cx).toBe(flowX + 30);
    });

    it('should not create L-shape for small perpendicular movements (threshold)', () => {
      // 2-point horizontal flow: stock -> cloud
      // Small perpendicular movement < 5px should NOT trigger L-shape
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockEdgeX = stockX + StockWidth / 2;
      const cloudX = 200;
      const flowY = 100;

      const flow = makeFlow(flowUid, 150, flowY, [
        { x: stockEdgeX, y: flowY, attachedToUid: stockUid },
        { x: cloudX, y: flowY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, cloudX, flowY);

      // Small perpendicular movement (3px, below threshold of 5px)
      const moveDelta = { x: 0, y: 3 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should remain straight (2 points)
      expect(newFlow.points.size).toBe(2);

      // Cloud should still be at the same Y (constrained to flow axis)
      expect(newCloud.cy).toBe(flowY);
    });

    it('should not create L-shape when parallel movement dominates', () => {
      // 2-point horizontal flow: stock -> cloud
      // If parallel movement > perpendicular, don't reroute
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockEdgeX = stockX + StockWidth / 2;
      const cloudX = 200;
      const flowY = 100;

      const flow = makeFlow(flowUid, 150, flowY, [
        { x: stockEdgeX, y: flowY, attachedToUid: stockUid },
        { x: cloudX, y: flowY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, cloudX, flowY);

      // Parallel movement (30px) dominates perpendicular (10px)
      const moveDelta = { x: -30, y: 10 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should remain straight (2 points) since parallel dominates
      expect(newFlow.points.size).toBe(2);

      // Cloud should have moved along the flow axis
      expect(newCloud.cx).toBe(cloudX + 30);
      expect(newCloud.cy).toBe(flowY); // constrained to horizontal
    });

    it('should handle source cloud perpendicular offset (cloud at first point)', () => {
      // 2-point horizontal flow: cloud -> stock
      // Drag cloud perpendicular -> should create L-shape with corner near stock
      const stockUid = 1;
      const cloudUid = 3;
      const cloudX = 100;
      const stockX = 200;
      const stockEdgeX = stockX - StockWidth / 2;
      const flowY = 100;

      const flow = makeFlow(flowUid, 150, flowY, [
        { x: cloudX, y: flowY, attachedToUid: cloudUid },
        { x: stockEdgeX, y: flowY, attachedToUid: stockUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, cloudX, flowY);

      // Drag cloud up by 30 (perpendicular to horizontal flow)
      const moveDelta = { x: 0, y: 30 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should now be L-shaped (3 points)
      expect(newFlow.points.size).toBe(3);

      // Cloud endpoint should have moved up
      const cloudPoint = newFlow.points.get(0)!;
      expect(cloudPoint.y).toBe(flowY - 30);
      expect(cloudPoint.attachedToUid).toBe(cloudUid);

      // Stock endpoint should stay fixed
      const stockPoint = newFlow.points.get(2)!;
      expect(stockPoint.x).toBe(stockEdgeX);
      expect(stockPoint.y).toBe(flowY);
      expect(stockPoint.attachedToUid).toBe(stockUid);

      // Corner should connect them orthogonally
      const corner = newFlow.points.get(1)!;
      expect(corner.y).toBe(cloudPoint.y); // same Y as cloud (horizontal from cloud)
      expect(corner.x).toBe(stockEdgeX); // same X as stock (vertical to stock)

      // Cloud position should be updated
      expect(newCloud.cy).toBe(flowY - 30);
    });

    it('should update adjacent corner when dragging cloud on multi-segment flow', () => {
      // 3-point L-shaped flow: stock -> corner -> cloud
      // Dragging cloud should update the adjacent corner while preserving orthogonality
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockEdgeX = stockX + StockWidth / 2;
      const cornerX = 200;
      const cornerY = 100;
      const cloudX = cornerX;
      const cloudY = 200;

      const flow = makeFlow(flowUid, 150, cornerY, [
        { x: stockEdgeX, y: cornerY, attachedToUid: stockUid },
        { x: cornerX, y: cornerY },
        { x: cloudX, y: cloudY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, cloudX, cloudY);

      // Drag cloud horizontally (perpendicular to the vertical segment)
      const moveDelta = { x: -30, y: 0 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should still be L-shaped (3 points)
      expect(newFlow.points.size).toBe(3);

      // Stock endpoint should stay fixed
      const stockPoint = newFlow.points.get(0)!;
      expect(stockPoint.x).toBe(stockEdgeX);
      expect(stockPoint.y).toBe(cornerY);

      // Cloud should have moved horizontally
      const cloudPoint = newFlow.points.get(2)!;
      expect(cloudPoint.x).toBe(cloudX + 30);
      expect(cloudPoint.y).toBe(cloudY);

      // Corner should be updated to maintain orthogonality
      const corner = newFlow.points.get(1)!;
      expect(corner.x).toBe(cloudX + 30); // X updated to match cloud
      expect(corner.y).toBe(cornerY); // Y preserved to maintain horizontal first segment

      // Cloud position should be updated
      expect(newCloud.cx).toBe(cloudX + 30);
    });

    it('should preserve valve fractional position on multi-segment flow when cloud moves', () => {
      // 3-point L-shaped flow with valve on the vertical segment (segment 1)
      // When cloud moves, valve's fractional position along its segment should be preserved
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockEdgeX = stockX + StockWidth / 2;
      const cornerX = 200;
      const cornerY = 100;
      const cloudX = cornerX;
      const cloudY = 200;

      // Valve at (200, 150) - 50% along the vertical segment from corner (200,100) to cloud (200,200)
      const valveX = cornerX;
      const valveY = 150;

      const flow = makeFlow(flowUid, valveX, valveY, [
        { x: stockEdgeX, y: cornerY, attachedToUid: stockUid },
        { x: cornerX, y: cornerY },
        { x: cloudX, y: cloudY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, cloudX, cloudY);

      // Move cloud down by 50 (extending the vertical segment)
      const moveDelta = { x: 0, y: -50 };
      const [, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Valve was at 50% of segment 1 (corner to cloud)
      // Original segment: (200, 100) to (200, 200), length 100, valve at y=150 (50%)
      // New segment: (200, 100) to (200, 250), length 150, valve should be at 50% = y=175
      expect(newFlow.cx).toBe(valveX);
      expect(newFlow.cy).toBeCloseTo(175, 0);
    });

    it('should keep multi-segment endpoints on stock edge when dragging stock-attached source', () => {
      // 3-point L-shaped flow: stock -> corner -> cloud
      // When dragging the source (attached to stock), the endpoint should stay on
      // the stock's edge, not shift to the stock's center.
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockY = 100;
      const stockEdgeX = stockX + StockWidth / 2; // Right edge of stock
      const cornerX = 200;
      const cornerY = stockY; // Horizontal segment from stock edge to corner
      const cloudX = cornerX;
      const cloudY = 200;

      // Flow starts at stock's right edge, goes horizontal to corner, then vertical to cloud
      const flow = makeFlow(flowUid, 150, cornerY, [
        { x: stockEdgeX, y: cornerY, attachedToUid: stockUid },
        { x: cornerX, y: cornerY },
        { x: cloudX, y: cloudY, attachedToUid: cloudUid },
      ]);

      // Create a stock (not a cloud) to simulate dragging the source end
      const stock = makeStock(stockUid, stockX, stockY);

      // Apply zero movement - this simulates "dropping back on the same stock"
      // The endpoint should stay on the stock edge, not shift to stock center
      const moveDelta = { x: 0, y: 0 };
      const [, newFlow] = UpdateCloudAndFlow(stock, flow, moveDelta);

      // The source endpoint should still be on the stock's right edge
      const sourcePoint = newFlow.points.get(0)!;
      expect(sourcePoint.x).toBe(stockEdgeX); // Should be on edge, not stockX (center)
      expect(sourcePoint.y).toBe(cornerY);

      // Corner should be unchanged
      const corner = newFlow.points.get(1)!;
      expect(corner.x).toBe(cornerX);
      expect(corner.y).toBe(cornerY);
    });

    it('should keep multi-segment endpoints on stock edge when dragging stock-attached sink', () => {
      // 3-point L-shaped flow: cloud -> corner -> stock
      // When dragging the sink (attached to stock), the endpoint should stay on
      // the stock's edge, not shift to the stock's center.
      const stockUid = 1;
      const cloudUid = 3;
      const cloudX = 100;
      const cloudY = 100;
      const cornerX = 200;
      const cornerY = cloudY; // Horizontal segment from cloud to corner
      const stockX = 200;
      const stockY = 200;
      const stockEdgeY = stockY - StockHeight / 2; // Top edge of stock

      // Flow starts at cloud, goes horizontal to corner, then vertical to stock's top edge
      const flow = makeFlow(flowUid, 150, cornerY, [
        { x: cloudX, y: cloudY, attachedToUid: cloudUid },
        { x: cornerX, y: cornerY },
        { x: cornerX, y: stockEdgeY, attachedToUid: stockUid },
      ]);

      // Create a stock (not a cloud) to simulate dragging the sink end
      const stock = makeStock(stockUid, stockX, stockY);

      // Apply zero movement - this simulates "dropping back on the same stock"
      const moveDelta = { x: 0, y: 0 };
      const [, newFlow] = UpdateCloudAndFlow(stock, flow, moveDelta);

      // The sink endpoint should still be on the stock's top edge
      const sinkPoint = newFlow.points.get(2)!;
      expect(sinkPoint.x).toBe(cornerX);
      expect(sinkPoint.y).toBe(stockEdgeY); // Should be on edge, not stockY (center)

      // Corner should be unchanged
      const corner = newFlow.points.get(1)!;
      expect(corner.x).toBe(cornerX);
      expect(corner.y).toBe(cornerY);
    });

    it('should apply movement delta to existing endpoint position, not stock center', () => {
      // When dragging a stock-attached endpoint by some delta, the new position
      // should be computed from the current endpoint position (on the edge),
      // not from the stock's center.
      const stockUid = 1;
      const cloudUid = 3;
      const stockX = 100;
      const stockY = 100;
      const stockEdgeX = stockX + StockWidth / 2;
      const cornerX = 200;
      const cornerY = stockY;
      const cloudX = cornerX;
      const cloudY = 200;

      const flow = makeFlow(flowUid, 150, cornerY, [
        { x: stockEdgeX, y: cornerY, attachedToUid: stockUid },
        { x: cornerX, y: cornerY },
        { x: cloudX, y: cloudY, attachedToUid: cloudUid },
      ]);

      const stock = makeStock(stockUid, stockX, stockY);

      // Drag the source down by 20 pixels
      const moveDelta = { x: 0, y: -20 };
      const [, newFlow] = UpdateCloudAndFlow(stock, flow, moveDelta);

      // The source endpoint should move from the edge position, not the center
      // Original position: (stockEdgeX, cornerY) = (130, 100)
      // With moveDelta.y = -20 (inverted, so +20 to Y): new Y should be 120
      const sourcePoint = newFlow.points.get(0)!;
      expect(sourcePoint.x).toBe(stockEdgeX); // X unchanged for vertical movement
      expect(sourcePoint.y).toBe(cornerY + 20); // Y moved from edge position

      // Corner Y should also update to maintain orthogonality (horizontal segment)
      const corner = newFlow.points.get(1)!;
      expect(corner.y).toBe(cornerY + 20);
    });
  });

  describe('UpdateCloudAndFlow - degenerate flow creation', () => {
    // When a flow is first created, both endpoints are at the same position.
    // The segment is both horizontal AND vertical (zero length).
    // The drag direction should determine the flow axis.

    it('should create vertical flow when dragging mostly downward from degenerate start', () => {
      const stockUid = 1;
      const cloudUid = 3;
      const startX = 100;
      const startY = 100;

      // Degenerate flow: both points at same position
      const flow = makeFlow(flowUid, startX, startY, [
        { x: startX, y: startY, attachedToUid: stockUid },
        { x: startX, y: startY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, startX, startY);

      // Drag mostly downward (negative moveDelta.y means moving down in screen coords)
      // moveDelta is inverted: negative y means moving to higher Y
      const moveDelta = { x: -5, y: -50 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should remain straight (2 points) and be vertical
      expect(newFlow.points.size).toBe(2);

      // Both points should have the same X (vertical flow)
      const firstPt = newFlow.points.get(0)!;
      const lastPt = newFlow.points.get(1)!;
      expect(firstPt.x).toBe(lastPt.x);

      // Cloud should have moved down (Y increased)
      expect(newCloud.cy).toBe(startY + 50);
    });

    it('should create vertical flow when dragging mostly upward from degenerate start', () => {
      const stockUid = 1;
      const cloudUid = 3;
      const startX = 100;
      const startY = 100;

      const flow = makeFlow(flowUid, startX, startY, [
        { x: startX, y: startY, attachedToUid: stockUid },
        { x: startX, y: startY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, startX, startY);

      // Drag mostly upward (positive moveDelta.y means moving up)
      const moveDelta = { x: 5, y: 50 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should remain straight and be vertical
      expect(newFlow.points.size).toBe(2);

      const firstPt = newFlow.points.get(0)!;
      const lastPt = newFlow.points.get(1)!;
      expect(firstPt.x).toBe(lastPt.x);

      // Cloud should have moved up (Y decreased)
      expect(newCloud.cy).toBe(startY - 50);
    });

    it('should create horizontal flow when dragging mostly rightward from degenerate start', () => {
      const stockUid = 1;
      const cloudUid = 3;
      const startX = 100;
      const startY = 100;

      const flow = makeFlow(flowUid, startX, startY, [
        { x: startX, y: startY, attachedToUid: stockUid },
        { x: startX, y: startY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, startX, startY);

      // Drag mostly rightward (negative moveDelta.x means moving right)
      const moveDelta = { x: -50, y: -5 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should remain straight and be horizontal
      expect(newFlow.points.size).toBe(2);

      const firstPt = newFlow.points.get(0)!;
      const lastPt = newFlow.points.get(1)!;
      expect(firstPt.y).toBe(lastPt.y);

      // Cloud should have moved right (X increased)
      expect(newCloud.cx).toBe(startX + 50);
    });

    it('should create horizontal flow when dragging mostly leftward from degenerate start', () => {
      const stockUid = 1;
      const cloudUid = 3;
      const startX = 100;
      const startY = 100;

      const flow = makeFlow(flowUid, startX, startY, [
        { x: startX, y: startY, attachedToUid: stockUid },
        { x: startX, y: startY, attachedToUid: cloudUid },
      ]);

      const cloud = makeCloud(cloudUid, flowUid, startX, startY);

      // Drag mostly leftward (positive moveDelta.x means moving left)
      const moveDelta = { x: 50, y: 5 };
      const [newCloud, newFlow] = UpdateCloudAndFlow(cloud, flow, moveDelta);

      // Flow should remain straight and be horizontal
      expect(newFlow.points.size).toBe(2);

      const firstPt = newFlow.points.get(0)!;
      const lastPt = newFlow.points.get(1)!;
      expect(firstPt.y).toBe(lastPt.y);

      // Cloud should have moved left (X decreased)
      expect(newCloud.cx).toBe(startX - 50);
    });
  });

  describe('UpdateCloudAndFlow - stock edge recomputation', () => {
    it('should recompute stock edge when reattaching to stock on opposite side of corner', () => {
      // Scenario: L-shaped flow with source on left, corner in middle, sink on right
      // Original source stock is LEFT of corner (endpoint on stock's right edge)
      // New source stock is RIGHT of corner (endpoint should be on stock's LEFT edge)
      //
      // Before:  [Stock1] ---> corner
      //                          |
      //                          v
      //                        sink
      //
      // After:   corner <--- [Stock2]
      //            |
      //            v
      //          sink
      //
      // The endpoint should be on the LEFT edge of Stock2, not preserve the
      // "right edge" offset from Stock1.

      const oldStockUid = 1;
      const sinkUid = 3;

      // Old stock at (100, 100), endpoint on right edge at (100 + StockWidth/2, 100)
      const oldStockX = 100;
      const oldStockY = 100;
      const oldStockRightEdge = oldStockX + StockWidth / 2;

      // Corner at (200, 100) - to the right of old stock
      const cornerX = 200;
      const cornerY = oldStockY;

      // Sink at (200, 200) - below corner
      const sinkX = cornerX;
      const sinkY = 200;

      // New stock at (300, 100) - to the RIGHT of the corner
      // The flow should exit from its LEFT edge (toward the corner)
      const newStockX = 300;
      const newStockY = 100;
      const newStockLeftEdge = newStockX - StockWidth / 2;

      // Create the flow: source -> corner -> sink
      const flow = makeFlow(flowUid, 150, 100, [
        { x: oldStockRightEdge, y: oldStockY, attachedToUid: oldStockUid },
        { x: cornerX, y: cornerY, attachedToUid: undefined },
        { x: sinkX, y: sinkY, attachedToUid: sinkUid },
      ]);

      // Create the stock at OLD coordinates, as Editor.tsx does when calling UpdateCloudAndFlow.
      // Editor.tsx resets the stock back to old coordinates before calling the function.
      const stock = new StockViewElement({
        uid: oldStockUid, // Same UID since we're simulating reattachment
        name: 'Stock',
        ident: 'stock',
        var: undefined,
        x: oldStockX, // Passed with OLD coordinates
        y: oldStockY,
        labelSide: 'center',
        isZeroRadius: false,
        inflows: List([]),
        outflows: List([flowUid]),
      });

      // moveDelta = oldCenter - newCenter (as computed in Editor.tsx)
      // old: (100, 100), new: (300, 100) -> moveDelta = (100 - 300, 100 - 100) = (-200, 0)
      // So newCenter = cloud.cx - moveDelta.x = 100 - (-200) = 300
      const moveDelta = { x: oldStockX - newStockX, y: oldStockY - newStockY };

      const [, newFlow] = UpdateCloudAndFlow(stock, flow, moveDelta);

      // The endpoint should be on the LEFT edge of the new stock (facing the corner)
      const firstPt = newFlow.points.get(0)!;
      expect(firstPt.x).toBe(newStockLeftEdge);
      expect(firstPt.y).toBe(newStockY);

      // The corner should maintain orthogonality with the new endpoint
      const secondPt = newFlow.points.get(1)!;
      expect(secondPt.y).toBe(firstPt.y); // Same Y for horizontal segment
    });

    it('should recompute stock edge for vertical segments when reattaching', () => {
      // Similar test but with a vertical first segment
      // Source stock above corner, new stock below corner

      const oldStockUid = 1;
      const sinkUid = 3;

      // Old stock at (100, 50), endpoint on bottom edge
      const oldStockX = 100;
      const oldStockY = 50;
      const oldStockBottomEdge = oldStockY + StockHeight / 2;

      // Corner at (100, 150) - below old stock
      const cornerX = oldStockX;
      const cornerY = 150;

      // Sink at (200, 150) - to the right of corner
      const sinkX = 200;
      const sinkY = cornerY;

      // New stock at (100, 250) - BELOW the corner
      // The flow should exit from its TOP edge (toward the corner)
      const newStockX = 100;
      const newStockY = 250;
      const newStockTopEdge = newStockY - StockHeight / 2;

      // Create the flow: source -> corner -> sink
      const flow = makeFlow(flowUid, 100, 100, [
        { x: oldStockX, y: oldStockBottomEdge, attachedToUid: oldStockUid },
        { x: cornerX, y: cornerY, attachedToUid: undefined },
        { x: sinkX, y: sinkY, attachedToUid: sinkUid },
      ]);

      // Create the stock at OLD coordinates, as Editor.tsx does when calling UpdateCloudAndFlow.
      const stock = new StockViewElement({
        uid: oldStockUid,
        name: 'Stock',
        ident: 'stock',
        var: undefined,
        x: oldStockX, // Passed with OLD coordinates
        y: oldStockY,
        labelSide: 'center',
        isZeroRadius: false,
        inflows: List([]),
        outflows: List([flowUid]),
      });

      // moveDelta = oldCenter - newCenter (as computed in Editor.tsx)
      // old: (100, 50), new: (100, 250) -> moveDelta = (0, -200)
      // So newCenterY = cloud.cy - moveDelta.y = 50 - (-200) = 250
      const moveDelta = { x: oldStockX - newStockX, y: oldStockY - newStockY };

      const [, newFlow] = UpdateCloudAndFlow(stock, flow, moveDelta);

      // The endpoint should be on the TOP edge of the new stock (facing the corner)
      const firstPt = newFlow.points.get(0)!;
      expect(firstPt.x).toBe(newStockX);
      expect(firstPt.y).toBe(newStockTopEdge);

      // The corner should maintain orthogonality with the new endpoint
      const secondPt = newFlow.points.get(1)!;
      expect(secondPt.x).toBe(firstPt.x); // Same X for vertical segment
    });

    it('should treat isZeroRadius stocks as clouds (simple translation)', () => {
      // When detaching a flow from a stock, Canvas creates a temporary placeholder
      // with isZeroRadius: true. This should be treated as a cloud, not go through
      // stock-edge logic, so the endpoint tracks the drag position directly.

      const stockUid = 1;
      const sinkUid = 3;

      // Stock at (100, 100), endpoint on right edge
      const stockX = 100;
      const stockY = 100;
      const stockRightEdge = stockX + StockWidth / 2;

      // Corner at (200, 100)
      const cornerX = 200;
      const cornerY = stockY;

      // Sink at (200, 200)
      const sinkX = cornerX;
      const sinkY = 200;

      // Create the flow: source -> corner -> sink
      const flow = makeFlow(flowUid, 150, 100, [
        { x: stockRightEdge, y: stockY, attachedToUid: stockUid },
        { x: cornerX, y: cornerY, attachedToUid: undefined },
        { x: sinkX, y: sinkY, attachedToUid: sinkUid },
      ]);

      // Create a zero-radius placeholder (simulating drag detachment)
      // Position at (150, 80) - somewhere the user is dragging to
      const dragX = 150;
      const dragY = 80;
      const zeroRadiusPlaceholder = new StockViewElement({
        uid: stockUid,
        name: 'DragPlaceholder',
        ident: 'drag_placeholder',
        var: undefined,
        x: stockX, // OLD position (as passed by Editor.tsx)
        y: stockY,
        labelSide: 'center',
        isZeroRadius: true, // Key: this makes it a drag placeholder
        inflows: List([]),
        outflows: List([flowUid]),
      });

      // moveDelta = oldCenter - newCenter
      // Dragging from (100, 100) to (150, 80) -> moveDelta = (100-150, 100-80) = (-50, 20)
      const moveDelta = { x: stockX - dragX, y: stockY - dragY };

      const [, newFlow] = UpdateCloudAndFlow(zeroRadiusPlaceholder, flow, moveDelta);

      // For isZeroRadius, endpoint should simply translate (like a cloud)
      // newX = stockRightEdge - moveDelta.x = 122.5 - (-50) = 172.5
      // newY = stockY - moveDelta.y = 100 - 20 = 80
      const firstPt = newFlow.points.get(0)!;
      expect(firstPt.x).toBe(stockRightEdge - moveDelta.x);
      expect(firstPt.y).toBe(stockY - moveDelta.y);
    });
  });
});
