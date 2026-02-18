// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  FlowViewElement,
  StockViewElement,
  CloudViewElement,
  AuxViewElement,
  LinkViewElement,
  ModuleViewElement,
  AliasViewElement,
  GroupViewElement,
  ViewElement,
  UID,
} from '@simlin/core/datamodel';

import { StockWidth } from '../drawing/Stock';
import { applyGroupMovement } from '../group-movement';

// Helper functions to create test elements
function makeStock(
  uid: number,
  x: number,
  y: number,
  inflows: number[] = [],
  outflows: number[] = [],
): StockViewElement {
  return {
    type: 'stock',
    uid,
    name: `Stock${uid}`,
    ident: `stock_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
    inflows,
    outflows,
  };
}

function makeFlow(
  uid: number,
  x: number,
  y: number,
  points: Array<{ x: number; y: number; attachedToUid?: number }>,
): FlowViewElement {
  return {
    type: 'flow',
    uid,
    name: `Flow${uid}`,
    ident: `flow_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    points: points.map((p) => ({ x: p.x, y: p.y, attachedToUid: p.attachedToUid })),
    isZeroRadius: false,
  };
}

function makeCloud(uid: number, flowUid: number, x: number, y: number): CloudViewElement {
  return {
    type: 'cloud',
    uid,
    flowUid,
    x,
    y,
    isZeroRadius: false,
    ident: undefined,
  };
}

function makeAux(uid: number, x: number, y: number, isArrayed = false): AuxViewElement {
  const auxVar = isArrayed
    ? {
        type: 'aux' as const,
        ident: `aux_${uid}`,
        equation: {
          type: 'applyToAll' as const,
          dimensionNames: ['dim1'],
          equation: '1',
        },
        documentation: '',
        units: '',
        gf: undefined,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      }
    : undefined;
  return {
    type: 'aux',
    uid,
    name: `Aux${uid}`,
    ident: `aux_${uid}`,
    var: auxVar,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
  };
}

function makeLink(uid: number, fromUid: number, toUid: number, arc: number = 0): LinkViewElement {
  return {
    type: 'link',
    uid,
    fromUid,
    toUid,
    arc,
    isStraight: false,
    multiPoint: undefined,
    polarity: undefined,
    x: 0,
    y: 0,
    isZeroRadius: false,
    ident: undefined,
  };
}

