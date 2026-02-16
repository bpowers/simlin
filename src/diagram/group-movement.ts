// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Shared logic for group selection movement.
 *
 * This module provides functions for computing how diagram elements should
 * move when a group selection is dragged. The same logic is used by both
 * Editor.tsx (for persisting changes) and Canvas.tsx (for live preview).
 */

import { List, Map, Set } from 'immutable';
import { first, last } from '@simlin/core/collections';
import {
  ViewElement,
  FlowViewElement,
  StockViewElement,
  CloudViewElement,
  AuxViewElement,
  LinkViewElement,
  ModuleViewElement,
  AliasViewElement,
  GroupViewElement,
  UID,
  Point,
} from '@simlin/core/datamodel';
import { updateArcAngle, radToDeg } from './arc-utils';
import { getVisualCenter, takeoffθ } from './drawing/Connector';
import {
  clampToSegment,
  computeFlowOffsets,
  computeFlowRoute,
  findClosestSegment,
  getSegments,
  UpdateCloudAndFlow,
  UpdateFlow,
} from './drawing/Flow';

// Tolerance for floating-point comparison when checking if two movements are equal
const MOVEMENT_EQUALITY_EPSILON = 0.1;

// Minimum distance in pixels before requiring L-shape routing to avoid diagonal flows
const MIN_DIAGONAL_DISTANCE = 1;

/**
 * Represents a 2D movement delta (change in position).
 * Distinct from Point which represents an absolute position with optional attachment.
 */
export interface MovementDelta {
  x: number;
  y: number;
}

/**
 * @deprecated Use MovementDelta instead. Kept for backwards compatibility.
 */
export type Point2D = MovementDelta;

export interface GroupMovementResult {
  /** Map from element UID to updated element (for elements that changed) */
  updatedElements: Map<UID, ViewElement>;
  /** Additional elements to update (clouds updated via flow routing, etc.) */
  sideEffects: List<ViewElement>;
}

/**
 * Result of routing a flow attached to a cloud endpoint.
 */
interface CloudFlowRouteResult {
  updatedFlow: FlowViewElement;
  movedCloud: CloudViewElement;
}

/**
 * Route a flow attached to a cloud endpoint during group movement.
 *
 * This handles:
 * - Moving the cloud by the full delta
 * - Creating an L-shaped flow if the movement would create a diagonal
 * - Re-clamping the valve to the new flow path
 *
 * @param cloud The cloud endpoint being moved
 * @param flow The flow attached to the cloud
 * @param delta The movement delta
 * @param isSource True if the cloud is the source (first point), false if sink (last point)
 * @returns The updated flow and moved cloud
 */
