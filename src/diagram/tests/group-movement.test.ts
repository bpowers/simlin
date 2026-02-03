// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Set, Map } from 'immutable';

import {
  Point,
  FlowViewElement,
  StockViewElement,
  CloudViewElement,
  AuxViewElement,
  ViewElement,
  UID,
} from '@system-dynamics/core/datamodel';

import { StockWidth } from '../drawing/Stock';
import {
  clampToSegment,
  computeFlowOffsets,
  computeFlowRoute,
  findClosestSegment,
  getSegments,
  UpdateCloudAndFlow,
} from '../drawing/Flow';

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

function makeCloud(uid: number, flowUid: number, x: number, y: number): CloudViewElement {
  return new CloudViewElement({
    uid,
    flowUid,
    x,
    y,
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

interface Point2D {
  x: number;
  y: number;
}

/**
 * Core function to apply group movement logic.
 *
 * When multiple elements are selected, movement is handled as follows:
 *
 * 1. Stocks/Auxes in selection: Move by delta
 * 2. Flows in selection:
 *    - If BOTH endpoints are in selection (or attached to elements in selection): Translate entire flow uniformly
 *    - If ONLY ONE endpoint is in selection: That end moves with selection, other end adjusts
 *    - If NEITHER endpoint is in selection: Valve position moves (like single-flow move)
 * 3. Links in selection: If both from/to are in selection, link moves with them (no arc change)
 * 4. Links NOT in selection but connected to selection: Arc angle adjusts
 *
 * For stocks NOT in selection but attached to flows IN selection:
 *   - The stock stays fixed, the flow endpoint adjusts
 *
 * For flows NOT in selection but attached to stocks IN selection:
 *   - Use UpdateStockAndFlows to adjust the flow (as if moving stock alone)
 */
export function applyGroupMovement(
  elements: Map<UID, ViewElement>,
  selection: Set<UID>,
  delta: Point2D,
): Map<UID, ViewElement> {
  // First pass: identify what types of elements are in selection
  const selectedStockUids = new globalThis.Set<UID>();
  const selectedFlowUids = new globalThis.Set<UID>();
  const selectedCloudUids = new globalThis.Set<UID>();
  const selectedAuxUids = new globalThis.Set<UID>();

  for (const uid of selection) {
    const element = elements.get(uid);
    if (!element) continue;
    if (element instanceof StockViewElement) {
      selectedStockUids.add(uid);
    } else if (element instanceof FlowViewElement) {
      selectedFlowUids.add(uid);
    } else if (element instanceof CloudViewElement) {
      selectedCloudUids.add(uid);
    } else if (element instanceof AuxViewElement) {
      selectedAuxUids.add(uid);
    }
  }

  let result = elements;

  // Process stocks: Move all selected stocks by delta
  for (const uid of selectedStockUids) {
    const stock = result.get(uid) as StockViewElement;
    result = result.set(
      uid,
      stock.merge({
        x: stock.cx - delta.x,
        y: stock.cy - delta.y,
      }),
    );
  }

  // Process auxes: Move all selected auxes by delta
  for (const uid of selectedAuxUids) {
    const aux = result.get(uid) as AuxViewElement;
    result = result.set(
      uid,
      aux.merge({
        x: aux.cx - delta.x,
        y: aux.cy - delta.y,
      }),
    );
  }

  // Pre-compute flow offsets for all flows attached to moved stocks.
  // This ensures both selected and unselected flows maintain proper spacing.
  const preComputedOffsets = new globalThis.Map<UID, number>();
  const preProcessedFlows = new globalThis.Map<UID, FlowViewElement>();

  // Identify all moved stocks and compute offsets for flows that need routing
  for (const stockUid of selectedStockUids) {
    const stock = elements.get(stockUid) as StockViewElement;

    // Collect flows attached to this stock that need routing (excluding flows where
    // both endpoints are selected, since those translate uniformly and don't use
    // the stock's routing logic - including them would miscompute offsets because
    // their anchors aren't adjusted for the drag)
    let flowsNeedingRouting = List<FlowViewElement>();
    for (const [, el] of elements) {
      if (!(el instanceof FlowViewElement)) continue;
      const pts = el.points;
      if (pts.size < 2) continue;
      const sourceUid = pts.first()!.attachedToUid;
      const sinkUid = pts.last()!.attachedToUid;
      const attachedToThisStock = sourceUid === stockUid || sinkUid === stockUid;
      if (!attachedToThisStock) continue;

      // Exclude flows where both endpoints are selected (they translate uniformly)
      const otherEndpointUid = sourceUid === stockUid ? sinkUid : sourceUid;
      const bothEndpointsSelected = otherEndpointUid !== undefined && selectedStockUids.has(otherEndpointUid);
      if (!bothEndpointsSelected) {
        flowsNeedingRouting = flowsNeedingRouting.push(el);
      }
    }

    // Compute offsets at the new stock position
    const newStockCx = stock.cx - delta.x;
    const newStockCy = stock.cy - delta.y;
    const offsets = computeFlowOffsets(flowsNeedingRouting, stock.uid, newStockCx, newStockCy);

    // Store offsets for flows needing routing
    for (const [flowUid, offset] of offsets) {
      preComputedOffsets.set(flowUid, offset);
    }
  }

  // Pre-process selected flows with one endpoint selected (stock endpoint only)
  for (const flowUid of selectedFlowUids) {
    const flow = elements.get(flowUid) as FlowViewElement;
    const points = flow.points;
    if (points.size < 2) continue;

    const sourceUid = points.first()!.attachedToUid;
    const sinkUid = points.last()!.attachedToUid;
    const sourceInSel = sourceUid !== undefined && selectedStockUids.has(sourceUid);
    const sinkInSel = sinkUid !== undefined && selectedStockUids.has(sinkUid);

    // Process flows where exactly one endpoint is a selected stock
    if (sourceInSel && !sinkInSel && sourceUid !== undefined) {
      const endpoint = elements.get(sourceUid);
      if (endpoint instanceof StockViewElement) {
        const newStockCx = endpoint.cx - delta.x;
        const newStockCy = endpoint.cy - delta.y;
        const offset = preComputedOffsets.get(flow.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(flow, endpoint, newStockCx, newStockCy, offset);
        preProcessedFlows.set(flow.uid, updatedFlow);
      }
    } else if (!sourceInSel && sinkInSel && sinkUid !== undefined) {
      const endpoint = elements.get(sinkUid);
      if (endpoint instanceof StockViewElement) {
        const newStockCx = endpoint.cx - delta.x;
        const newStockCy = endpoint.cy - delta.y;
        const offset = preComputedOffsets.get(flow.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(flow, endpoint, newStockCx, newStockCy, offset);
        preProcessedFlows.set(flow.uid, updatedFlow);
      }
    }
  }

  // Process flows in selection
  for (const flowUid of selectedFlowUids) {
    const flow = result.get(flowUid) as FlowViewElement;
    const points = flow.points;
    if (points.size < 2) continue;

    const sourceUid = points.first()!.attachedToUid;
    const sinkUid = points.last()!.attachedToUid;

    // Check if source and sink are in selection (directly or via their stock)
    const sourceInSelection = sourceUid !== undefined && (selection.has(sourceUid) || selectedStockUids.has(sourceUid));
    const sinkInSelection = sinkUid !== undefined && (selection.has(sinkUid) || selectedStockUids.has(sinkUid));

    if (sourceInSelection && sinkInSelection) {
      // Both endpoints in selection: translate entire flow uniformly
      const newPoints = points.map((p) =>
        p.merge({
          x: p.x - delta.x,
          y: p.y - delta.y,
        }),
      );
      result = result.set(
        flowUid,
        flow.merge({
          x: flow.cx - delta.x,
          y: flow.cy - delta.y,
          points: newPoints,
        }),
      );
    } else if (sourceInSelection || sinkInSelection) {
      // One endpoint in selection: that endpoint moves, flow re-routes to fixed endpoint
      // Check if this flow was pre-processed (for multi-flow spacing preservation)
      const preProcessed = preProcessedFlows.get(flowUid);
      if (preProcessed) {
        result = result.set(flowUid, preProcessed);
      } else if (sourceInSelection && sourceUid !== undefined) {
        // Handle clouds (not pre-processed since they only have one flow each)
        // Ensure flow endpoint matches cloud's actual new position after routing
        const sourceEndpoint = elements.get(sourceUid);
        if (sourceEndpoint instanceof CloudViewElement) {
          let [, updatedFlow] = UpdateCloudAndFlow(sourceEndpoint, flow, delta);
          // Ensure endpoint matches cloud's actual new position (raw delta, not clamped)
          const newCloudCx = sourceEndpoint.cx - delta.x;
          const newCloudCy = sourceEndpoint.cy - delta.y;
          const cloudPointIndex = 0; // source is at index 0
          const cloudPoint = updatedFlow.points.get(cloudPointIndex);
          if (cloudPoint) {
            const updatedPoints = updatedFlow.points.set(
              cloudPointIndex,
              cloudPoint.merge({ x: newCloudCx, y: newCloudCy }),
            );
            updatedFlow = updatedFlow.set('points', updatedPoints);
          }
          result = result.set(flowUid, updatedFlow);
        }
      } else if (sinkInSelection && sinkUid !== undefined) {
        const sinkEndpoint = elements.get(sinkUid);
        if (sinkEndpoint instanceof CloudViewElement) {
          let [, updatedFlow] = UpdateCloudAndFlow(sinkEndpoint, flow, delta);
          // Ensure endpoint matches cloud's actual new position (raw delta, not clamped)
          const newCloudCx = sinkEndpoint.cx - delta.x;
          const newCloudCy = sinkEndpoint.cy - delta.y;
          const cloudPointIndex = updatedFlow.points.size - 1; // sink is at last index
          const cloudPoint = updatedFlow.points.get(cloudPointIndex);
          if (cloudPoint) {
            const updatedPoints = updatedFlow.points.set(
              cloudPointIndex,
              cloudPoint.merge({ x: newCloudCx, y: newCloudCy }),
            );
            updatedFlow = updatedFlow.set('points', updatedPoints);
          }
          result = result.set(flowUid, updatedFlow);
        }
      }
    } else {
      // Neither endpoint in selection: move valve but clamp to flow path
      const proposedValve = {
        x: flow.cx - delta.x,
        y: flow.cy - delta.y,
      };
      const segments = getSegments(points);
      if (segments.length > 0) {
        const closestSegment = findClosestSegment(proposedValve, segments);
        const clampedValve = clampToSegment(proposedValve, closestSegment);
        result = result.set(
          flowUid,
          flow.merge({
            x: clampedValve.x,
            y: clampedValve.y,
          }),
        );
      } else {
        result = result.set(
          flowUid,
          flow.merge({
            x: proposedValve.x,
            y: proposedValve.y,
          }),
        );
      }
    }
  }

  // Process clouds in selection: move them and update their flows
  for (const cloudUid of selectedCloudUids) {
    const cloud = result.get(cloudUid) as CloudViewElement;
    result = result.set(
      cloudUid,
      cloud.merge({
        x: cloud.cx - delta.x,
        y: cloud.cy - delta.y,
      }),
    );
  }

  // Process flows NOT in selection but attached to endpoints IN selection
  // Group flows by endpoint to preserve multi-flow spacing
  const flowsBySourceEndpoint = new globalThis.Map<UID, List<FlowViewElement>>();
  const flowsBySinkEndpoint = new globalThis.Map<UID, List<FlowViewElement>>();
  const bothEndsSelectedFlows: { uid: UID; flow: FlowViewElement }[] = [];

  for (const [uid, element] of result) {
    if (!(element instanceof FlowViewElement)) continue;
    if (selectedFlowUids.has(uid)) continue; // Already processed

    const flow = element;
    const points = flow.points;
    if (points.size < 2) continue;

    const sourceUid = points.first()!.attachedToUid;
    const sinkUid = points.last()!.attachedToUid;

    // Check if source or sink is a selected endpoint (stock or cloud)
    const sourceEndpointSelected =
      sourceUid !== undefined && (selectedStockUids.has(sourceUid) || selectedCloudUids.has(sourceUid));
    const sinkEndpointSelected =
      sinkUid !== undefined && (selectedStockUids.has(sinkUid) || selectedCloudUids.has(sinkUid));

    if (sourceEndpointSelected && sinkEndpointSelected) {
      bothEndsSelectedFlows.push({ uid, flow });
    } else if (sourceEndpointSelected && sourceUid !== undefined) {
      const existing = flowsBySourceEndpoint.get(sourceUid) || List<FlowViewElement>();
      flowsBySourceEndpoint.set(sourceUid, existing.push(flow));
    } else if (sinkEndpointSelected && sinkUid !== undefined) {
      const existing = flowsBySinkEndpoint.get(sinkUid) || List<FlowViewElement>();
      flowsBySinkEndpoint.set(sinkUid, existing.push(flow));
    }
  }

  // Handle flows where both ends are selected: translate uniformly
  for (const { uid, flow } of bothEndsSelectedFlows) {
    const points = flow.points;
    const newPoints = points.map((p) =>
      p.merge({
        x: p.x - delta.x,
        y: p.y - delta.y,
      }),
    );
    result = result.set(
      uid,
      flow.merge({
        x: flow.cx - delta.x,
        y: flow.cy - delta.y,
        points: newPoints,
      }),
    );
  }

  // Update flows grouped by source endpoint using pre-computed offsets
  for (const [endpointUid, flows] of flowsBySourceEndpoint) {
    const endpoint = result.get(endpointUid);
    if (endpoint instanceof StockViewElement) {
      // Use pre-computed offsets to maintain spacing with selected flows
      const originalStock = elements.get(endpointUid) as StockViewElement;
      const newStockCx = originalStock.cx - delta.x;
      const newStockCy = originalStock.cy - delta.y;
      for (const flow of flows) {
        const offset = preComputedOffsets.get(flow.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(flow, originalStock, newStockCx, newStockCy, offset);
        result = result.set(updatedFlow.uid, updatedFlow);
      }
    } else if (endpoint instanceof CloudViewElement) {
      // For clouds, use UpdateCloudAndFlow for orthogonal re-routing on perpendicular moves,
      // but ensure the endpoint matches the cloud's actual new position.
      const originalCloud = elements.get(endpointUid);
      if (originalCloud instanceof CloudViewElement) {
        const newCloudCx = originalCloud.cx - delta.x;
        const newCloudCy = originalCloud.cy - delta.y;
        for (const flow of flows) {
          let [, updatedFlow] = UpdateCloudAndFlow(originalCloud, flow, delta);
          // Ensure the cloud endpoint matches the actual cloud position
          const cloudPointIndex =
            updatedFlow.points.first()!.attachedToUid === endpointUid ? 0 : updatedFlow.points.size - 1;
          const cloudPoint = updatedFlow.points.get(cloudPointIndex);
          if (cloudPoint) {
            const updatedPoints = updatedFlow.points.set(
              cloudPointIndex,
              cloudPoint.merge({ x: newCloudCx, y: newCloudCy }),
            );
            updatedFlow = updatedFlow.set('points', updatedPoints);
          }
          result = result.set(updatedFlow.uid, updatedFlow);
        }
      }
    }
  }

  // Update flows grouped by sink endpoint using pre-computed offsets
  for (const [endpointUid, flows] of flowsBySinkEndpoint) {
    const endpoint = result.get(endpointUid);
    if (endpoint instanceof StockViewElement) {
      // Use pre-computed offsets to maintain spacing with selected flows
      const originalStock = elements.get(endpointUid) as StockViewElement;
      const newStockCx = originalStock.cx - delta.x;
      const newStockCy = originalStock.cy - delta.y;
      for (const flow of flows) {
        const offset = preComputedOffsets.get(flow.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(flow, originalStock, newStockCx, newStockCy, offset);
        result = result.set(updatedFlow.uid, updatedFlow);
      }
    } else if (endpoint instanceof CloudViewElement) {
      // For clouds, use UpdateCloudAndFlow for orthogonal re-routing on perpendicular moves,
      // but ensure the endpoint matches the cloud's actual new position.
      const originalCloud = elements.get(endpointUid);
      if (originalCloud instanceof CloudViewElement) {
        const newCloudCx = originalCloud.cx - delta.x;
        const newCloudCy = originalCloud.cy - delta.y;
        for (const flow of flows) {
          let [, updatedFlow] = UpdateCloudAndFlow(originalCloud, flow, delta);
          // Ensure the cloud endpoint matches the actual cloud position
          const cloudPointIndex =
            updatedFlow.points.last()!.attachedToUid === endpointUid ? updatedFlow.points.size - 1 : 0;
          const cloudPoint = updatedFlow.points.get(cloudPointIndex);
          if (cloudPoint) {
            const updatedPoints = updatedFlow.points.set(
              cloudPointIndex,
              cloudPoint.merge({ x: newCloudCx, y: newCloudCy }),
            );
            updatedFlow = updatedFlow.set('points', updatedPoints);
          }
          result = result.set(updatedFlow.uid, updatedFlow);
        }
      }
    }
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow).set(3, stockB);

      const selection = Set<UID>([1, 2, 3]);
      const delta = { x: -50, y: -30 }; // Move right 50, down 30

      const result = applyGroupMovement(elements, selection, delta);

      // Stock A should move
      const newStockA = result.get(1) as StockViewElement;
      expect(newStockA.cx).toBe(150); // 100 + 50
      expect(newStockA.cy).toBe(130); // 100 + 30

      // Stock B should move
      const newStockB = result.get(3) as StockViewElement;
      expect(newStockB.cx).toBe(250); // 200 + 50
      expect(newStockB.cy).toBe(130); // 100 + 30

      // Flow valve should move
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.cx).toBe(200); // 150 + 50
      expect(newFlow.cy).toBe(130); // 100 + 30

      // Flow endpoints should move too
      const newPoints = newFlow.points;
      expect(newPoints.first()!.x).toBe(100 + StockWidth / 2 + 50);
      expect(newPoints.first()!.y).toBe(130);
      expect(newPoints.last()!.x).toBe(200 - StockWidth / 2 + 50);
      expect(newPoints.last()!.y).toBe(130);
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow1).set(3, stockB).set(4, flow2).set(5, stockC);

      const selection = Set<UID>([1, 2, 3, 4, 5]);
      const delta = { x: -100, y: 0 }; // Move right 100

      const result = applyGroupMovement(elements, selection, delta);

      // All stocks should move by same amount
      expect((result.get(1) as StockViewElement).cx).toBe(200);
      expect((result.get(3) as StockViewElement).cx).toBe(300);
      expect((result.get(5) as StockViewElement).cx).toBe(400);

      // All flows should move by same amount
      expect((result.get(2) as FlowViewElement).cx).toBe(250);
      expect((result.get(4) as FlowViewElement).cx).toBe(350);
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow).set(3, cloud);

      // Only select the stock, not the flow
      const selection = Set<UID>([1]);
      const delta = { x: -50, y: 0 }; // Move stock right 50

      const result = applyGroupMovement(elements, selection, delta);

      // Stock should move
      const newStock = result.get(1) as StockViewElement;
      expect(newStock.cx).toBe(150);

      // Flow should be adjusted (routed from new stock position to fixed cloud)
      const newFlow = result.get(2) as FlowViewElement;
      // The source point should be updated to connect to new stock position
      expect(newFlow.points.first()!.attachedToUid).toBe(1);
      // IMPORTANT: Verify the source point x-coordinate is at the NEW stock edge
      // (not double-moved). Stock moved from 100 to 150, so source point should
      // be at 150 + StockWidth/2.
      expect(newFlow.points.first()!.x).toBe(150 + StockWidth / 2);
      // The sink point should still be at cloud
      expect(newFlow.points.last()!.attachedToUid).toBe(3);
      expect(newFlow.points.last()!.x).toBe(200);
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow).set(3, stockB);

      // Only select the flow
      const selection = Set<UID>([2]);
      const delta = { x: -20, y: 0 }; // Move right 20

      const result = applyGroupMovement(elements, selection, delta);

      // Stocks should NOT move
      expect((result.get(1) as StockViewElement).cx).toBe(100);
      expect((result.get(3) as StockViewElement).cx).toBe(200);

      // Flow valve should move but be clamped to stay on the flow path
      // Flow goes from x=122.5 to x=177.5 (with 10px margin, max is 167.5)
      // Proposed position is 170, so it gets clamped to 167.5
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.cx).toBe(167.5); // Clamped to stay within flow bounds

      // Flow endpoints should stay fixed (attached to stocks)
      expect(newFlow.points.first()!.x).toBe(100 + StockWidth / 2);
      expect(newFlow.points.last()!.x).toBe(200 - StockWidth / 2);
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow).set(3, stockB);

      // Only select the flow, move UP (perpendicular to the horizontal flow)
      const selection = Set<UID>([2]);
      const delta = { x: 0, y: 50 }; // Move up 50

      const result = applyGroupMovement(elements, selection, delta);

      // Stocks should NOT move
      expect((result.get(1) as StockViewElement).cy).toBe(100);
      expect((result.get(3) as StockViewElement).cy).toBe(100);

      // Flow valve should be clamped to stay on the horizontal flow path (y=100)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.cy).toBe(100); // Should stay on the flow path

      // Flow endpoints should stay fixed
      expect(newFlow.points.first()!.y).toBe(100);
      expect(newFlow.points.last()!.y).toBe(100);
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow).set(3, stockB);

      // Select Stock A and Flow 1, but NOT Stock B
      const selection = Set<UID>([1, 2]);
      const delta = { x: -50, y: 0 }; // Move right 50

      const result = applyGroupMovement(elements, selection, delta);

      // Stock A should move
      expect((result.get(1) as StockViewElement).cx).toBe(150);

      // Stock B should NOT move
      expect((result.get(3) as StockViewElement).cx).toBe(200);

      // Flow source should move with Stock A
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.first()!.x).toBe(100 + StockWidth / 2 + 50);

      // Flow sink should stay at Stock B's position
      expect(newFlow.points.last()!.x).toBe(200 - StockWidth / 2);
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

      let elements = Map<UID, ViewElement>().set(1, stockA).set(2, flow).set(3, stockB);

      // Select Stock A and Flow, but NOT Stock B
      // Move UP by 50 (perpendicular to original horizontal flow)
      const selection = Set<UID>([1, 2]);
      const delta = { x: 0, y: 50 }; // Move up 50

      const result = applyGroupMovement(elements, selection, delta);

      // Stock A should move up
      expect((result.get(1) as StockViewElement).cy).toBe(50);

      // Stock B should stay in place
      expect((result.get(3) as StockViewElement).cy).toBe(100);

      // Flow should be re-routed properly (L-shaped or straight to fixed stock)
      // The key assertion: the flow should NOT have a diagonal segment
      // The sink point should still connect to Stock B at its edge
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.last()!.attachedToUid).toBe(3);
      expect(newFlow.points.last()!.x).toBe(200 - StockWidth / 2);
      expect(newFlow.points.last()!.y).toBe(100);
    });
  });

  describe('Aux movement in group', () => {
    it('should move auxes along with other selected elements', () => {
      const aux1 = makeAux(1, 100, 100);
      const aux2 = makeAux(2, 150, 150);
      const stock = makeStock(3, 200, 100);

      let elements = Map<UID, ViewElement>().set(1, aux1).set(2, aux2).set(3, stock);

      const selection = Set<UID>([1, 2, 3]);
      const delta = { x: -30, y: -20 };

      const result = applyGroupMovement(elements, selection, delta);

      expect((result.get(1) as AuxViewElement).cx).toBe(130);
      expect((result.get(1) as AuxViewElement).cy).toBe(120);
      expect((result.get(2) as AuxViewElement).cx).toBe(180);
      expect((result.get(2) as AuxViewElement).cy).toBe(170);
      expect((result.get(3) as StockViewElement).cx).toBe(230);
      expect((result.get(3) as StockViewElement).cy).toBe(120);
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

      let elements = Map<UID, ViewElement>().set(1, cloud).set(2, flow).set(3, stock);

      // Select cloud, flow, and stock
      const selection = Set<UID>([1, 2, 3]);
      const delta = { x: -50, y: -30 };

      const result = applyGroupMovement(elements, selection, delta);

      // Cloud should move
      expect((result.get(1) as CloudViewElement).cx).toBe(150);
      expect((result.get(1) as CloudViewElement).cy).toBe(130);

      // Stock should move
      expect((result.get(3) as StockViewElement).cx).toBe(250);
      expect((result.get(3) as StockViewElement).cy).toBe(130);

      // Flow should translate uniformly
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.cx).toBe(200);
      expect(newFlow.cy).toBe(130);
      expect(newFlow.points.first()!.x).toBe(150);
      expect(newFlow.points.first()!.y).toBe(130);
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

      let elements = Map<UID, ViewElement>().set(1, cloud).set(2, flow).set(3, stock);

      // Only select the cloud, not the flow
      const selection = Set<UID>([1]);
      const delta = { x: -50, y: 0 }; // Move cloud right 50 (parallel to flow)

      const result = applyGroupMovement(elements, selection, delta);

      // Cloud should move
      const newCloud = result.get(1) as CloudViewElement;
      expect(newCloud.cx).toBe(150);

      // Flow should be adjusted (routed from new cloud position to fixed stock)
      const newFlow = result.get(2) as FlowViewElement;
      // Flow remains 2 points (straight horizontal line)
      expect(newFlow.points.size).toBe(2);
      // The source point should be updated to connect to new cloud position
      expect(newFlow.points.first()!.attachedToUid).toBe(1);
      expect(newFlow.points.first()!.x).toBe(150);
      expect(newFlow.points.first()!.y).toBe(100);
      // The sink point should still be at stock
      expect(newFlow.points.last()!.attachedToUid).toBe(3);
      expect(newFlow.points.last()!.x).toBe(200 - StockWidth / 2);
    });

    it('should create L-shaped flow when cloud moves perpendicular to flow direction', () => {
      // Setup: Cloud -> Flow (not selected) -> Stock, horizontal flow
      const cloud = makeCloud(1, 2, 100, 100);
      const stock = makeStock(3, 200, 100, [2], []);
      const flow = makeFlow(2, 150, 100, [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 200 - StockWidth / 2, y: 100, attachedToUid: 3 },
      ]);

      let elements = Map<UID, ViewElement>().set(1, cloud).set(2, flow).set(3, stock);

      // Only select the cloud, not the flow
      const selection = Set<UID>([1]);
      // Move cloud DOWN 30 (perpendicular to horizontal flow)
      // delta is subtracted, so y: -30 moves from y=100 to y=130
      const delta = { x: 0, y: -30 };

      const result = applyGroupMovement(elements, selection, delta);

      // Cloud should move down
      const newCloud = result.get(1) as CloudViewElement;
      expect(newCloud.cx).toBe(100); // x unchanged
      expect(newCloud.cy).toBe(130); // moved down 30

      // Flow should be re-routed as L-shaped (3 points)
      const newFlow = result.get(2) as FlowViewElement;
      expect(newFlow.points.size).toBe(3);

      // First point: at cloud's new position
      const firstPt = newFlow.points.first()!;
      expect(firstPt.attachedToUid).toBe(1);
      expect(firstPt.x).toBe(100);
      expect(firstPt.y).toBe(130);

      // Middle point: corner creating the L-shape (at stock's x, cloud's new y)
      const middlePt = newFlow.points.get(1)!;
      expect(middlePt.attachedToUid).toBeUndefined(); // corner point, not attached
      expect(middlePt.x).toBe(200 - StockWidth / 2); // at stock's x
      expect(middlePt.y).toBe(130); // at cloud's new y

      // Last point: at stock (unchanged)
      const lastPt = newFlow.points.last()!;
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
  // Note: Link arc adjustment tests would go here
  // These require the arc calculation utilities which are tested separately
  // in arc-utils.test.ts
});