function makeModule(uid: number, x: number, y: number): ModuleViewElement {
  return {
    type: 'module',
    uid,
    name: `Module${uid}`,
    ident: `module_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
  };
}

function makeAlias(uid: number, aliasOfUid: number, x: number, y: number): AliasViewElement {
  return {
    type: 'alias',
    uid,
    aliasOfUid,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
    ident: undefined,
  };
}

function makeGroup(uid: number, x: number, y: number, width: number = 100, height: number = 80): GroupViewElement {
  return {
    type: 'group',
    uid,
    name: `Group${uid}`,
    x,
    y,
    width,
    height,
    isZeroRadius: false,
    ident: undefined,
  };
}

interface Point2D {
  x: number;
  y: number;
}

/**
 * Test helper that wraps the actual applyGroupMovement function.
 * Takes elements as a Map (for convenience in tests) and returns a Map
 * with all elements (original + updated).
 */
function testApplyGroupMovement(
  elements: Map<UID, ViewElement>,
  selection: Set<UID>,
  delta: Point2D,
  arcPoint?: Point2D,
): Map<UID, ViewElement> {
  const { updatedElements } = applyGroupMovement({
    elements: elements.values(),
    selection,
    delta,
    arcPoint,
  });

  // Merge updates back into the original elements map
  const result = new Map(elements);
  for (const [uid, el] of updatedElements) {
    result.set(uid, el);
  }
  return result;
}

describe('Group Movement', () => {
  describe('Chain of stocks and flows all in selection', () => {
    it('should translate entire chain uniformly when all elements are selected', () => {
      // Setup: Stock A -> Flow 1 -> Stock B
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, stockB]]);

      const selection = new Set<UID>([1, 2, 3]);
      const delta = { x: -50, y: -30 }; // Move right 50, down 30

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock A should move
      const newStockA = result.get(1) as StockViewElement;
      expect(newStockA.x).toBe(150); // 100 + 50
      expect(newStockA.y).toBe(130); // 100 + 30

      // Stock B should move
      const newStockB = result.get(3) as StockViewElement;
      expect(newStockB.x).toBe(250); // 200 + 50
      expect(newStockB.y).toBe(130); // 100 + 30

      // Flow valve should move
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.x).toBe(200); // 150 + 50
      expect(newFlow.y).toBe(130); // 100 + 30

      // Flow endpoints should move too
      const newPoints = newFlow.points;
      expect(newPoints[0].x).toBe(100 + StockWidth / 2 + 50);
      expect(newPoints[0].y).toBe(130);
      expect(newPoints[newPoints.length - 1].x).toBe(200 - StockWidth / 2 + 50);
      expect(newPoints[newPoints.length - 1].y).toBe(130);
    });

    it('should preserve relative positions in a longer chain', () => {
      // Setup: Stock A -> Flow 1 -> Stock B -> Flow 2 -> Stock C
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 200, 100, [2], [4]);
      const stockC = makeStock(5, 300, 100, [4], []);
      const flow1 = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);
      const flow2 = makeFlow(4, 250, 100, [
        { x: 200 + StockWidth / 2, y: 100, attachedToUid: 3 },
        { x: 300 - StockWidth / 2, y: 100, attachedToUid: 5 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow1], [3, stockB], [4, flow2], [5, stockC]]);

      const selection = new Set<UID>([1, 2, 3, 4, 5]);
      const delta = { x: -100, y: 0 }; // Move right 100

      const result = testApplyGroupMovement(elements, selection, delta);

      // All stocks should move by same amount
      expect((result.get(1) as StockViewElement).x).toBe(200);
      expect((result.get(3) as StockViewElement).x).toBe(300);
      expect((result.get(5) as StockViewElement).x).toBe(400);

      // All flows should move by same amount
      expect((result.get(2) as FlowViewElement).x).toBe(250);
      expect((result.get(4) as FlowViewElement).x).toBe(350);
    });
  });

  describe('Stock in selection, attached flow not in selection', () => {
    it('should adjust flow when stock moves but flow is not selected', () => {
      // Setup: Stock A -> Flow 1 (not selected) -> Cloud
      const stockA = makeStock(1, 100, 100, [], [2]);
      const cloud = makeCloud(3, 2, 200, 100);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, cloud]]);

      // Only select the stock, not the flow
      const selection = new Set<UID>([1]);
      const delta = { x: -50, y: 0 }; // Move stock right 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock should move
      const newStock = result.get(1) as StockViewElement;
      expect(newStock.x).toBe(150);

      // Flow should be adjusted (routed from new stock position to fixed cloud)
      const newFlow = result.get(2) as FlowViewElement;
      // The source point should be updated to connect to new stock position
      expect(newFlow.points[0].attachedToUid).toBe(1);
      // IMPORTANT: Verify the source point x-coordinate is at the NEW stock edge
      // (not double-moved). Stock moved from 100 to 150, so source point should
      // be at 150 + StockWidth/2.
      expect(newFlow.points[0].x).toBe(150 + StockWidth / 2);
      // The sink point should still be at cloud
      expect(newFlow.points[newFlow.points.length - 1].attachedToUid).toBe(3);
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200);
    });
  });

  describe('Offset preservation with translated and routed flows', () => {
    it('should compute offsets for all stocks regardless of iteration order', () => {
      // This test verifies that computePreRoutedOffsets correctly handles its
      // `elements` iterable even when it's a single-use iterator (like Map.values()).
      // The bug: elements was iterated twice (outer loop for stocks, inner loop for flows).
      // When applyGroupMovement calls computePreRoutedOffsets with originalElements.values(),
      // the inner loop continues from the iterator's current position instead of restarting,
      // causing flows appearing earlier in iteration order to be skipped for later stocks.
      //
      // Setup: Two stocks (A and B), each with TWO outflows to clouds.
      // Stock A has flows 1 and 2. Stock B has flows 3 and 4.
      // When both stocks move, ALL flows should get proper offset computation.
      // Without the fix, Stock B's flows might have incorrect/missing offsets because
      // the flows for Stock A were already consumed by the iterator.

      const stockA = makeStock(1, 100, 100, [], [10, 11]);
      const stockB = makeStock(2, 300, 100, [], [20, 21]);

      // Stock A's two outflows
      const cloudA1 = makeCloud(30, 10, 200, 80);
      const cloudA2 = makeCloud(31, 11, 200, 120);
      const flowA1 = makeFlow(10, 150, 90, [
        { x: 100 + StockWidth / 2, y: 90, attachedToUid: 1 },
        { x: 200, y: 80, attachedToUid: 30 },
      ]);
      const flowA2 = makeFlow(11, 150, 110, [
        { x: 100 + StockWidth / 2, y: 110, attachedToUid: 1 },
        { x: 200, y: 120, attachedToUid: 31 },
      ]);

      // Stock B's two outflows
      const cloudB1 = makeCloud(40, 20, 400, 80);
      const cloudB2 = makeCloud(41, 21, 400, 120);
      const flowB1 = makeFlow(20, 350, 90, [
        { x: 300 + StockWidth / 2, y: 90, attachedToUid: 2 },
        { x: 400, y: 80, attachedToUid: 40 },
      ]);
      const flowB2 = makeFlow(21, 350, 110, [
        { x: 300 + StockWidth / 2, y: 110, attachedToUid: 2 },
        { x: 400, y: 120, attachedToUid: 41 },
      ]);

      const elements = new Map<UID, ViewElement>([
        [1, stockA],
        [2, stockB],
        [10, flowA1],
        [11, flowA2],
        [20, flowB1],
        [21, flowB2],
        [30, cloudA1],
        [31, cloudA2],
        [40, cloudB1],
        [41, cloudB2],
      ]);

      // Select both stocks (not the flows or clouds)
      const selection = new Set<UID>([1, 2]);
      const delta = { x: -50, y: 0 }; // Move stocks right by 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Both stocks should move
      expect((result.get(1) as StockViewElement).x).toBe(150);
      expect((result.get(2) as StockViewElement).x).toBe(350);

      // All four flows should be routed and have DIFFERENT y-coordinates on their stock edges.
      // This verifies that offsets were computed correctly for both stocks.

      // Stock A's flows should have different y-coordinates at the stock edge
      const newFlowA1 = result.get(10) as FlowViewElement;
      const newFlowA2 = result.get(11) as FlowViewElement;
      const flowA1SourceY = newFlowA1.points[0].y;
      const flowA2SourceY = newFlowA2.points[0].y;
      expect(flowA1SourceY).not.toBe(flowA2SourceY);

      // Stock B's flows should have different y-coordinates at the stock edge
      const newFlowB1 = result.get(20) as FlowViewElement;
      const newFlowB2 = result.get(21) as FlowViewElement;
      const flowB1SourceY = newFlowB1.points[0].y;
      const flowB2SourceY = newFlowB2.points[0].y;
      // This assertion will FAIL if the iterator bug exists, because Stock B's
      // flows won't have proper offsets computed (inner loop consumed iterator)
      expect(flowB1SourceY).not.toBe(flowB2SourceY);
    });

    it('should preserve flow spacing when stock has both translated and routed flows', () => {
      // Setup: Stock A with two outflows - one to another selected stock (translated),
      // one to a non-selected cloud (routed). Both flows should maintain proper spacing.
      //
      // Stock A (selected) -> Flow 1 (both endpoints selected) -> Stock B (selected)
      // Stock A (selected) -> Flow 2 (one endpoint selected) -> Cloud (not selected)
      //
      // When Stock A moves, Flow 1 translates and Flow 2 is routed. Both should
      // maintain their relative positions on Stock A's right edge.

      const stockA = makeStock(1, 100, 100, [], [2, 3]);
      const stockB = makeStock(4, 200, 50, [2], []);
      const cloud = makeCloud(5, 3, 200, 150);

      // Flow 1: Stock A -> Stock B (horizontal, from right side of A)
      const flow1 = makeFlow(2, 150, 75, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 50, attachedToUid: 4 },
      ]);

      // Flow 2: Stock A -> Cloud (horizontal, from right side of A)
      const flow2 = makeFlow(3, 150, 125, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200, y: 150, attachedToUid: 5 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow1], [3, flow2], [4, stockB], [5, cloud]]);

      // Select Stock A and Stock B (so Flow 1 has both endpoints selected)
      // Don't select the cloud (so Flow 2 has only one endpoint selected)
      const selection = new Set<UID>([1, 4]);
      const delta = { x: -50, y: 0 }; // Move stocks right 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Both stocks should move
      const newStockA = result.get(1) as StockViewElement;
      const newStockB = result.get(4) as StockViewElement;
      expect(newStockA.x).toBe(150);
      expect(newStockB.x).toBe(250);

      // Flow 1 should translate uniformly (both endpoints moved)
      const newFlow1 = result.get(2) as FlowViewElement;
      expect(newFlow1.points[0].x).toBe(150 + StockWidth / 2);
      expect(newFlow1.points[newFlow1.points.length - 1].x).toBe(250 - StockWidth / 2);

      // Flow 2 should be routed (one endpoint moved, one fixed)
      const newFlow2 = result.get(3) as FlowViewElement;
      expect(newFlow2.points[0].x).toBe(150 + StockWidth / 2); // at new stock position
      expect(newFlow2.points[newFlow2.points.length - 1].x).toBe(200); // cloud unchanged

      // Both flows should have different y-coordinates on Stock A's edge
      // (i.e., they should not overlap)
      const flow1SourceY = newFlow1.points[0].y;
      const flow2SourceY = newFlow2.points[0].y;
      expect(flow1SourceY).not.toBe(flow2SourceY);
    });
  });

  describe('Cloud-to-cloud flow movement', () => {
    it('should move entire flow and both clouds when cloud-cloud flow selected alone', () => {
      // Setup: Cloud A -> Flow -> Cloud B (both clouds, no stocks)
      // This tests the regression where cloud-cloud flows only moved the valve
      // instead of translating the entire flow + both clouds together.
      const cloudA = makeCloud(1, 2, 100, 100);
      const cloudB = makeCloud(3, 2, 200, 100);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloudA], [2, flow], [3, cloudB]]);

      // Only select the flow (not the clouds)
      const selection = new Set<UID>([2]);
      const delta = { x: -50, y: -30 }; // Move right 50, down 30

      const result = testApplyGroupMovement(elements, selection, delta);

      // Both clouds should move along with the flow
      const newCloudA = result.get(1) as CloudViewElement;
      expect(newCloudA.x).toBe(150); // 100 + 50
      expect(newCloudA.y).toBe(130); // 100 + 30

      const newCloudB = result.get(3) as CloudViewElement;
      expect(newCloudB.x).toBe(250); // 200 + 50
      expect(newCloudB.y).toBe(130); // 100 + 30

      // Flow valve should move
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.x).toBe(200); // 150 + 50
      expect(newFlow.y).toBe(130); // 100 + 30

      // Flow endpoints should move too
      expect(newFlow.points[0].x).toBe(150);
      expect(newFlow.points[0].y).toBe(130);
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(250);
      expect(newFlow.points[newFlow.points.length - 1].y).toBe(130);
    });

    it('should move L-shaped cloud-cloud flow uniformly', () => {
      // Setup: Cloud A -> L-shaped Flow (3 points) -> Cloud B
      const cloudA = makeCloud(1, 2, 100, 100);
      const cloudB = makeCloud(3, 2, 200, 200);
      // L-shaped flow with corner at (100, 200)
      const flow = makeFlow(2, 100, 150, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 100, y: 200 }, // corner point
        { x: 200, y: 200, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloudA], [2, flow], [3, cloudB]]);

      // Only select the flow
      const selection = new Set<UID>([2]);
      const delta = { x: -25, y: -25 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Both clouds should move
      expect((result.get(1) as CloudViewElement).x).toBe(125);
      expect((result.get(1) as CloudViewElement).y).toBe(125);
      expect((result.get(3) as CloudViewElement).x).toBe(225);
      expect((result.get(3) as CloudViewElement).y).toBe(225);

      // Flow valve and all points should move
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.x).toBe(125);
      expect(newFlow.y).toBe(175);
      expect(newFlow.points[0].x).toBe(125);
      expect(newFlow.points[0].y).toBe(125);
      expect(newFlow.points[1].x).toBe(125);
      expect(newFlow.points[1].y).toBe(225);
      expect(newFlow.points[2].x).toBe(225);
      expect(newFlow.points[2].y).toBe(225);
    });
  });

  describe('Cloud-stock flow perpendicular drag behavior', () => {
    it('should create L-shape when cloud-stock flow in group selection dragged perpendicular', () => {
      // Setup: Cloud -> Flow (selected) -> Stock, horizontal flow
      // This tests the bug where selecting a flow with a cloud endpoint as part
      // of a group (e.g., flow + aux) prevents perpendicular drag rerouting.
      // The cloud should move and the flow should become L-shaped, same as single-flow selection.
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);
      // Add an aux that's also selected (making this a group selection)
      const aux = makeAux(4, 300, 200);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock], [4, aux]]);

      // Select the flow AND the aux (group selection, but neither flow endpoint is selected)
      const selection = new Set<UID>([2, 4]);
      // Drag DOWN 30 (perpendicular to horizontal flow, > 5px threshold)
      // delta is subtracted, so y: -30 moves down
      const delta = { x: 0, y: -30 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock should NOT move (not selected)
      expect((result.get(3) as StockViewElement).x).toBe(200);
      expect((result.get(3) as StockViewElement).y).toBe(100);

      // Aux should move with the group
      expect((result.get(4) as AuxViewElement).x).toBe(300);
      expect((result.get(4) as AuxViewElement).y).toBe(230);

      // Cloud should move down (perpendicular to flow direction)
      const newCloud = result.get(1) as CloudViewElement;
      expect(newCloud.x).toBe(100); // x unchanged
      expect(newCloud.y).toBe(130); // moved down 30

      // Flow should become L-shaped (3 points)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.length).toBe(3);

      // First point: at cloud's new position
      expect(newFlow.points[0].x).toBe(100);
      expect(newFlow.points[0].y).toBe(130);

      // Middle point: corner (at stock's x, cloud's new y)
      expect(newFlow.points[1].x).toBe(200 - StockWidth / 2);
      expect(newFlow.points[1].y).toBe(130);

      // Last point: at stock (unchanged)
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200 - StockWidth / 2);
      expect(newFlow.points[newFlow.points.length - 1].y).toBe(100);
    });

    it('should create L-shape when single cloud-stock flow dragged perpendicular', () => {
      // Setup: Cloud -> Flow (selected) -> Stock, horizontal flow
      // This tests the behavior from UpdateFlow where perpendicular drag
      // converts a 2-point flow to an L-shape and moves the cloud endpoint.
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock]]);

      // Only select the flow (single-flow selection, not group movement)
      const selection = new Set<UID>([2]);
      // Drag DOWN 30 (perpendicular to horizontal flow, > 5px threshold)
      // delta is subtracted, so y: -30 moves down
      const delta = { x: 0, y: -30 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock should NOT move
      expect((result.get(3) as StockViewElement).x).toBe(200);
      expect((result.get(3) as StockViewElement).y).toBe(100);

      // Cloud should move down (perpendicular to flow direction)
      const newCloud = result.get(1) as CloudViewElement;
      expect(newCloud.x).toBe(100); // x unchanged
      expect(newCloud.y).toBe(130); // moved down 30

      // Flow should become L-shaped (3 points)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.length).toBe(3);

      // First point: at cloud's new position
      expect(newFlow.points[0].x).toBe(100);
      expect(newFlow.points[0].y).toBe(130);

      // Middle point: corner (at stock's x, cloud's new y)
      expect(newFlow.points[1].x).toBe(200 - StockWidth / 2);
      expect(newFlow.points[1].y).toBe(130);

      // Last point: at stock (unchanged)
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200 - StockWidth / 2);
      expect(newFlow.points[newFlow.points.length - 1].y).toBe(100);
    });

    it('should not create L-shape when perpendicular drag is too small', () => {
      // When perpendicular movement is < 5px, the flow should remain 2-point
      // and just clamp the valve (no L-shape conversion)
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock]]);

      // Small perpendicular drag (< 5px threshold)
      const selection = new Set<UID>([2]);
      const delta = { x: 0, y: -3 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Flow should remain 2-point (no L-shape)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.length).toBe(2);

      // Cloud should NOT move
      expect((result.get(1) as CloudViewElement).y).toBe(100);
    });

    it('should slide valve along path when single cloud-stock flow dragged parallel', () => {
      // When drag is parallel to the flow (not perpendicular), valve should
      // slide along the flow path, not convert to L-shape
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 140, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock]]);

      // Parallel drag (along the flow)
      const selection = new Set<UID>([2]);
      const delta = { x: -20, y: 0 }; // Move right 20

      const result = testApplyGroupMovement(elements, selection, delta);

      // Flow should remain 2-point
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.length).toBe(2);

      // Valve should move right (clamped to flow path)
      // Original: 140, proposed: 160
      // Flow spans from 100 to 177.5 (stock edge minus 10px margin = 167.5)
      expect(newFlow.x).toBe(160);
      expect(newFlow.y).toBe(100);

      // Cloud should NOT move (parallel drag)
      expect((result.get(1) as CloudViewElement).x).toBe(100);
    });
  });

  describe('Flow in selection, endpoint stocks not in selection', () => {
    it('should only move valve when flow selected but attached stocks are not', () => {
      // Setup: Stock A (not selected) -> Flow 1 (selected) -> Stock B (not selected)
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, stockB]]);

      // Only select the flow
      const selection = new Set<UID>([2]);
      const delta = { x: -20, y: 0 }; // Move right 20

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stocks should NOT move
      expect((result.get(1) as StockViewElement).x).toBe(100);
      expect((result.get(3) as StockViewElement).x).toBe(200);

      // Flow valve should move but be clamped to stay on the flow path
      // Flow goes from x=122.5 to x=177.5 (with 10px margin, max is 167.5)
      // Proposed position is 170, so it gets clamped to 167.5
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.x).toBe(167.5); // Clamped to stay within flow bounds

      // Flow endpoints should stay fixed (attached to stocks)
      expect(newFlow.points[0].x).toBe(100 + StockWidth / 2);
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200 - StockWidth / 2);
    });

    it('should clamp valve to flow path when moving perpendicular to flow direction', () => {
      // Setup: Stock A -> horizontal Flow -> Stock B
      // When we select only the flow (not the stocks) and move it perpendicular,
      // the valve should stay on the flow path, not move off into empty space
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, stockB]]);

      // Only select the flow, move UP (perpendicular to the horizontal flow)
      const selection = new Set<UID>([2]);
      const delta = { x: 0, y: 50 }; // Move up 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stocks should NOT move
      expect((result.get(1) as StockViewElement).y).toBe(100);
      expect((result.get(3) as StockViewElement).y).toBe(100);

      // Flow valve should be clamped to stay on the horizontal flow path (y=100)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.y).toBe(100); // Should stay on the flow path

      // Flow endpoints should stay fixed
      expect(newFlow.points[0].y).toBe(100);
      expect(newFlow.points[newFlow.points.length - 1].y).toBe(100);
    });
  });

  describe('Partial chain selection', () => {
    it('should move selected elements and adjust connections to unselected elements', () => {
      // Setup: Stock A (selected) -> Flow 1 (selected) -> Stock B (not selected)
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, stockB]]);

      // Select Stock A and Flow 1, but NOT Stock B
      const selection = new Set<UID>([1, 2]);
      const delta = { x: -50, y: 0 }; // Move right 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock A should move
      expect((result.get(1) as StockViewElement).x).toBe(150);

      // Stock B should NOT move
      expect((result.get(3) as StockViewElement).x).toBe(200);

      // Flow source should move with Stock A
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points[0].x).toBe(100 + StockWidth / 2 + 50);

      // Flow sink should stay at Stock B's position
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200 - StockWidth / 2);
    });

    it('should move flow valve with the group when flow and one endpoint are selected', () => {
      // This tests the bug where valve "lags behind" because computeFlowRoute
      // preserves the valve based on old position without applying drag delta.
      //
      // Setup: Stock A (selected) -> Flow (selected, valve at 140) -> Stock B (not selected)
      // When we drag Stock A + Flow, the valve should move with the drag delta,
      // then be clamped to the new flow path.
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 300, 100, [2], []);
      // Valve is at x=140, which is closer to Stock A
      // Flow spans from 122.5 (Stock A edge) to 277.5 (Stock B edge)
      const flow = makeFlow(2, 140, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 }, // x = 122.5
        { x: 300 - StockWidth / 2, y: 100, attachedToUid: 3 }, // x = 277.5
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, stockB]]);

      // Select Stock A and Flow (not Stock B)
      const selection = new Set<UID>([1, 2]);
      const delta = { x: -50, y: 0 }; // Move right 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock A should move from 100 to 150
      expect((result.get(1) as StockViewElement).x).toBe(150);

      // Flow should be re-routed:
      // - Source moves from 122.5 to 172.5 (at new Stock A edge)
      // - Sink stays at 277.5 (at Stock B edge)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points[0].x).toBe(150 + StockWidth / 2); // 172.5
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(300 - StockWidth / 2); // 277.5

      // The valve should have moved with the drag:
      // Original valve at 140, drag delta of 50 right -> proposed position 190
      // Flow now spans 172.5 to 277.5, so 190 is within bounds
      // Valve should be at or near 190 (clamped to flow path)
      expect(newFlow.x).toBeCloseTo(190, 0);
      expect(newFlow.y).toBe(100);
    });

    it('should preserve orthogonal flow geometry when moving perpendicular to flow direction', () => {
      // Setup: Stock A -> horizontal Flow -> Stock B
      // When we move Stock A + Flow UP (perpendicular to the flow), the flow
      // should maintain orthogonal segments rather than becoming diagonal
      const stockA = makeStock(1, 100, 100, [], [2]);
      const stockB = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, stockA], [2, flow], [3, stockB]]);

      // Select Stock A and Flow, but NOT Stock B
      // Move UP by 50 (perpendicular to original horizontal flow)
      const selection = new Set<UID>([1, 2]);
      const delta = { x: 0, y: 50 }; // Move up 50

      const result = testApplyGroupMovement(elements, selection, delta);

      // Stock A should move up
      expect((result.get(1) as StockViewElement).y).toBe(50);

      // Stock B should stay in place
      expect((result.get(3) as StockViewElement).y).toBe(100);

      // Flow should be re-routed properly (L-shaped or straight to fixed stock)
      // The key assertion: the flow should NOT have a diagonal segment
      // The sink point should still connect to Stock B at its edge
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points[newFlow.points.length - 1].attachedToUid).toBe(3);
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200 - StockWidth / 2);
      expect(newFlow.points[newFlow.points.length - 1].y).toBe(100);
    });
  });

  describe('Aux movement in group', () => {
    it('should move auxes along with other selected elements', () => {
      const aux1 = makeAux(1, 100, 100);
      const aux2 = makeAux(2, 150, 150);
      const stock = makeStock(3, 200, 100);

      const elements = new Map<UID, ViewElement>([[1, aux1], [2, aux2], [3, stock]]);

      const selection = new Set<UID>([1, 2, 3]);
      const delta = { x: -30, y: -20 };

      const result = testApplyGroupMovement(elements, selection, delta);

      expect((result.get(1) as AuxViewElement).x).toBe(130);
      expect((result.get(1) as AuxViewElement).y).toBe(120);
      expect((result.get(2) as AuxViewElement).x).toBe(180);
      expect((result.get(2) as AuxViewElement).y).toBe(170);
      expect((result.get(3) as StockViewElement).x).toBe(230);
      expect((result.get(3) as StockViewElement).y).toBe(120);
    });
  });

  describe('Module movement in group', () => {
    it('should move modules when selected', () => {
      const module1 = makeModule(1, 100, 100);
      const stock = makeStock(2, 200, 100);

      const elements = new Map<UID, ViewElement>([[1, module1], [2, stock]]);

      const selection = new Set<UID>([1, 2]);
      const delta = { x: -50, y: -25 };

      const result = testApplyGroupMovement(elements, selection, delta);

      const newModule = result.get(1) as ModuleViewElement;
      expect(newModule.x).toBe(150);
      expect(newModule.y).toBe(125);
      expect((result.get(2) as StockViewElement).x).toBe(250);
    });

    it('should move a single selected module', () => {
      const module1 = makeModule(1, 100, 100);

      const elements = new Map<UID, ViewElement>([[1, module1]]);

      const selection = new Set<UID>([1]);
      const delta = { x: -30, y: -20 };

      const result = testApplyGroupMovement(elements, selection, delta);

      const newModule = result.get(1) as ModuleViewElement;
      expect(newModule.x).toBe(130);
      expect(newModule.y).toBe(120);
    });
  });

  describe('Alias movement in group', () => {
    it('should move aliases when selected', () => {
      const aux = makeAux(1, 100, 100);
      const alias = makeAlias(2, 1, 200, 150);

      const elements = new Map<UID, ViewElement>([[1, aux], [2, alias]]);

      const selection = new Set<UID>([1, 2]);
      const delta = { x: -40, y: -30 };

      const result = testApplyGroupMovement(elements, selection, delta);

      expect((result.get(1) as AuxViewElement).x).toBe(140);
      expect((result.get(1) as AuxViewElement).y).toBe(130);
      const newAlias = result.get(2) as AliasViewElement;
      expect(newAlias.x).toBe(240);
      expect(newAlias.y).toBe(180);
    });

    it('should move a single selected alias', () => {
      const aux = makeAux(1, 100, 100);
      const alias = makeAlias(2, 1, 200, 150);

      const elements = new Map<UID, ViewElement>([[1, aux], [2, alias]]);

      // Only select the alias
      const selection = new Set<UID>([2]);
      const delta = { x: -25, y: -15 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Aux should NOT move
      expect((result.get(1) as AuxViewElement).x).toBe(100);
      // Alias should move
      const newAlias = result.get(2) as AliasViewElement;
      expect(newAlias.x).toBe(225);
      expect(newAlias.y).toBe(165);
    });
  });

  describe('Group element movement', () => {
    it('should move group elements when selected', () => {
      const group = makeGroup(1, 150, 140, 100, 80);
      const stock = makeStock(2, 150, 140);

      const elements = new Map<UID, ViewElement>([[1, group], [2, stock]]);

      const selection = new Set<UID>([1, 2]);
      const delta = { x: -50, y: -30 };

      const result = testApplyGroupMovement(elements, selection, delta);

      const newGroup = result.get(1) as GroupViewElement;
      expect(newGroup.x).toBe(200);
      expect(newGroup.y).toBe(170);
      // Width and height should be preserved
      expect(newGroup.width).toBe(100);
      expect(newGroup.height).toBe(80);
    });

    it('should move a single selected group element', () => {
      const group = makeGroup(1, 150, 140, 120, 90);

      const elements = new Map<UID, ViewElement>([[1, group]]);

      const selection = new Set<UID>([1]);
      const delta = { x: -20, y: -10 };

      const result = testApplyGroupMovement(elements, selection, delta);

      const newGroup = result.get(1) as GroupViewElement;
      expect(newGroup.x).toBe(170);
      expect(newGroup.y).toBe(150);
      expect(newGroup.width).toBe(120);
      expect(newGroup.height).toBe(90);
    });
  });

  describe('Cloud with flow movement', () => {
    it('should translate cloud-to-stock flow uniformly when cloud and flow are both selected', () => {
      // Setup: Cloud -> Flow -> Stock
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock]]);

      // Select cloud, flow, and stock
      const selection = new Set<UID>([1, 2, 3]);
      const delta = { x: -50, y: -30 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Cloud should move
      expect((result.get(1) as CloudViewElement).x).toBe(150);
      expect((result.get(1) as CloudViewElement).y).toBe(130);

      // Stock should move
      expect((result.get(3) as StockViewElement).x).toBe(250);
      expect((result.get(3) as StockViewElement).y).toBe(130);

      // Flow should translate uniformly
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.x).toBe(200);
      expect(newFlow.y).toBe(130);
      expect(newFlow.points[0].x).toBe(150);
      expect(newFlow.points[0].y).toBe(130);
    });
  });

  describe('Inline chain: cloud-flow-stock-flow-cloud, all selected', () => {
    it('should translate entire inline chain uniformly', () => {
      const cloudA = makeCloud(10, 20, 50, 100);
      const stock = makeStock(1, 150, 100, [20], [21]);
      const cloudB = makeCloud(11, 21, 250, 100);
      const flowIn = makeFlow(20, 100, 100, [
        { x: 50, y: 100, attachedToUid: 10 },
        { x: 150 - StockWidth / 2, y: 100, attachedToUid: 1 },
      ]);
      const flowOut = makeFlow(21, 200, 100, [
        { x: 150 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 250, y: 100, attachedToUid: 11 },
      ]);

      const elements = new Map<UID, ViewElement>([
        [10, cloudA],
        [20, flowIn],
        [1, stock],
        [21, flowOut],
        [11, cloudB],
      ]);

      const selection = new Set<UID>([10, 20, 1, 21, 11]);
      // delta is subtracted from viewBox coords, so negative = elements move in positive direction
      const delta = { x: -60, y: -40 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // All elements should shift by (60, 40)
      expect((result.get(10) as CloudViewElement).x).toBe(110);
      expect((result.get(10) as CloudViewElement).y).toBe(140);
      expect((result.get(11) as CloudViewElement).x).toBe(310);
      expect((result.get(11) as CloudViewElement).y).toBe(140);
      expect((result.get(1) as StockViewElement).x).toBe(210);
      expect((result.get(1) as StockViewElement).y).toBe(140);

      const newFlowIn = result.get(20) as FlowViewElement;
      expect(newFlowIn.x).toBe(160);
      expect(newFlowIn.y).toBe(140);
      expect(newFlowIn.points[0].x).toBe(110);
      expect(newFlowIn.points[0].y).toBe(140);
      expect(newFlowIn.points[newFlowIn.points.length - 1].x).toBe(150 - StockWidth / 2 + 60);
      expect(newFlowIn.points[newFlowIn.points.length - 1].y).toBe(140);
      expect(newFlowIn.points.length).toBe(2);

      const newFlowOut = result.get(21) as FlowViewElement;
      expect(newFlowOut.x).toBe(260);
      expect(newFlowOut.y).toBe(140);
      expect(newFlowOut.points[0].x).toBe(150 + StockWidth / 2 + 60);
      expect(newFlowOut.points[0].y).toBe(140);
      expect(newFlowOut.points[newFlowOut.points.length - 1].x).toBe(310);
      expect(newFlowOut.points[newFlowOut.points.length - 1].y).toBe(140);
      expect(newFlowOut.points.length).toBe(2);
    });

    it('should re-route flows when only clouds and stock are selected', () => {
      const cloudA = makeCloud(10, 20, 50, 100);
      const stock = makeStock(1, 150, 100, [20], [21]);
      const cloudB = makeCloud(11, 21, 250, 100);
      const flowIn = makeFlow(20, 100, 100, [
        { x: 50, y: 100, attachedToUid: 10 },
        { x: 150 - StockWidth / 2, y: 100, attachedToUid: 1 },
      ]);
      const flowOut = makeFlow(21, 200, 100, [
        { x: 150 + StockWidth / 2, y: 100, attachedToUid: 1 },
        { x: 250, y: 100, attachedToUid: 11 },
      ]);

      const elements = new Map<UID, ViewElement>([
        [10, cloudA],
        [20, flowIn],
        [1, stock],
        [21, flowOut],
        [11, cloudB],
      ]);

      // Select only clouds + stock, not flows
      const selection = new Set<UID>([10, 1, 11]);
      const delta = { x: -60, y: 0 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Clouds and stock move
      expect((result.get(10) as CloudViewElement).x).toBe(110);
      expect((result.get(1) as StockViewElement).x).toBe(210);
      expect((result.get(11) as CloudViewElement).x).toBe(310);

      // Flows are re-routed between new positions
      const newFlowIn = result.get(20) as FlowViewElement;
      expect(newFlowIn.points[0].x).toBe(110);
      expect(newFlowIn.points[newFlowIn.points.length - 1].x).toBe(210 - StockWidth / 2);

      const newFlowOut = result.get(21) as FlowViewElement;
      expect(newFlowOut.points[0].x).toBe(210 + StockWidth / 2);
      expect(newFlowOut.points[newFlowOut.points.length - 1].x).toBe(310);
    });
  });

  describe('Cloud in selection, attached flow not in selection', () => {
    it('should adjust flow when cloud moves parallel to flow direction', () => {
      // Setup: Cloud -> Flow (not selected) -> Stock, horizontal flow
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock]]);

      // Only select the cloud, not the flow
      const selection = new Set<UID>([1]);
      const delta = { x: -50, y: 0 }; // Move cloud right 50 (parallel to flow)

      const result = testApplyGroupMovement(elements, selection, delta);

      // Cloud should move
      const newCloud = result.get(1) as CloudViewElement;
      expect(newCloud.x).toBe(150);

      // Flow should be adjusted (routed from new cloud position to fixed stock)
      const newFlow = result.get(2) as FlowViewElement;
      // Flow remains 2 points (straight horizontal line)
      expect(newFlow.points.length).toBe(2);
      // The source point should be updated to connect to new cloud position
      expect(newFlow.points[0].attachedToUid).toBe(1);
      expect(newFlow.points[0].x).toBe(150);
      expect(newFlow.points[0].y).toBe(100);
      // The sink point should still be at stock
      expect(newFlow.points[newFlow.points.length - 1].attachedToUid).toBe(3);
      expect(newFlow.points[newFlow.points.length - 1].x).toBe(200 - StockWidth / 2);
    });

    it('should create L-shaped flow when cloud moves perpendicular to flow direction', () => {
      // Setup: Cloud -> Flow (not selected) -> Stock, horizontal flow
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      const elements = new Map<UID, ViewElement>([[1, cloud], [2, flow], [3, stock]]);

      // Only select the cloud, not the flow
      const selection = new Set<UID>([1]);
      // Move cloud DOWN 30 (perpendicular to horizontal flow)
      // delta is subtracted, so y: -30 moves from y=100 to y=130
      const delta = { x: 0, y: -30 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Cloud should move down
      const newCloud = result.get(1) as CloudViewElement;
      expect(newCloud.x).toBe(100); // x unchanged
      expect(newCloud.y).toBe(130); // moved down 30

      // Flow should be re-routed as L-shaped (3 points)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.length).toBe(3);

      // First point: at cloud's new position
      const firstPt = newFlow.points[0];
      expect(firstPt.attachedToUid).toBe(1);
      expect(firstPt.x).toBe(100);
      expect(firstPt.y).toBe(130);

      // Middle point: corner creating the L-shape (at stock's x, cloud's new y)
      const middlePt = newFlow.points[1];
      expect(middlePt.attachedToUid).toBeUndefined(); // corner point, not attached
      expect(middlePt.x).toBe(200 - StockWidth / 2); // at stock's x
      expect(middlePt.y).toBe(130); // at cloud's new y

      // Last point: at stock (unchanged)
      const lastPt = newFlow.points[newFlow.points.length - 1];
      expect(lastPt.attachedToUid).toBe(3);
      expect(lastPt.x).toBe(200 - StockWidth / 2);
      expect(lastPt.y).toBe(100);

      // Verify flow maintains orthogonal segments (horizontal + vertical)
      // Segment 1: (100, 130) -> (175, 130) is horizontal
      expect(firstPt.y).toBe(middlePt.y);
      // Segment 2: (175, 130) -> (175, 100) is vertical
      expect(middlePt.x).toBe(lastPt.x);
    });
  });
});

describe('Link arc adjustment during group movement', () => {
  it('should preserve arc when both link endpoints move together', () => {
    // Setup: Aux A -> Link (with arc) -> Aux B, both selected
    const auxA = makeAux(1, 100, 100);
    const auxB = makeAux(2, 200, 100);
    const link = makeLink(3, 1, 2, 30); // Arc of 30 degrees

    const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

    // Select both auxes and the link
    const selection = new Set<UID>([1, 2, 3]);
    const delta = { x: -50, y: -25 }; // Move everything right 50, down 25

    const result = testApplyGroupMovement(elements, selection, delta);

    // Auxes should move
    expect((result.get(1) as AuxViewElement).x).toBe(150);
    expect((result.get(1) as AuxViewElement).y).toBe(125);
    expect((result.get(2) as AuxViewElement).x).toBe(250);
    expect((result.get(2) as AuxViewElement).y).toBe(125);

    // Link arc should be preserved since both endpoints moved together
    const newLink = result.get(3) as LinkViewElement;
    expect(newLink.arc).toBe(30);
  });

  it('should adjust arc angle when only one endpoint moves', () => {
    // Setup: Aux A (selected) -> Link (selected) -> Aux B (not selected)
    // Moving Aux A will change the link direction, so arc should be adjusted
    // to preserve the curve shape
    const auxA = makeAux(1, 100, 100);
    const auxB = makeAux(2, 200, 100);
    const link = makeLink(3, 1, 2, 30);

    const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

    // Select only Aux A and the link (not Aux B)
    const selection = new Set<UID>([1, 3]);
    const delta = { x: -50, y: 0 }; // Move Aux A right 50, keeping horizontal

    const result = testApplyGroupMovement(elements, selection, delta);

    // Aux A should move
    expect((result.get(1) as AuxViewElement).x).toBe(150);
    expect((result.get(1) as AuxViewElement).y).toBe(100);

    // Aux B should stay
    expect((result.get(2) as AuxViewElement).x).toBe(200);

    // Link arc should be adjusted to preserve curve shape.
    // Original line: (100, 100) -> (200, 100), angle = 0
    // New line: (150, 100) -> (200, 100), angle = 0
    // Angle difference is 0, so arc should stay the same in this case
    const newLink = result.get(3) as LinkViewElement;
    expect(newLink.arc).toBeCloseTo(30, 5);
  });

  it('should adjust arc angle for rotational movement of one endpoint', () => {
    // Setup: Aux A (selected) -> Link (selected) -> Aux B (not selected)
    // Move Aux A perpendicular to the original link direction, causing rotation
    const auxA = makeAux(1, 100, 100);
    const auxB = makeAux(2, 200, 100);
    const link = makeLink(3, 1, 2, 0); // No initial arc

    const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

    // Select only Aux A and the link
    const selection = new Set<UID>([1, 3]);
    // Move Aux A down, causing rotation
    const delta = { x: 0, y: -100 };

    const result = testApplyGroupMovement(elements, selection, delta);

    // Aux A should move down
    expect((result.get(1) as AuxViewElement).x).toBe(100);
    expect((result.get(1) as AuxViewElement).y).toBe(200);

    // Link arc should be adjusted for the rotation
    // Original line: (100, 100) -> (200, 100), angle = 0
    // New line: (100, 200) -> (200, 100), angle = atan2(100-200, 200-100) = atan2(-100, 100) = -45 degrees
    // Angle difference = 0 - (-45) = 45 degrees
    // newArc = originalArc - angleDiff = 0 - 45 = -45 degrees
    const newLink = result.get(3) as LinkViewElement;
    // Arc should have been adjusted to preserve curve shape
    expect(Math.abs(newLink.arc - -45)).toBeLessThan(1);
  });

  it('should not double-adjust arc when link is selected with one endpoint', () => {
    // This test verifies that when a link is selected along with one of its endpoints,
    // the arc is only adjusted once (not twice from two separate passes).
    // The expected arc adjustment is -45 degrees, not -90 degrees (which would be double).
    const auxA = makeAux(1, 100, 100);
    const auxB = makeAux(2, 200, 100);
    const link = makeLink(3, 1, 2, 0);

    const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

    // Select Aux A and the link (but not Aux B)
    const selection = new Set<UID>([1, 3]);
    const delta = { x: 0, y: -100 }; // Move down

    const result = testApplyGroupMovement(elements, selection, delta);

    const newLink = result.get(3) as LinkViewElement;
    // Arc should be adjusted once (-45 degrees), not twice (-90 degrees)
    // If double-adjusted, arc would be around -90 instead of -45
    expect(Math.abs(newLink.arc - -45)).toBeLessThan(1);
    expect(Math.abs(newLink.arc - -90)).toBeGreaterThan(40); // Verify it's not -90
  });

  it('should adjust arc based on drag position when only link is selected', () => {
    // When a link is the only selected element, dragging it should change its
    // curvature based on the arcPoint (drag position), not just preserve it.
    const auxA = makeAux(1, 100, 100);
    const auxB = makeAux(2, 200, 100);
    const link = makeLink(3, 1, 2, 0); // Initially straight (arc = 0)

    const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

    // Select only the link (no endpoints)
    const selection = new Set<UID>([3]);
    const delta = { x: 0, y: 0 }; // No actual movement
    // Drag to a point above the link line to create an arc
    const arcPoint = { x: 150, y: 50 };

    const result = testApplyGroupMovement(elements, selection, delta, arcPoint);

    const newLink = result.get(3) as LinkViewElement;
    // Arc should have changed from 0 to some non-zero value
    // The exact value depends on takeoffθ calculation, but it should be non-zero
    expect(newLink.arc).not.toBe(0);
    // Dragging above the line should create a positive arc
    expect(newLink.arc).toBeGreaterThan(0);
  });

  describe('arrayed elements', () => {
    it('should correctly adjust arc when arrayed source moves with fixed endpoint', () => {
      // This tests the bug where arrayed elements use visual centers (with ArrayedOffset)
      // for old positions but raw cx/cy for new positions, causing arc drift.
      //
      // Setup: Arrayed Aux A -> Link -> Non-arrayed Aux B
      // Move Aux A down (perpendicular movement causes rotation).
      // The arc adjustment should be the same as for non-arrayed elements.
      const auxA = makeAux(1, 100, 100, true); // Arrayed
      const auxB = makeAux(2, 200, 100, false); // Not arrayed
      const link = makeLink(3, 1, 2, 0); // Initially straight

      const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

      // Select Aux A and link (not Aux B)
      const selection = new Set<UID>([1, 3]);
      // Move Aux A down by 100 - this causes a 45-degree rotation
      const delta = { x: 0, y: -100 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Aux A should move down
      expect((result.get(1) as AuxViewElement).y).toBe(200);

      // Link arc should be adjusted by -45 degrees (same as non-arrayed case)
      // This verifies the bug is fixed: without the fix, the arc would drift
      // due to the ArrayedOffset (3px) mismatch between old and new position calculations.
      const newLink = result.get(3) as LinkViewElement;
      expect(Math.abs(newLink.arc - -45)).toBeLessThan(1);
    });

    it('should correctly adjust arc when arrayed target moves with fixed source', () => {
      // Setup: Non-arrayed Aux A -> Link -> Arrayed Aux B
      // Move Aux B down (perpendicular movement causes rotation).
      // The arc adjustment should match the geometric rotation based on visual centers.
      const auxA = makeAux(1, 100, 100, false); // Not arrayed
      const auxB = makeAux(2, 200, 100, true); // Arrayed (visual center at 197, 97 due to ArrayedOffset)
      const link = makeLink(3, 1, 2, 0); // Initially straight

      const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

      // Select Aux B and link (not Aux A)
      const selection = new Set<UID>([2, 3]);
      // Move Aux B down by 100 (causes rotation)
      const delta = { x: 0, y: -100 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Aux B should move down (y increases by 100)
      expect((result.get(2) as AuxViewElement).y).toBe(200);

      // Calculate expected arc adjustment based on visual centers:
      // Old visual line: (100, 100) -> (197, 97), old angle ≈ atan2(-3, 97)
      // New visual line: (100, 100) -> (197, 197), new angle = atan2(97, 97) = 45 degrees
      // The arc should be adjusted to preserve the curve shape
      const newLink = result.get(3) as LinkViewElement;
      // Without the fix, the arc would be wrong because old angle was computed from visual
      // centers but new angle was computed from raw positions.
      // With the fix, both angles are computed from visual centers, so the geometry is consistent.
      // We just verify the arc changed significantly (rotation occurred)
      expect(Math.abs(newLink.arc)).toBeGreaterThan(30);
    });

    it('should correctly adjust arc when both endpoints are arrayed', () => {
      // Setup: Arrayed Aux A -> Link -> Arrayed Aux B
      // Move Aux A diagonally while Aux B stays fixed.
      const auxA = makeAux(1, 100, 100, true); // Arrayed
      const auxB = makeAux(2, 200, 100, true); // Arrayed
      const link = makeLink(3, 1, 2, 0); // Initially straight

      const elements = new Map<UID, ViewElement>([[1, auxA], [2, auxB], [3, link]]);

      // Select Aux A and link (not Aux B)
      const selection = new Set<UID>([1, 3]);
      // Move Aux A down by 100 (causes 45-degree rotation)
      const delta = { x: 0, y: -100 };

      const result = testApplyGroupMovement(elements, selection, delta);

      // Link arc should be adjusted by -45 degrees (same as non-arrayed case)
      const newLink = result.get(3) as LinkViewElement;
      expect(Math.abs(newLink.arc - -45)).toBeLessThan(1);
    });
  });
});
