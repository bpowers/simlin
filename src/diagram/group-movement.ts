// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Shared logic for group selection movement.
 *
 * This module provides functions for computing how diagram elements should
 * move when a group selection is dragged. The same logic is used by both
 * Editor.tsx (for persisting changes) and Canvas.tsx (for live preview).
 */

import { List, Set } from 'immutable';
import { first, last } from '@system-dynamics/core/collections';
import { ViewElement, FlowViewElement, StockViewElement, CloudViewElement, UID } from '@system-dynamics/core/datamodel';
import {
  clampToSegment,
  computeFlowOffsets,
  computeFlowRoute,
  findClosestSegment,
  getSegments,
  UpdateCloudAndFlow,
} from './drawing/Flow';

export interface Point2D {
  x: number;
  y: number;
}

export interface GroupMovementResult {
  /** Map from element UID to updated element (for elements that changed) */
  updatedElements: Map<UID, ViewElement>;
  /** Additional elements to update (clouds updated via flow routing, etc.) */
  sideEffects: List<ViewElement>;
}

/**
 * Pre-compute flow offsets for all flows attached to moved stocks.
 * This ensures both selected and unselected flows maintain proper spacing.
 *
 * @param elements All view elements
 * @param selectedStockUids UIDs of stocks in the selection
 * @param delta The movement delta
 * @param isInSelection Function to check if a UID is in the selection
 * @returns Map from flow UID to its offset fraction
 */
export function computePreRoutedOffsets(
  elements: Iterable<ViewElement>,
  selectedStockUids: globalThis.Set<UID>,
  delta: Point2D,
  isInSelection: (uid: UID | undefined) => boolean,
): globalThis.Map<UID, number> {
  const preComputedOffsets = new globalThis.Map<UID, number>();

  for (const element of elements) {
    if (!(element instanceof StockViewElement)) continue;
    if (!selectedStockUids.has(element.uid)) continue;

    // Collect flows attached to this stock that need routing (excluding flows where
    // both endpoints are selected, since those translate uniformly and don't use
    // the stock's routing logic - including them would miscompute offsets because
    // their anchors aren't adjusted for the drag)
    let flowsNeedingRouting = List<FlowViewElement>();
    for (const el of elements) {
      if (!(el instanceof FlowViewElement)) continue;
      const pts = el.points;
      if (pts.size < 2) continue;
      const sourceUid = first(pts).attachedToUid;
      const sinkUid = last(pts).attachedToUid;
      const attachedToThisStock = sourceUid === element.uid || sinkUid === element.uid;
      if (!attachedToThisStock) continue;

      // Exclude flows where both endpoints are selected (they translate uniformly)
      const otherEndpointUid = sourceUid === element.uid ? sinkUid : sourceUid;
      const bothEndpointsSelected = isInSelection(otherEndpointUid);
      if (!bothEndpointsSelected) {
        flowsNeedingRouting = flowsNeedingRouting.push(el);
      }
    }

    // Compute offsets at the new stock position
    const newStockCx = element.cx - delta.x;
    const newStockCy = element.cy - delta.y;
    const offsets = computeFlowOffsets(flowsNeedingRouting, element.uid, newStockCx, newStockCy);

    // Store offsets for flows needing routing
    for (const [flowUid, offset] of offsets) {
      preComputedOffsets.set(flowUid, offset);
    }
  }

  return preComputedOffsets;
}

/**
 * Pre-process selected flows with one endpoint selected (stock endpoint only).
 * Returns a map from flow UID to the pre-routed flow.
 *
 * @param elements All view elements
 * @param selectedFlowUids UIDs of flows in the selection
 * @param preComputedOffsets Pre-computed offsets from computePreRoutedOffsets
 * @param delta The movement delta
 * @param isInSelection Function to check if a UID is in the selection
 * @param getElementByUid Function to get an element by UID
 * @returns Map from flow UID to pre-routed flow
 */