function routeCloudEndpointFlow(
  cloud: CloudViewElement,
  flow: FlowViewElement,
  delta: MovementDelta,
  isSource: boolean,
): CloudFlowRouteResult {
  const [, routedFlow] = UpdateCloudAndFlow(cloud, flow, delta);

  const newCloudX = cloud.cx - delta.x;
  const newCloudY = cloud.cy - delta.y;
  const movedCloud = cloud.merge({ x: newCloudX, y: newCloudY });

  const cloudPointIndex = isSource ? 0 : routedFlow.points.size - 1;
  const otherPointIndex = isSource ? routedFlow.points.size - 1 : 0;
  const cloudPoint = routedFlow.points.get(cloudPointIndex);
  const otherPoint = routedFlow.points.get(otherPointIndex);

  let updatedFlow = routedFlow;
  if (cloudPoint && otherPoint) {
    // Check if the flow is 2-point straight and movement would create a diagonal
    const needsLShape =
      routedFlow.points.size === 2 &&
      Math.abs(newCloudX - otherPoint.x) > MIN_DIAGONAL_DISTANCE &&
      Math.abs(newCloudY - otherPoint.y) > MIN_DIAGONAL_DISTANCE;

    if (needsLShape) {
      // Add intermediate point to create L-shape (horizontal then vertical)
      const intermediatePoint = new Point({ x: newCloudX, y: otherPoint.y, attachedToUid: undefined });
      const newCloudPoint = cloudPoint.merge({ x: newCloudX, y: newCloudY });
      // Order depends on whether cloud is source or sink
      const newPoints = isSource
        ? List([newCloudPoint, intermediatePoint, otherPoint])
        : List([otherPoint, intermediatePoint, newCloudPoint]);
      updatedFlow = routedFlow.set('points', newPoints);

      // Re-clamp valve to the new path
      const segments = getSegments(newPoints);
      if (segments.length > 0) {
        const closestSegment = findClosestSegment({ x: updatedFlow.cx, y: updatedFlow.cy }, segments);
        const clampedValve = clampToSegment({ x: updatedFlow.cx, y: updatedFlow.cy }, closestSegment);
        updatedFlow = updatedFlow.merge({ x: clampedValve.x, y: clampedValve.y });
      }
    } else {
      // Just update the cloud point position
      const updatedPoints = routedFlow.points.set(cloudPointIndex, cloudPoint.merge({ x: newCloudX, y: newCloudY }));
      updatedFlow = routedFlow.set('points', updatedPoints);
    }
  }

  return { updatedFlow, movedCloud };
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

  // Materialize the iterable into an array so we can iterate multiple times.
  // The caller may pass a single-use iterator (e.g., Map.values()), and we need
  // to iterate once for stocks (outer loop) and again for flows (inner loop).
  const allElements = Array.from(elements);

  for (const element of allElements) {
    if (!(element instanceof StockViewElement)) continue;
    if (!selectedStockUids.has(element.uid)) continue;

    // Collect ALL flows attached to this stock for proper offset computation.
    // For flows where both endpoints are selected, we translate their points by delta
    // so their anchor position is correct relative to the stock's new position.
    // This ensures translated flows reserve their slots and don't overlap with routed flows.
    let allFlows = List<FlowViewElement>();
    for (const el of allElements) {
      if (!(el instanceof FlowViewElement)) continue;
      const pts = el.points;
      if (pts.size < 2) continue;
      const sourceUid = first(pts).attachedToUid;
      const sinkUid = last(pts).attachedToUid;
      const attachedToThisStock = sourceUid === element.uid || sinkUid === element.uid;
      if (!attachedToThisStock) continue;

      const otherEndpointUid = sourceUid === element.uid ? sinkUid : sourceUid;
      const bothEndpointsSelected = isInSelection(otherEndpointUid);

      if (bothEndpointsSelected) {
        // Translate points by delta so anchor is correct relative to new stock position
        const translatedPoints = pts.map((p) =>
          p.merge({
            x: p.x - delta.x,
            y: p.y - delta.y,
          }),
        );
        allFlows = allFlows.push(el.set('points', translatedPoints));
      } else {
        allFlows = allFlows.push(el);
      }
    }

    // Compute offsets at the new stock position
    const newStockCx = element.cx - delta.x;
    const newStockCy = element.cy - delta.y;
    const offsets = computeFlowOffsets(allFlows, element.uid, newStockCx, newStockCy);

    // Store offsets for flows
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
      // When the flow is selected, the valve should move with the drag delta,
      // then be clamped to the new flow path. The pre-processed flow has the
      // valve's fractional position preserved, which is wrong when selected.
      const proposedValve = {
        x: flow.cx - delta.x,
        y: flow.cy - delta.y,
      };
      const segments = getSegments(preProcessed.points);
      if (segments.length > 0) {
        const closestSegment = findClosestSegment(proposedValve, segments);
        const clampedValve = clampToSegment(proposedValve, closestSegment);
        return [preProcessed.merge({ x: clampedValve.x, y: clampedValve.y }), sideEffects];
      }
      return [preProcessed, sideEffects];
    }

    // Handle different endpoint types: stocks need computeFlowRoute, clouds need UpdateCloudAndFlow
    if (sourceInSelection && sourceUid !== undefined) {
      const sourceEndpoint = getElementByUid(sourceUid);
      if (sourceEndpoint instanceof StockViewElement) {
        // Route flow from moved stock to fixed sink endpoint
        const newStockCx = sourceEndpoint.cx - delta.x;
        const newStockCy = sourceEndpoint.cy - delta.y;
        // Use default offset of 0.5 (center) for selected flows
        let updatedFlow = computeFlowRoute(flow, sourceEndpoint, newStockCx, newStockCy, 0.5);

        // When the flow is selected, the valve should move with the drag delta,
        // then be clamped to the new flow path. computeFlowRoute preserves the
        // valve's fractional position which is wrong when the flow is selected.
        const proposedValve = {
          x: flow.cx - delta.x,
          y: flow.cy - delta.y,
        };
        const segments = getSegments(updatedFlow.points);
        if (segments.length > 0) {
          const closestSegment = findClosestSegment(proposedValve, segments);
          const clampedValve = clampToSegment(proposedValve, closestSegment);
          updatedFlow = updatedFlow.merge({ x: clampedValve.x, y: clampedValve.y });
        }
        return [updatedFlow, sideEffects];
      } else if (sourceEndpoint instanceof CloudViewElement) {
        const { updatedFlow, movedCloud } = routeCloudEndpointFlow(sourceEndpoint, flow, delta, true);
        sideEffects = sideEffects.push(movedCloud);
        return [updatedFlow, sideEffects];
      }
    } else if (sinkInSelection && sinkUid !== undefined) {
      const sinkEndpoint = getElementByUid(sinkUid);
      if (sinkEndpoint instanceof StockViewElement) {
        // Route flow from fixed source to moved stock
        const newStockCx = sinkEndpoint.cx - delta.x;
        const newStockCy = sinkEndpoint.cy - delta.y;
        // Use default offset of 0.5 (center) for selected flows
        let updatedFlow = computeFlowRoute(flow, sinkEndpoint, newStockCx, newStockCy, 0.5);

        // When the flow is selected, the valve should move with the drag delta,
        // then be clamped to the new flow path. computeFlowRoute preserves the
        // valve's fractional position which is wrong when the flow is selected.
        const proposedValve = {
          x: flow.cx - delta.x,
          y: flow.cy - delta.y,
        };
        const segments = getSegments(updatedFlow.points);
        if (segments.length > 0) {
          const closestSegment = findClosestSegment(proposedValve, segments);
          const clampedValve = clampToSegment(proposedValve, closestSegment);
          updatedFlow = updatedFlow.merge({ x: clampedValve.x, y: clampedValve.y });
        }
        return [updatedFlow, sideEffects];
      } else if (sinkEndpoint instanceof CloudViewElement) {
        const { updatedFlow, movedCloud } = routeCloudEndpointFlow(sinkEndpoint, flow, delta, false);
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
    // Neither endpoint is selected. For cloud-cloud flows, move the entire flow
    // and both clouds together. For flows with cloud endpoints, use UpdateFlow
    // to allow perpendicular drag rerouting. For stock-to-stock flows, just move
    // the valve.
    const sourceEl = sourceUid !== undefined ? getElementByUid(sourceUid) : undefined;
    const sinkEl = sinkUid !== undefined ? getElementByUid(sinkUid) : undefined;
    const sourceIsCloud = sourceEl instanceof CloudViewElement;
    const sinkIsCloud = sinkEl instanceof CloudViewElement;
    const sourceIsStock = sourceEl instanceof StockViewElement;
    const sinkIsStock = sinkEl instanceof StockViewElement;
    const hasCloud = sourceIsCloud || sinkIsCloud;

    // Cloud-to-cloud flows: translate everything uniformly (matches UpdateFlow behavior)
    if (sourceIsCloud && sinkIsCloud) {
      const newPoints = pts.map((p) =>
        p.merge({
          x: p.x - delta.x,
          y: p.y - delta.y,
        }),
      );
      const updatedFlow = flow.merge({
        x: flow.cx - delta.x,
        y: flow.cy - delta.y,
        points: newPoints,
      });
      // Update both clouds as side effects
      const movedSourceCloud = (sourceEl as CloudViewElement).merge({
        x: sourceEl.cx - delta.x,
        y: sourceEl.cy - delta.y,
      });
      const movedSinkCloud = (sinkEl as CloudViewElement).merge({
        x: sinkEl.cx - delta.x,
        y: sinkEl.cy - delta.y,
      });
      sideEffects = sideEffects.push(movedSourceCloud, movedSinkCloud);
      return [updatedFlow, sideEffects];
    }

    // Cloud-stock flows: delegate to UpdateFlow for perpendicular drag L-shape behavior
    if (hasCloud && sourceEl && sinkEl && (sourceIsStock || sourceIsCloud) && (sinkIsStock || sinkIsCloud)) {
      const ends = List<StockViewElement | CloudViewElement>([
        sourceEl as StockViewElement | CloudViewElement,
        sinkEl as StockViewElement | CloudViewElement,
      ]);
      const [newFlow, newClouds] = UpdateFlow(flow, ends, delta, undefined);
      for (const cloud of newClouds) {
        sideEffects = sideEffects.push(cloud);
      }
      return [newFlow, sideEffects];
    }

    // Stock-to-stock flows: move valve but clamp to flow path
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

  // Build maps FIRST so we can reuse them (iterators can only be consumed once)
  const elementsMap = new globalThis.Map<UID, ViewElement>();
  for (const el of elements) {
    elementsMap.set(el.uid, el);
  }
  const originalElementsMap = new globalThis.Map<UID, ViewElement>();
  for (const el of originalElements) {
    originalElementsMap.set(el.uid, el);
  }
  const getOriginalElement = (uid: UID) => originalElementsMap.get(uid);

  // Collect flows grouped by their attached endpoint
  const flowsBySourceEndpoint = new globalThis.Map<UID, List<FlowViewElement>>();
  const flowsBySinkEndpoint = new globalThis.Map<UID, List<FlowViewElement>>();
  const bothEndsSelectedFlows: FlowViewElement[] = [];

  for (const element of elementsMap.values()) {
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

  // Update flows grouped by source endpoint using pre-computed offsets
  for (const [endpointUid, flows] of flowsBySourceEndpoint) {
    const movedEndpoint = elementsMap.get(endpointUid);

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
      // For clouds, use UpdateCloudAndFlow for routing but honor full delta if cloud is selected
      const originalCloud = getOriginalElement(endpointUid) as CloudViewElement | undefined;
      if (originalCloud) {
        const cloudIsSelected = selection.has(endpointUid);
        const newCloudX = originalCloud.cx - delta.x;
        const newCloudY = originalCloud.cy - delta.y;
        for (const flow of flows) {
          let [, updatedFlow] = UpdateCloudAndFlow(originalCloud, flow, delta);
          // If cloud is selected, ensure flow matches full delta position with proper orthogonal geometry
          if (cloudIsSelected) {
            const cloudPointIndex =
              first(updatedFlow.points).attachedToUid === endpointUid ? 0 : updatedFlow.points.size - 1;
            const cloudPoint = updatedFlow.points.get(cloudPointIndex);
            const otherPointIndex = cloudPointIndex === 0 ? updatedFlow.points.size - 1 : 0;
            const otherPoint = updatedFlow.points.get(otherPointIndex);
            if (cloudPoint && otherPoint) {
              // Check if the flow is 2-point straight and we'd create a diagonal
              const needsLShape =
                updatedFlow.points.size === 2 &&
                Math.abs(newCloudX - otherPoint.x) > MIN_DIAGONAL_DISTANCE &&
                Math.abs(newCloudY - otherPoint.y) > MIN_DIAGONAL_DISTANCE;
              if (needsLShape) {
                // Add intermediate point to create L-shape (horizontal then vertical)
                const intermediateX = newCloudX;
                const intermediateY = otherPoint.y;
                const intermediatePoint = new Point({ x: intermediateX, y: intermediateY, attachedToUid: undefined });
                const newCloudPoint = cloudPoint.merge({ x: newCloudX, y: newCloudY });
                // Cloud is source (index 0): cloud -> intermediate -> other
                // Cloud is sink (index size-1): other -> intermediate -> cloud
                const newPoints =
                  cloudPointIndex === 0
                    ? List([newCloudPoint, intermediatePoint, otherPoint])
                    : List([otherPoint, intermediatePoint, newCloudPoint]);
                updatedFlow = updatedFlow.set('points', newPoints);
                // Re-clamp valve to the new path
                const segments = getSegments(newPoints);
                if (segments.length > 0) {
                  const closestSegment = findClosestSegment({ x: updatedFlow.cx, y: updatedFlow.cy }, segments);
                  const clampedValve = clampToSegment({ x: updatedFlow.cx, y: updatedFlow.cy }, closestSegment);
                  updatedFlow = updatedFlow.merge({ x: clampedValve.x, y: clampedValve.y });
                }
              } else {
                const updatedPoints = updatedFlow.points.set(
                  cloudPointIndex,
                  cloudPoint.merge({ x: newCloudX, y: newCloudY }),
                );
                updatedFlow = updatedFlow.set('points', updatedPoints);
              }
            }
          }
          updatedFlows = updatedFlows.push(updatedFlow);
        }
      }
    }
  }

  // Update flows grouped by sink endpoint using pre-computed offsets
  for (const [endpointUid, flows] of flowsBySinkEndpoint) {
    const movedEndpoint = elementsMap.get(endpointUid);

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
      // For clouds, use UpdateCloudAndFlow for routing but honor full delta if cloud is selected
      const originalCloud = getOriginalElement(endpointUid) as CloudViewElement | undefined;
      if (originalCloud) {
        const cloudIsSelected = selection.has(endpointUid);
        const newCloudX = originalCloud.cx - delta.x;
        const newCloudY = originalCloud.cy - delta.y;
        for (const flow of flows) {
          let [, updatedFlow] = UpdateCloudAndFlow(originalCloud, flow, delta);
          // If cloud is selected, ensure flow matches full delta position with proper orthogonal geometry
          if (cloudIsSelected) {
            const cloudPointIndex =
              last(updatedFlow.points).attachedToUid === endpointUid ? updatedFlow.points.size - 1 : 0;
            const cloudPoint = updatedFlow.points.get(cloudPointIndex);
            const otherPointIndex = cloudPointIndex === 0 ? updatedFlow.points.size - 1 : 0;
            const otherPoint = updatedFlow.points.get(otherPointIndex);
            if (cloudPoint && otherPoint) {
              // Check if the flow is 2-point straight and we'd create a diagonal
              const needsLShape =
                updatedFlow.points.size === 2 &&
                Math.abs(newCloudX - otherPoint.x) > MIN_DIAGONAL_DISTANCE &&
                Math.abs(newCloudY - otherPoint.y) > MIN_DIAGONAL_DISTANCE;
              if (needsLShape) {
                // Add intermediate point to create L-shape (horizontal then vertical)
                const intermediateX = newCloudX;
                const intermediateY = otherPoint.y;
                const intermediatePoint = new Point({ x: intermediateX, y: intermediateY, attachedToUid: undefined });
                const newCloudPoint = cloudPoint.merge({ x: newCloudX, y: newCloudY });
                // Cloud is source (index 0): cloud -> intermediate -> other
                // Cloud is sink (index size-1): other -> intermediate -> cloud
                const newPoints =
                  cloudPointIndex === 0
                    ? List([newCloudPoint, intermediatePoint, otherPoint])
                    : List([otherPoint, intermediatePoint, newCloudPoint]);
                updatedFlow = updatedFlow.set('points', newPoints);
                // Re-clamp valve to the new path
                const segments = getSegments(newPoints);
                if (segments.length > 0) {
                  const closestSegment = findClosestSegment({ x: updatedFlow.cx, y: updatedFlow.cy }, segments);
                  const clampedValve = clampToSegment({ x: updatedFlow.cx, y: updatedFlow.cy }, closestSegment);
                  updatedFlow = updatedFlow.merge({ x: clampedValve.x, y: clampedValve.y });
                }
              } else {
                const updatedPoints = updatedFlow.points.set(
                  cloudPointIndex,
                  cloudPoint.merge({ x: newCloudX, y: newCloudY }),
                );
                updatedFlow = updatedFlow.set('points', updatedPoints);
              }
            }
          }
          updatedFlows = updatedFlows.push(updatedFlow);
        }
      }
    }
  }

  return updatedFlows;
}

/**
 * Input for the unified applyGroupMovement function.
 */
export interface GroupMovementInput {
  /**
   * All view elements in the diagram. This can be any Iterable (List, array, Map.values(), etc.).
   *
   * IMPORTANT: The iterator will be consumed exactly once when building an internal lookup Map.
   * This allows callers to pass any Iterable without pre-materializing it, while the function
   * handles the materialization internally for efficient repeated access.
   */
  elements: Iterable<ViewElement>;
  /** UIDs of elements in the selection that should move */
  selection: Set<UID>;
  /** Movement delta to apply (subtracted from positions, so negative = move right/down) */
  delta: MovementDelta;
  /** For single-link arc drag: the current drag position */
  arcPoint?: MovementDelta;
  /** For single-flow segment drag: which segment is being dragged */
  segmentIndex?: number;
}

/**
 * Output from the unified applyGroupMovement function.
 */
export interface GroupMovementOutput {
  updatedElements: Map<UID, ViewElement>;
}

/**
 * Process links during group movement.
 *
 * Links are processed LAST because flows may re-route during group movement,
 * so we need to use the actual final positions of endpoints (not assume they
 * moved by exactly `delta`).
 *
 * @param links Links to process
 * @param originalElements Map of original elements (before movement)
 * @param updatedElements Map of updated elements (after movement)
 * @param selection Set of selected UIDs
 * @param delta Movement delta
 * @param arcPoint Optional arc point for single-link drag
 * @returns Map of updated link elements
 */
export function processLinks(
  links: Iterable<LinkViewElement>,
  originalElements: Map<UID, ViewElement>,
  updatedElements: Map<UID, ViewElement>,
  selection: Set<UID>,
  delta: Point2D,
  arcPoint?: Point2D,
): Map<UID, LinkViewElement> {
  let result = Map<UID, LinkViewElement>();

  for (const link of links) {
    // Get original and updated endpoint positions
    const oldFrom = originalElements.get(link.fromUid);
    const oldTo = originalElements.get(link.toUid);
    if (!oldFrom || !oldTo) {
      continue;
    }

    const newFrom = updatedElements.get(link.fromUid) ?? oldFrom;
    const newTo = updatedElements.get(link.toUid) ?? oldTo;

    // Check if both endpoints moved by the same amount (pure translation)
    const fromDelta = { x: oldFrom.cx - newFrom.cx, y: oldFrom.cy - newFrom.cy };
    const toDelta = { x: oldTo.cx - newTo.cx, y: oldTo.cy - newTo.cy };
    const sameMovement =
      Math.abs(fromDelta.x - toDelta.x) < MOVEMENT_EQUALITY_EPSILON &&
      Math.abs(fromDelta.y - toDelta.y) < MOVEMENT_EQUALITY_EPSILON;
    const didMove = fromDelta.x !== 0 || fromDelta.y !== 0 || toDelta.x !== 0 || toDelta.y !== 0;

    // Single link selection with arcPoint: adjust arc based on drag position
    if (selection.size === 1 && selection.has(link.uid) && arcPoint) {
      const newTakeoff = takeoffθ({
        element: link,
        from: oldFrom,
        to: oldTo,
        arcPoint: { x: arcPoint.x, y: arcPoint.y },
      });
      result = result.set(link.uid, link.merge({ arc: radToDeg(newTakeoff) }));
    } else if (sameMovement && didMove) {
      // Both endpoints moved together - translate multiPoint if present, keep arc
      if (link.multiPoint) {
        const translatedMultiPoint = link.multiPoint.map((p) =>
          p.merge({
            x: p.x - fromDelta.x,
            y: p.y - fromDelta.y,
          }),
        );
        result = result.set(link.uid, link.merge({ multiPoint: translatedMultiPoint }));
      }
      // arc is preserved (no change needed)
    } else if (didMove) {
      // Endpoints moved differently - adjust arc based on rotation of the
      // line between endpoints' visual centers
      const oldFromVisual = getVisualCenter(oldFrom);
      const oldToVisual = getVisualCenter(oldTo);
      const newFromVisual = getVisualCenter(newFrom);
      const newToVisual = getVisualCenter(newTo);

      const oldθ = Math.atan2(oldToVisual.cy - oldFromVisual.cy, oldToVisual.cx - oldFromVisual.cx);
      const newθ = Math.atan2(newToVisual.cy - newFromVisual.cy, newToVisual.cx - newFromVisual.cx);
      const diffθ = oldθ - newθ;

      result = result.set(link.uid, link.merge({ arc: updateArcAngle(link.arc, radToDeg(diffθ)) }));
    }
  }

  return result;
}

/**
 * Unified function to apply group movement to all element types.
 *
 * This function handles:
 * 1. Single-element movement (delegates to existing helpers for special cases)
 * 2. Multi-element group movement with proper flow routing and link arc adjustment
 *
 * Processing order:
 * 1. Move stocks and auxes by delta
 * 2. Pre-compute flow offsets for multi-flow spacing
 * 3. Process selected flows (route or translate based on endpoint selection)
 * 4. Move selected clouds by delta
 * 5. Route unselected flows attached to selected endpoints
 * 6. Process links LAST (using actual updated positions)
 *
 * @param input Movement input parameters
 * @returns Map of element UID to updated element
 */
export function applyGroupMovement(input: GroupMovementInput): GroupMovementOutput {
  const { elements, selection, delta, arcPoint, segmentIndex } = input;

  // Build maps of elements for efficient lookup
  const originalElements = Map<UID, ViewElement>().withMutations((mutable) => {
    for (const el of elements) {
      mutable.set(el.uid, el);
    }
  });

  // Helper to check if a UID is in selection
  const isInSelection = (uid: UID | undefined): boolean => {
    return uid !== undefined && selection.has(uid);
  };

  // Classify elements by type
  const selectedStockUids = new globalThis.Set<UID>();
  const selectedFlowUids = Set<UID>().withMutations((mutable) => {
    for (const uid of selection) {
      const el = originalElements.get(uid);
      if (!el) continue;
      if (el instanceof StockViewElement) {
        selectedStockUids.add(uid);
      } else if (el instanceof FlowViewElement) {
        mutable.add(uid);
      }
    }
  });

  let updatedElements = originalElements;

  // First pass: Move positioned elements (stocks, auxes, clouds, modules, aliases, groups) by delta
  for (const uid of selection) {
    const el = originalElements.get(uid);
    if (!el) continue;

    if (el instanceof StockViewElement) {
      updatedElements = updatedElements.set(
        uid,
        el.merge({
          x: el.cx - delta.x,
          y: el.cy - delta.y,
        }),
      );
    } else if (el instanceof AuxViewElement) {
      updatedElements = updatedElements.set(
        uid,
        el.merge({
          x: el.cx - delta.x,
          y: el.cy - delta.y,
        }),
      );
    } else if (el instanceof CloudViewElement) {
      updatedElements = updatedElements.set(
        uid,
        el.merge({
          x: el.cx - delta.x,
          y: el.cy - delta.y,
        }),
      );
    } else if (el instanceof ModuleViewElement) {
      updatedElements = updatedElements.set(
        uid,
        el.merge({
          x: el.cx - delta.x,
          y: el.cy - delta.y,
        }),
      );
    } else if (el instanceof AliasViewElement) {
      updatedElements = updatedElements.set(
        uid,
        el.merge({
          x: el.cx - delta.x,
          y: el.cy - delta.y,
        }),
      );
    } else if (el instanceof GroupViewElement) {
      updatedElements = updatedElements.set(
        uid,
        el.merge({
          x: el.cx - delta.x,
          y: el.cy - delta.y,
        }),
      );
    }
  }

  // Pre-compute flow offsets for all flows attached to moved stocks.
  // This applies even for single-stock selection to preserve multi-flow spacing.
  // Note: We use originalElements.values() since the input `elements` iterator was already consumed.
  const preComputedOffsets =
    selectedStockUids.size > 0
      ? computePreRoutedOffsets(originalElements.values(), selectedStockUids, delta, isInSelection)
      : new globalThis.Map<UID, number>();

  // Pre-process selected flows with one endpoint selected (stock endpoint only)
  const [preProcessedFlows] =
    selection.size > 1
      ? preProcessSelectedFlows(
          originalElements.values(),
          selectedFlowUids,
          preComputedOffsets,
          delta,
          isInSelection,
          (uid) => originalElements.get(uid),
        )
      : [new globalThis.Map<UID, FlowViewElement>()];

  // Process selected flows
  for (const uid of selection) {
    const el = originalElements.get(uid);
    if (!(el instanceof FlowViewElement)) continue;

    // Single-flow selection with segmentIndex: move segment
    if (selection.size === 1 && segmentIndex !== undefined) {
      const pts = el.points;
      if (pts.size >= 2) {
        const sourceId = first(pts).attachedToUid;
        const sinkId = last(pts).attachedToUid;
        const source = sourceId !== undefined ? originalElements.get(sourceId) : undefined;
        const sink = sinkId !== undefined ? originalElements.get(sinkId) : undefined;

        if (
          source &&
          sink &&
          (source instanceof StockViewElement || source instanceof CloudViewElement) &&
          (sink instanceof StockViewElement || sink instanceof CloudViewElement)
        ) {
          const ends = List<StockViewElement | CloudViewElement>([source, sink]);
          const [newFlow, newClouds] = UpdateFlow(el, ends, delta, segmentIndex);
          updatedElements = updatedElements.set(uid, newFlow);
          for (const cloud of newClouds) {
            updatedElements = updatedElements.set(cloud.uid, cloud);
          }
        }
      }
      continue;
    }

    // Single-flow selection without segmentIndex: delegate to UpdateFlow for flows
    // with cloud endpoints to preserve perpendicular drag L-shape behavior
    if (selection.size === 1) {
      const pts = el.points;
      if (pts.size >= 2) {
        const sourceId = first(pts).attachedToUid;
        const sinkId = last(pts).attachedToUid;
        const source = sourceId !== undefined ? originalElements.get(sourceId) : undefined;
        const sink = sinkId !== undefined ? originalElements.get(sinkId) : undefined;
        const hasCloud = source instanceof CloudViewElement || sink instanceof CloudViewElement;

        if (
          hasCloud &&
          source &&
          sink &&
          (source instanceof StockViewElement || source instanceof CloudViewElement) &&
          (sink instanceof StockViewElement || sink instanceof CloudViewElement)
        ) {
          const ends = List<StockViewElement | CloudViewElement>([source, sink]);
          const [newFlow, newClouds] = UpdateFlow(el, ends, delta, undefined);
          updatedElements = updatedElements.set(uid, newFlow);
          for (const cloud of newClouds) {
            updatedElements = updatedElements.set(cloud.uid, cloud);
          }
          continue;
        }
      }
    }

    const [newFlow, sideEffects] = processSelectedFlow(el, delta, isInSelection, preProcessedFlows, (flowUid) =>
      originalElements.get(flowUid),
    );
    updatedElements = updatedElements.set(uid, newFlow);
    for (const sideEffect of sideEffects) {
      updatedElements = updatedElements.set(sideEffect.uid, sideEffect);
    }
  }

  // Route unselected flows attached to selected endpoints.
  // This applies even for single-element selection (e.g., moving a single stock
  // should route its attached flows).
  // Note: We use originalElements.values() here because the input `elements` iterator
  // was already consumed when building originalElements.
  const hasSelectedEndpoints = selectedStockUids.size > 0 || selection.size > 0;
  if (hasSelectedEndpoints) {
    const routedFlows = routeUnselectedFlows(
      originalElements.values(),
      originalElements.values(),
      selection,
      preComputedOffsets,
      delta,
    );
    for (const flow of routedFlows) {
      updatedElements = updatedElements.set(flow.uid, flow);
    }
  }

  // Process links LAST using actual updated positions
  const links: LinkViewElement[] = [];
  for (const el of originalElements.values()) {
    if (el instanceof LinkViewElement) {
      // Include link if it's selected OR if either endpoint was updated
      const fromUpdated = updatedElements.get(el.fromUid) !== originalElements.get(el.fromUid);
      const toUpdated = updatedElements.get(el.toUid) !== originalElements.get(el.toUid);
      if (selection.has(el.uid) || fromUpdated || toUpdated) {
        links.push(el);
      }
    }
  }

  const updatedLinks = processLinks(links, originalElements, updatedElements, selection, delta, arcPoint);
  for (const [uid, link] of updatedLinks) {
    updatedElements = updatedElements.set(uid, link);
  }

  // Filter to only return elements that actually changed
  let result = Map<UID, ViewElement>();
  for (const [uid, newEl] of updatedElements) {
    const oldEl = originalElements.get(uid);
    if (newEl !== oldEl) {
      result = result.set(uid, newEl);
    }
  }

  return { updatedElements: result };
}