export function preProcessSelectedFlows(
  elements: Iterable<ViewElement>,
  selectedFlowUids: Set<UID>,
  preComputedOffsets: globalThis.Map<UID, number>,
  delta: Point2D,
  isInSelection: (uid: UID | undefined) => boolean,
  getElementByUid: (uid: UID) => ViewElement | undefined,
): [globalThis.Map<UID, FlowViewElement>, List<ViewElement>] {
  const preProcessedFlows = new globalThis.Map<UID, FlowViewElement>();
  let sideEffects = List<ViewElement>();

  for (const element of elements) {
    if (!(element instanceof FlowViewElement)) continue;
    if (!selectedFlowUids.has(element.uid)) continue;

    const pts = element.points;
    if (pts.size < 2) continue;

    const sourceUid = first(pts).attachedToUid;
    const sinkUid = last(pts).attachedToUid;
    const sourceInSel = isInSelection(sourceUid);
    const sinkInSel = isInSelection(sinkUid);

    // Process flows where exactly one endpoint is in selection (and is a stock)
    if (sourceInSel && !sinkInSel && sourceUid !== undefined) {
      const endpoint = getElementByUid(sourceUid);
      if (endpoint instanceof StockViewElement) {
        const newStockCx = endpoint.cx - delta.x;
        const newStockCy = endpoint.cy - delta.y;
        const offset = preComputedOffsets.get(element.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(element, endpoint, newStockCx, newStockCy, offset);
        preProcessedFlows.set(element.uid, updatedFlow);
      }
    } else if (!sourceInSel && sinkInSel && sinkUid !== undefined) {
      const endpoint = getElementByUid(sinkUid);
      if (endpoint instanceof StockViewElement) {
        const newStockCx = endpoint.cx - delta.x;
        const newStockCy = endpoint.cy - delta.y;
        const offset = preComputedOffsets.get(element.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(element, endpoint, newStockCx, newStockCy, offset);
        preProcessedFlows.set(element.uid, updatedFlow);
      }
    }
  }

  return [preProcessedFlows, sideEffects];
}

/**
 * Process a selected flow element during group movement.
 *
 * @param flow The flow element to process
 * @param delta The movement delta
 * @param isInSelection Function to check if a UID is in the selection
 * @param preProcessedFlows Pre-processed flows from preProcessSelectedFlows
 * @param getElementByUid Function to get an element by UID
 * @returns Tuple of [updatedFlow, sideEffectElements]
 */
export function processSelectedFlow(
  flow: FlowViewElement,
  delta: Point2D,
  isInSelection: (uid: UID | undefined) => boolean,
  preProcessedFlows: globalThis.Map<UID, FlowViewElement>,
  getElementByUid: (uid: UID) => ViewElement | undefined,
): [FlowViewElement, List<ViewElement>] {
  let sideEffects = List<ViewElement>();
  const pts = flow.points;

  if (pts.size < 2) {
    return [flow, sideEffects];
  }

  const sourceUid = first(pts).attachedToUid;
  const sinkUid = last(pts).attachedToUid;
  const sourceInSelection = isInSelection(sourceUid);
  const sinkInSelection = isInSelection(sinkUid);

  if (sourceInSelection && sinkInSelection) {
    // Both endpoints are selected: translate entire flow uniformly
    const newPoints = pts.map((p) =>
      p.merge({
        x: p.x - delta.x,
        y: p.y - delta.y,
      }),
    );
    return [
      flow.merge({
        x: flow.cx - delta.x,
        y: flow.cy - delta.y,
        points: newPoints,
      }),
      sideEffects,
    ];
  } else if (sourceInSelection || sinkInSelection) {
    // One endpoint is selected: that endpoint moves, flow re-routes to fixed endpoint
    // Check if this flow was pre-processed (for multi-flow spacing preservation)
    const preProcessed = preProcessedFlows.get(flow.uid);
    if (preProcessed) {
      return [preProcessed, sideEffects];
    }

    // Handle clouds: when a cloud is selected in a group, it should move by the full delta
    // without axis clamping (unlike single-element cloud movement which clamps small perpendicular moves).
    // We use UpdateCloudAndFlow for flow routing but override the cloud position to honor full delta.
    if (sourceInSelection && sourceUid !== undefined) {
      const sourceEndpoint = getElementByUid(sourceUid);
      if (sourceEndpoint instanceof CloudViewElement) {
        const [, routedFlow] = UpdateCloudAndFlow(sourceEndpoint, flow, delta);
        // Move cloud by full delta (not clamped) and update flow endpoint to match
        const newCloudX = sourceEndpoint.cx - delta.x;
        const newCloudY = sourceEndpoint.cy - delta.y;
        const movedCloud = sourceEndpoint.merge({ x: newCloudX, y: newCloudY });
        // Update the flow's cloud endpoint to match the full-delta cloud position
        const cloudPointIndex = 0; // source is first point
        const cloudPoint = routedFlow.points.get(cloudPointIndex);
        let updatedFlow = routedFlow;
        if (cloudPoint) {
          const updatedPoints = routedFlow.points.set(
            cloudPointIndex,
            cloudPoint.merge({ x: newCloudX, y: newCloudY }),
          );
          updatedFlow = routedFlow.set('points', updatedPoints);
        }
        sideEffects = sideEffects.push(movedCloud);
        return [updatedFlow, sideEffects];
      }
    } else if (sinkInSelection && sinkUid !== undefined) {
      const sinkEndpoint = getElementByUid(sinkUid);
      if (sinkEndpoint instanceof CloudViewElement) {
        const [, routedFlow] = UpdateCloudAndFlow(sinkEndpoint, flow, delta);
        // Move cloud by full delta (not clamped) and update flow endpoint to match
        const newCloudX = sinkEndpoint.cx - delta.x;
        const newCloudY = sinkEndpoint.cy - delta.y;
        const movedCloud = sinkEndpoint.merge({ x: newCloudX, y: newCloudY });
        // Update the flow's cloud endpoint to match the full-delta cloud position
        const cloudPointIndex = routedFlow.points.size - 1; // sink is last point
        const cloudPoint = routedFlow.points.get(cloudPointIndex);
        let updatedFlow = routedFlow;
        if (cloudPoint) {
          const updatedPoints = routedFlow.points.set(
            cloudPointIndex,
            cloudPoint.merge({ x: newCloudX, y: newCloudY }),
          );
          updatedFlow = routedFlow.set('points', updatedPoints);
        }
        sideEffects = sideEffects.push(movedCloud);
        return [updatedFlow, sideEffects];
      }
    }

    // Fallback: just move valve if we couldn't find the endpoint
    return [
      flow.merge({
        x: flow.cx - delta.x,
        y: flow.cy - delta.y,
      }),
      sideEffects,
    ];
  } else {
    // Neither endpoint is selected: move valve but clamp to flow path
    const proposedValve = {
      x: flow.cx - delta.x,
      y: flow.cy - delta.y,
    };
    const segments = getSegments(pts);
    if (segments.length > 0) {
      const closestSegment = findClosestSegment(proposedValve, segments);
      const clampedValve = clampToSegment(proposedValve, closestSegment);
      return [
        flow.merge({
          x: clampedValve.x,
          y: clampedValve.y,
        }),
        sideEffects,
      ];
    }
    return [
      flow.merge({
        x: proposedValve.x,
        y: proposedValve.y,
      }),
      sideEffects,
    ];
  }
}

/**
 * Route unselected flows attached to selected endpoints.
 *
 * @param elements All view elements (after pass 1 updates)
 * @param originalElements Original view elements (before movement)
 * @param selection Selected UIDs
 * @param preComputedOffsets Pre-computed offsets
 * @param delta The movement delta
 * @returns List of updated flow elements
 */
export function routeUnselectedFlows(
  elements: Iterable<ViewElement>,
  originalElements: Iterable<ViewElement>,
  selection: Set<UID>,
  preComputedOffsets: globalThis.Map<UID, number>,
  delta: Point2D,
): List<ViewElement> {
  let updatedFlows = List<ViewElement>();

  // First, collect flows grouped by their attached endpoint
  const flowsBySourceEndpoint = new globalThis.Map<UID, List<FlowViewElement>>();
  const flowsBySinkEndpoint = new globalThis.Map<UID, List<FlowViewElement>>();
  const bothEndsSelectedFlows: FlowViewElement[] = [];

  for (const element of elements) {
    if (!(element instanceof FlowViewElement)) continue;
    if (selection.has(element.uid)) continue; // Already processed

    const pts = element.points;
    if (pts.size < 2) continue;

    const sourceUid = first(pts).attachedToUid;
    const sinkUid = last(pts).attachedToUid;
    const sourceEndpointSelected = sourceUid !== undefined && selection.has(sourceUid);
    const sinkEndpointSelected = sinkUid !== undefined && selection.has(sinkUid);

    if (sourceEndpointSelected && sinkEndpointSelected) {
      bothEndsSelectedFlows.push(element);
    } else if (sourceEndpointSelected && sourceUid !== undefined) {
      const existing = flowsBySourceEndpoint.get(sourceUid) || List<FlowViewElement>();
      flowsBySourceEndpoint.set(sourceUid, existing.push(element));
    } else if (sinkEndpointSelected && sinkUid !== undefined) {
      const existing = flowsBySinkEndpoint.get(sinkUid) || List<FlowViewElement>();
      flowsBySinkEndpoint.set(sinkUid, existing.push(element));
    }
  }

  // Handle flows where both ends are selected: translate uniformly
  for (const element of bothEndsSelectedFlows) {
    const pts = element.points;
    const newPoints = pts.map((p) =>
      p.merge({
        x: p.x - delta.x,
        y: p.y - delta.y,
      }),
    );
    updatedFlows = updatedFlows.push(
      element.merge({
        x: element.cx - delta.x,
        y: element.cy - delta.y,
        points: newPoints,
      }),
    );
  }

  // Helper to find original element by UID
  const originalElementsMap = new globalThis.Map<UID, ViewElement>();
  for (const el of originalElements) {
    originalElementsMap.set(el.uid, el);
  }
  const getOriginalElement = (uid: UID) => originalElementsMap.get(uid);

  // Update flows grouped by source endpoint using pre-computed offsets
  for (const [endpointUid, flows] of flowsBySourceEndpoint) {
    // Get the moved endpoint from elements
    let movedEndpoint: ViewElement | undefined;
    for (const el of elements) {
      if (el.uid === endpointUid) {
        movedEndpoint = el;
        break;
      }
    }

    if (movedEndpoint instanceof StockViewElement) {
      const originalStock = getOriginalElement(endpointUid) as StockViewElement;
      const newStockCx = originalStock.cx - delta.x;
      const newStockCy = originalStock.cy - delta.y;
      for (const flow of flows) {
        const offset = preComputedOffsets.get(flow.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(flow, originalStock, newStockCx, newStockCy, offset);
        updatedFlows = updatedFlows.push(updatedFlow);
      }
    } else if (movedEndpoint instanceof CloudViewElement) {
      // For clouds, use UpdateCloudAndFlow for orthogonal re-routing
      // Trust UpdateCloudAndFlow's output - it handles axis clamping correctly
      const originalCloud = getOriginalElement(endpointUid) as CloudViewElement | undefined;
      if (originalCloud) {
        for (const flow of flows) {
          const [, updatedFlow] = UpdateCloudAndFlow(originalCloud, flow, delta);
          updatedFlows = updatedFlows.push(updatedFlow);
        }
      }
    }
  }

  // Update flows grouped by sink endpoint using pre-computed offsets
  for (const [endpointUid, flows] of flowsBySinkEndpoint) {
    // Get the moved endpoint from elements
    let movedEndpoint: ViewElement | undefined;
    for (const el of elements) {
      if (el.uid === endpointUid) {
        movedEndpoint = el;
        break;
      }
    }

    if (movedEndpoint instanceof StockViewElement) {
      const originalStock = getOriginalElement(endpointUid) as StockViewElement;
      const newStockCx = originalStock.cx - delta.x;
      const newStockCy = originalStock.cy - delta.y;
      for (const flow of flows) {
        const offset = preComputedOffsets.get(flow.uid) ?? 0.5;
        const updatedFlow = computeFlowRoute(flow, originalStock, newStockCx, newStockCy, offset);
        updatedFlows = updatedFlows.push(updatedFlow);
      }
    } else if (movedEndpoint instanceof CloudViewElement) {
      // For clouds, use UpdateCloudAndFlow for orthogonal re-routing
      // Trust UpdateCloudAndFlow's output - it handles axis clamping correctly
      const originalCloud = getOriginalElement(endpointUid) as CloudViewElement | undefined;
      if (originalCloud) {
        for (const flow of flows) {
          const [, updatedFlow] = UpdateCloudAndFlow(originalCloud, flow, delta);
          updatedFlows = updatedFlows.push(updatedFlow);
        }
      }
    }
  }

  return updatedFlows;
}
