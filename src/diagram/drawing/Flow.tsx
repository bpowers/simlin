// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import clsx from 'clsx';

import {
  Point,
  FlowViewElement,
  ViewElement,
  StockViewElement,
  CloudViewElement,
} from '@system-dynamics/core/datamodel';
import { defined, Series } from '@system-dynamics/core/common';

import { Arrowhead } from './Arrowhead';
import { displayName, Point as IPoint, Rect } from './common';
import { AuxRadius, CloudRadius, FlowArrowheadRadius } from './default';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';
import { StockHeight, StockWidth } from './Stock';

import styles from './Flow.module.css';

const atan2 = Math.atan2;
const PI = Math.PI;

type Side = 'left' | 'right' | 'top' | 'bottom';

function getStockEdgePoint(stockCx: number, stockCy: number, side: Side): IPoint {
  switch (side) {
    case 'left':
      return { x: stockCx - StockWidth / 2, y: stockCy };
    case 'right':
      return { x: stockCx + StockWidth / 2, y: stockCy };
    case 'top':
      return { x: stockCx, y: stockCy - StockHeight / 2 };
    case 'bottom':
      return { x: stockCx, y: stockCy + StockHeight / 2 };
  }
}

function canFlowBeStraight(
  stockCx: number,
  stockCy: number,
  anchorX: number,
  anchorY: number,
  originalFlowIsHorizontal: boolean,
): boolean {
  if (originalFlowIsHorizontal) {
    return Math.abs(stockCy - anchorY) <= StockHeight / 2;
  } else {
    return Math.abs(stockCx - anchorX) <= StockWidth / 2;
  }
}

// Exported for testing
export function computeFlowRoute(
  flow: FlowViewElement,
  stockEl: StockViewElement,
  newStockCx: number,
  newStockCy: number,
): FlowViewElement {
  const points = flow.points;
  if (points.size < 2) {
    return flow;
  }

  const firstPoint = defined(points.first());
  const lastPoint = defined(points.last());

  const stockIsFirst = firstPoint.attachedToUid === stockEl.uid;
  const stockIsLast = lastPoint.attachedToUid === stockEl.uid;

  if (!stockIsFirst && !stockIsLast) {
    return flow;
  }

  const anchor = stockIsFirst ? lastPoint : firstPoint;

  // Determine original flow direction by looking at the anchor-side segment.
  // This works for both 2-point (straight) and 3+ point (L-shaped) flows.
  // For L-shaped flows, the anchor-side segment preserves the original direction.
  let anchorAdjacentPoint: Point;
  if (stockIsFirst) {
    // anchor is last, so look at second-to-last point
    anchorAdjacentPoint = points.size >= 2 ? defined(points.get(points.size - 2)) : firstPoint;
  } else {
    // anchor is first, so look at second point
    anchorAdjacentPoint = points.size >= 2 ? defined(points.get(1)) : lastPoint;
  }
  const originalFlowIsHorizontal = anchor.y === anchorAdjacentPoint.y;

  // For flows with 4+ points (imported or manually-edited multi-segment flows),
  // update both the attached endpoint and the adjacent corner to preserve
  // orthogonality. The endpoint stays on the stock edge, and the adjacent
  // corner is adjusted to maintain axis alignment.
  if (points.size >= 4) {
    const adjacentPointIndex = stockIsFirst ? 1 : points.size - 2;
    const adjacentPoint = defined(points.get(adjacentPointIndex));

    // Determine the ORIGINAL first segment's orientation from the existing geometry.
    // We must preserve this orientation to avoid creating diagonal segments between
    // corner1 and corner2 when the stock moves far perpendicular to the segment.
    const currentStockPoint = stockIsFirst ? firstPoint : lastPoint;
    const isHorizontalSegment = currentStockPoint.y === adjacentPoint.y;

    // Choose the attachment side based on the preserved orientation
    let side: Side;
    if (isHorizontalSegment) {
      // Horizontal segment: attach left or right based on adjacent point's X
      side = adjacentPoint.x > newStockCx ? 'right' : 'left';
    } else {
      // Vertical segment: attach top or bottom based on adjacent point's Y
      side = adjacentPoint.y > newStockCy ? 'bottom' : 'top';
    }

    // Keep the endpoint on the stock's actual edge
    const stockEdge = getStockEdgePoint(newStockCx, newStockCy, side);

    const newStockPoint = new Point({
      x: stockEdge.x,
      y: stockEdge.y,
      attachedToUid: stockEl.uid,
    });

    // Adjust the adjacent corner to preserve orthogonality
    let newAdjacentPoint: Point;
    if (isHorizontalSegment) {
      // For horizontal segment, corner's Y must match endpoint's Y
      newAdjacentPoint = new Point({
        x: adjacentPoint.x,
        y: stockEdge.y,
        attachedToUid: adjacentPoint.attachedToUid,
      });
    } else {
      // For vertical segment, corner's X must match endpoint's X
      newAdjacentPoint = new Point({
        x: stockEdge.x,
        y: adjacentPoint.y,
        attachedToUid: adjacentPoint.attachedToUid,
      });
    }

    let newPoints: List<Point>;
    if (stockIsFirst) {
      newPoints = points.set(0, newStockPoint).set(1, newAdjacentPoint);
    } else {
      newPoints = points.set(points.size - 1, newStockPoint).set(points.size - 2, newAdjacentPoint);
    }

    // Update valve position by clamping to the nearest segment.
    // This ensures the valve stays on the flow path after the endpoint moves.
    const newSegments = getSegments(newPoints);
    const currentValve: IPoint = { x: flow.cx, y: flow.cy };
    const closestSegment = findClosestSegment(currentValve, newSegments);
    const clampedValve = clampToSegment(currentValve, closestSegment);

    return flow.merge({
      x: clampedValve.x,
      y: clampedValve.y,
      points: newPoints,
    });
  }

  if (canFlowBeStraight(newStockCx, newStockCy, anchor.x, anchor.y, originalFlowIsHorizontal)) {
    let stockEdge: IPoint;
    if (originalFlowIsHorizontal) {
      const side: Side = anchor.x > newStockCx ? 'right' : 'left';
      stockEdge = getStockEdgePoint(newStockCx, anchor.y, side);
    } else {
      const side: Side = anchor.y > newStockCy ? 'bottom' : 'top';
      stockEdge = getStockEdgePoint(anchor.x, newStockCy, side);
    }

    const newStockPoint = new Point({
      x: stockEdge.x,
      y: stockEdge.y,
      attachedToUid: stockEl.uid,
    });

    let newPoints: List<Point>;
    if (stockIsFirst) {
      newPoints = List([newStockPoint, anchor]);
    } else {
      newPoints = List([firstPoint, newStockPoint]);
    }

    // Preserve valve position by clamping to the new segment
    const currentValve: IPoint = { x: flow.cx, y: flow.cy };
    const newSegments = getSegments(newPoints);
    const closestSegment = findClosestSegment(currentValve, newSegments);
    const clampedValve = clampToSegment(currentValve, closestSegment);

    return flow.merge({
      x: clampedValve.x,
      y: clampedValve.y,
      points: newPoints,
    });
  }

  // For L-shaped flow, attach perpendicular to the original flow direction
  let attachmentSide: Side;
  if (originalFlowIsHorizontal) {
    // Original was horizontal, so the new segment from stock should be vertical
    attachmentSide = anchor.y < newStockCy ? 'top' : 'bottom';
  } else {
    // Original was vertical, so the new segment from stock should be horizontal
    attachmentSide = anchor.x < newStockCx ? 'left' : 'right';
  }
  const stockEdge = getStockEdgePoint(newStockCx, newStockCy, attachmentSide);

  // Corner connects the stock's perpendicular segment to the original flow direction
  let corner: IPoint;
  if (originalFlowIsHorizontal) {
    // Vertical segment from stock, then horizontal to anchor
    corner = { x: stockEdge.x, y: anchor.y };
  } else {
    // Horizontal segment from stock, then vertical to anchor
    corner = { x: anchor.x, y: stockEdge.y };
  }

  const newStockPoint = new Point({
    x: stockEdge.x,
    y: stockEdge.y,
    attachedToUid: stockEl.uid,
  });
  const cornerPoint = new Point({
    x: corner.x,
    y: corner.y,
    attachedToUid: undefined,
  });

  let newPoints: List<Point>;
  if (stockIsFirst) {
    newPoints = List([newStockPoint, cornerPoint, anchor]);
  } else {
    newPoints = List([firstPoint, cornerPoint, newStockPoint]);
  }

  // Preserve valve position by clamping to the closest segment of the new L-shape
  const currentValve: IPoint = { x: flow.cx, y: flow.cy };
  const newSegments = getSegments(newPoints);
  const closestSegment = findClosestSegment(currentValve, newSegments);
  const clampedValve = clampToSegment(currentValve, closestSegment);

  return flow.merge({
    x: clampedValve.x,
    y: clampedValve.y,
    points: newPoints,
  });
}

function adjustFlows(
  origStock: StockViewElement | CloudViewElement,
  stock: StockViewElement | CloudViewElement,
  flows: List<FlowViewElement>,
  isCloud?: boolean,
): List<FlowViewElement> {
  return flows.map((flow: FlowViewElement) => {
    let horizontal = isHorizontal(flow);
    const vertical = isVertical(flow);
    const inCreation = horizontal && vertical;

    let otherEnd: IPoint | undefined;
    const points = flow.points.map((point, i) => {
      // if its not the start or end point, don't change it.
      if (!(i === 0 || i === flow.points.size - 1)) {
        return point;
      }

      if (point.attachedToUid !== stock.uid) {
        otherEnd = point;
        return point;
      }

      let compare: IPoint;
      // we're inside a lambda passed to flow.points.map(), so while
      // first and last can conceptually return undefined, we know
      // that can't actually happen here.
      if (i === 0) {
        compare = flow.points.last() as IPoint;
      } else {
        compare = flow.points.first() as IPoint;
      }

      const d = {
        x: stock.cx - compare.x,
        y: stock.cy - compare.y,
      };

      if (inCreation) {
        horizontal = d.x > d.y;
      }

      const adjust = {
        x: StockWidth / 2,
        y: StockHeight / 2,
      };
      if (stock instanceof CloudViewElement || stock.isZeroRadius) {
        adjust.x = 0;
        adjust.y = 0;
      }

      if (horizontal && d.x < 0) {
        // right
        point = point.set('x', stock.cx + adjust.x);
      } else if (horizontal) {
        // left
        point = point.set('x', stock.cx - adjust.x);
      } else if (!horizontal && d.y < 0) {
        // bottom
        point = point.set('y', stock.cy + adjust.y);
      } else {
        // top
        point = point.set('y', stock.cy - adjust.y);
      }

      return point;
    });

    otherEnd = defined(otherEnd);

    // FIXME: reduce this duplication
    if (isCloud) {
      const fraction = {
        x: flow.cx === otherEnd.x ? 0.5 : (stock.cx - otherEnd.x) / (origStock.cx - otherEnd.x),
        y: flow.cy === otherEnd.y ? 0.5 : (stock.cy - otherEnd.y) / (origStock.cy - otherEnd.y),
      };
      const d = {
        x: flow.cx === otherEnd.x ? stock.cx - otherEnd.x : flow.cx - otherEnd.x,
        y: flow.cy === otherEnd.y ? stock.cy - otherEnd.y : flow.cy - otherEnd.y,
      };
      const base = {
        x: Math.min(otherEnd.x, stock.cx),
        y: Math.min(otherEnd.y, stock.cy),
      };
      flow = flow.merge({
        x: base.x + Math.abs(fraction.x * d.x),
        y: base.y + Math.abs(fraction.y * d.y),
      });
    } else {
      const fraction = {
        x: (stock.cx - otherEnd.x) / (origStock.cx - otherEnd.x || 1),
        y: (stock.cy - otherEnd.y) / (origStock.cy - otherEnd.y || 1),
      };
      const d = {
        x: flow.cx - otherEnd.x,
        y: flow.cy - otherEnd.y,
      };
      flow = flow.merge({
        x: otherEnd.x + fraction.x * d.x,
        y: otherEnd.y + fraction.y * d.y,
      });
    }

    return flow.set('points', points);
  });
}

export function UpdateStockAndFlows(
  stockEl: StockViewElement,
  flows: List<FlowViewElement>,
  moveDelta: IPoint,
): [StockViewElement, List<FlowViewElement>] {
  const newStockCx = stockEl.cx - moveDelta.x;
  const newStockCy = stockEl.cy - moveDelta.y;

  stockEl = stockEl.merge({
    x: newStockCx,
    y: newStockCy,
  });

  flows = flows.map((flow) => computeFlowRoute(flow, stockEl, newStockCx, newStockCy));

  return [stockEl, flows];
}

function allEqual<T>(extractor: (pt: Point) => T): (flow: FlowViewElement) => boolean {
  return (flow: FlowViewElement) => {
    if (flow.points.size === 0) {
      return false;
    }

    const first = extractor(defined(flow.points.get(0)));
    return flow.points.every((pt) => extractor(pt) === first);
  };
}

const isHorizontal = allEqual((pt) => pt.y);
const isVertical = allEqual((pt) => pt.x);

export function UpdateCloudAndFlow(
  cloud: StockViewElement | CloudViewElement,
  flow: FlowViewElement,
  moveDelta: IPoint,
): [StockViewElement | CloudViewElement, FlowViewElement] {
  let proposed = new Point({
    x: cloud.cx - moveDelta.x,
    y: cloud.cy - moveDelta.y,
    attachedToUid: undefined,
  });

  const start = defined(flow.points.get(0));

  if (isHorizontal(flow) && isVertical(flow)) {
    const d = {
      x: proposed.x - start.x,
      y: proposed.y - start.y,
    };
    // we're creating a new flow
    if (Math.abs(d.x) > Math.abs(d.y)) {
      // horizontal then.
      proposed = proposed.set('y', start.y);
    } else {
      proposed = proposed.set('x', start.x);
    }
  } else if (isHorizontal(flow)) {
    proposed = proposed.set('y', start.y);
  } else if (isVertical(flow)) {
    proposed = proposed.set('x', start.x);
  }

  const origCloud = cloud;
  cloud = cloud.merge({
    x: proposed.x,
    y: proposed.y,
  });

  flow = defined(adjustFlows(origCloud, cloud, List([flow]), true).first());

  return [cloud, flow];
}

interface Segment {
  index: number;
  p1: IPoint;
  p2: IPoint;
  isHorizontal: boolean;
  isVertical: boolean;
  isDiagonal: boolean;
}

// Exported for testing
export function getSegments(points: List<Point>): Segment[] {
  const segments: Segment[] = [];
  for (let i = 0; i < points.size - 1; i++) {
    const p1 = defined(points.get(i));
    const p2 = defined(points.get(i + 1));
    const isHorizontal = p1.y === p2.y;
    const isVertical = p1.x === p2.x;
    const isDiagonal = !isHorizontal && !isVertical;
    segments.push({
      index: i,
      p1: { x: p1.x, y: p1.y },
      p2: { x: p2.x, y: p2.y },
      isHorizontal,
      isVertical,
      isDiagonal,
    });
  }
  return segments;
}

// General point-to-line-segment distance using vector projection
function distanceToSegmentGeneral(point: IPoint, p1: IPoint, p2: IPoint): number {
  const dx = p2.x - p1.x;
  const dy = p2.y - p1.y;
  const lenSq = dx * dx + dy * dy;

  // Degenerate segment (single point)
  if (lenSq === 0) {
    return Math.hypot(point.x - p1.x, point.y - p1.y);
  }

  // Parameter t of the closest point on the infinite line
  let t = ((point.x - p1.x) * dx + (point.y - p1.y) * dy) / lenSq;
  // Clamp t to [0, 1] to stay on the segment
  t = Math.max(0, Math.min(1, t));

  // Closest point on segment
  const closestX = p1.x + t * dx;
  const closestY = p1.y + t * dy;

  return Math.hypot(point.x - closestX, point.y - closestY);
}

function distanceToSegment(point: IPoint, seg: Segment): number {
  const { p1, p2 } = seg;

  // For diagonal segments, use the general formula
  if (seg.isDiagonal) {
    return distanceToSegmentGeneral(point, p1, p2);
  }

  if (seg.isHorizontal) {
    const minX = Math.min(p1.x, p2.x);
    const maxX = Math.max(p1.x, p2.x);
    if (point.x >= minX && point.x <= maxX) {
      return Math.abs(point.y - p1.y);
    }
    const distToP1 = Math.hypot(point.x - p1.x, point.y - p1.y);
    const distToP2 = Math.hypot(point.x - p2.x, point.y - p2.y);
    return Math.min(distToP1, distToP2);
  } else {
    // Vertical segment
    const minY = Math.min(p1.y, p2.y);
    const maxY = Math.max(p1.y, p2.y);
    if (point.y >= minY && point.y <= maxY) {
      return Math.abs(point.x - p1.x);
    }
    const distToP1 = Math.hypot(point.x - p1.x, point.y - p1.y);
    const distToP2 = Math.hypot(point.x - p2.x, point.y - p2.y);
    return Math.min(distToP1, distToP2);
  }
}

function findClosestSegment(point: IPoint, segments: Segment[]): Segment {
  if (segments.length === 0) {
    throw new Error('findClosestSegment called with empty segments array');
  }
  let closest = segments[0];
  let minDist = distanceToSegment(point, closest);
  for (let i = 1; i < segments.length; i++) {
    const dist = distanceToSegment(point, segments[i]);
    if (dist < minDist) {
      minDist = dist;
      closest = segments[i];
    }
  }
  return closest;
}

function clampToSegment(point: IPoint, seg: Segment, margin: number = VALVE_CLAMP_MARGIN): IPoint {
  const { p1, p2 } = seg;

  // For diagonal segments, project onto the line and apply margin
  if (seg.isDiagonal) {
    const dx = p2.x - p1.x;
    const dy = p2.y - p1.y;
    const len = Math.hypot(dx, dy);

    if (len === 0) {
      return { x: p1.x, y: p1.y };
    }

    // Parameter t of the closest point on the infinite line
    let t = ((point.x - p1.x) * dx + (point.y - p1.y) * dy) / (len * len);

    // Clamp t to [margin/len, 1 - margin/len] to apply margin from endpoints
    const marginT = margin / len;
    t = Math.max(marginT, Math.min(1 - marginT, t));

    return {
      x: p1.x + t * dx,
      y: p1.y + t * dy,
    };
  }

  if (seg.isHorizontal) {
    const minX = Math.min(p1.x, p2.x) + margin;
    const maxX = Math.max(p1.x, p2.x) - margin;
    return {
      x: Math.max(minX, Math.min(maxX, point.x)),
      y: p1.y,
    };
  } else {
    // Vertical segment
    const minY = Math.min(p1.y, p2.y) + margin;
    const maxY = Math.max(p1.y, p2.y) - margin;
    return {
      x: p1.x,
      y: Math.max(minY, Math.min(maxY, point.y)),
    };
  }
}

const VALVE_RADIUS = 6;
const VALVE_HIT_TOLERANCE = 5;
// Margin from segment endpoints when clamping valve position
const VALVE_CLAMP_MARGIN = 10;

// Check if a segment has an attached endpoint that would prevent dragging.
// Dragging a segment with an attached endpoint would create a diagonal segment,
// which breaks the axis-alignment assumptions used by hit-testing and valve clamping.
function segmentHasAttachedEndpoint(points: List<Point>, segmentIndex: number): boolean {
  const numSegments = points.size - 1;
  if (segmentIndex < 0 || segmentIndex >= numSegments) {
    return true;
  }

  // Check both endpoints of the segment
  const p1 = points.get(segmentIndex);
  const p2 = points.get(segmentIndex + 1);

  return p1?.attachedToUid !== undefined || p2?.attachedToUid !== undefined;
}

// Determine which segment was clicked, or undefined if clicking on the valve
export function findClickedSegment(
  clickX: number,
  clickY: number,
  valveCx: number,
  valveCy: number,
  points: List<Point>,
): number | undefined {
  // If click is on/near the valve, return undefined (valve drag, not segment)
  const distToValve = Math.hypot(clickX - valveCx, clickY - valveCy);
  if (distToValve <= VALVE_RADIUS + VALVE_HIT_TOLERANCE) {
    return undefined;
  }

  const segments = getSegments(points);
  if (segments.length === 0) {
    return undefined;
  }

  // For single-segment flows (straight lines), clicking anywhere drags the valve
  if (segments.length === 1) {
    return undefined;
  }

  // For multi-segment flows, find closest segment
  const clickPoint: IPoint = { x: clickX, y: clickY };
  const closest = findClosestSegment(clickPoint, segments);

  // Don't allow dragging segments that have an attached endpoint
  // (would create diagonal segments which break axis-alignment assumptions)
  if (segmentHasAttachedEndpoint(points, closest.index)) {
    return undefined;
  }

  return closest.index;
}

// Move a segment perpendicular to its direction, adjusting adjacent segments
export function moveSegment(points: List<Point>, segmentIndex: number, delta: IPoint): List<Point> {
  const segments = getSegments(points);
  if (segmentIndex < 0 || segmentIndex >= segments.length) {
    return points;
  }

  const seg = segments[segmentIndex];
  const isFirst = segmentIndex === 0;
  const isLast = segmentIndex === segments.length - 1;

  return points.map((p, i) => {
    if (seg.isHorizontal) {
      // Horizontal segment: move up/down (change Y)
      // Both endpoints of this segment move
      if (i === segmentIndex || i === segmentIndex + 1) {
        // Don't move attached endpoints (first and last points)
        if ((i === 0 && isFirst) || (i === points.size - 1 && isLast)) {
          return p;
        }
        return p.set('y', p.y - delta.y);
      }
    } else {
      // Vertical segment: move left/right (change X)
      if (i === segmentIndex || i === segmentIndex + 1) {
        if ((i === 0 && isFirst) || (i === points.size - 1 && isLast)) {
          return p;
        }
        return p.set('x', p.x - delta.x);
      }
    }
    return p;
  });
}

export function UpdateFlow(
  flowEl: FlowViewElement,
  ends: List<StockViewElement | CloudViewElement>,
  moveDelta: IPoint,
  segmentIndex?: number,
): [FlowViewElement, List<CloudViewElement>] {
  const clouds = ends.filter((e) => e instanceof CloudViewElement);

  let points = flowEl.points;

  const currentValve: IPoint = { x: flowEl.cx, y: flowEl.cy };
  const proposedValve: IPoint = {
    x: currentValve.x - moveDelta.x,
    y: currentValve.y - moveDelta.y,
  };

  // For cloud-to-cloud flows, move everything uniformly
  const hasStock = ends.some((e) => e instanceof StockViewElement);
  if (!hasStock) {
    points = points.map((p) => p.merge({ x: p.x - moveDelta.x, y: p.y - moveDelta.y }));
    flowEl = flowEl.merge({
      x: proposedValve.x,
      y: proposedValve.y,
      points,
    });

    const updatedClouds = clouds.map((cloud) => {
      return cloud.merge({
        x: cloud.cx - moveDelta.x,
        y: cloud.cy - moveDelta.y,
      }) as CloudViewElement;
    });

    return [flowEl, updatedClouds];
  }

  const segments = getSegments(points);

  // If a specific segment is being moved, move that segment.
  // Note: We return an empty clouds list because segment movement only affects
  // interior points (corners), not attached endpoints. Attached endpoints stay
  // fixed at their stock/cloud positions. Only draggable segments are interior
  // segments (between two corners), so no cloud positions need updating.
  if (segmentIndex !== undefined) {
    points = moveSegment(points, segmentIndex, moveDelta);

    // Always re-clamp the valve to the closest segment after any segment drag.
    // Dragging any segment can affect adjacent segments via shared corners,
    // so the valve's segment may have changed shape even if it wasn't the
    // segment being dragged.
    const newSegments = getSegments(points);
    const closestSeg = findClosestSegment(currentValve, newSegments);
    const newValve = clampToSegment(currentValve, closestSeg);
    flowEl = flowEl.merge({
      x: newValve.x,
      y: newValve.y,
      points,
    });

    return [flowEl, List<CloudViewElement>()];
  }

  // Moving the valve along the flow path.
  // Note: Valve movement doesn't change any endpoint positions, so no cloud
  // positions need updating. We return an empty clouds list.
  const closestSegment = findClosestSegment(currentValve, segments);
  const clampedValve = clampToSegment(proposedValve, closestSegment);

  flowEl = flowEl.merge({
    x: clampedValve.x,
    y: clampedValve.y,
  });

  return [flowEl, List<CloudViewElement>()];
}

export function flowBounds(element: FlowViewElement): Rect {
  const { cx, cy } = element;
  // Flow valve is a circle with radius 6 (FlowWidth/2 = 12/2 = 6)
  const r = 6;
  const bounds = {
    top: cy - r,
    left: cx - r,
    right: cx + r,
    bottom: cy + r,
  };

  // Include label bounds if there's a label
  if (element.name) {
    const side = element.labelSide;
    const labelProps: LabelProps = {
      cx,
      cy,
      side,
      rw: r,
      rh: r,
      text: displayName(element.name),
    };
    const lBounds = labelBounds(labelProps);

    bounds.top = Math.min(bounds.top, lBounds.top);
    bounds.left = Math.min(bounds.left, lBounds.left);
    bounds.right = Math.max(bounds.right, lBounds.right);
    bounds.bottom = Math.max(bounds.bottom, lBounds.bottom);
  }

  // Also include flow path points
  if (element.points) {
    for (const point of element.points) {
      bounds.left = Math.min(bounds.left, point.x);
      bounds.right = Math.max(bounds.right, point.x);
      bounds.top = Math.min(bounds.top, point.y);
      bounds.bottom = Math.max(bounds.bottom, point.y);
    }
  }

  return bounds;
}

export interface FlowProps {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  isMovingArrow: boolean;
  hasWarning?: boolean;
  series: Readonly<Array<Series>> | undefined;
  onSelection: (
    el: ViewElement,
    e: React.PointerEvent<SVGElement>,
    isText?: boolean,
    isArrowhead?: boolean,
    segmentIndex?: number,
  ) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  source: StockViewElement | CloudViewElement;
  element: FlowViewElement;
  sink: StockViewElement | CloudViewElement;
}

export class Flow extends React.PureComponent<FlowProps> {
  handlePointerUp = (_e: React.PointerEvent<SVGElement>): void => {
    // e.preventDefault();
    // e.stopPropagation();
  };

  handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();

    // Convert screen coordinates to model coordinates using the element's CTM.
    // We must use the clicked element's CTM (not the SVG root's CTM) because
    // Canvas applies zoom/pan via a parent <g transform="matrix(...)"> group.
    // The element's CTM includes this transform, so inverting it correctly
    // converts screen coordinates to model coordinates regardless of zoom/pan.
    const target = e.currentTarget as SVGGraphicsElement;
    const svg = target.ownerSVGElement;
    let segmentIndex: number | undefined;

    if (svg) {
      const pt = svg.createSVGPoint();
      pt.x = e.clientX;
      pt.y = e.clientY;
      const ctm = target.getScreenCTM();
      if (ctm) {
        const modelPt = pt.matrixTransform(ctm.inverse());
        segmentIndex = findClickedSegment(
          modelPt.x,
          modelPt.y,
          this.props.element.cx,
          this.props.element.cy,
          this.props.element.points,
        );
      }
    }

    this.props.onSelection(this.props.element, e, false, false, segmentIndex);
  };

  handleLabelSelection = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    this.props.onSelection(this.props.element, e, true);
  };

  handlePointerDownArrowhead = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    this.props.onSelection(this.props.element, e, false, true);
  };

  radius(): number {
    return AuxRadius;
  }

  indicators() {
    if (!this.props.hasWarning) {
      return undefined;
    }

    const { element } = this.props;
    const r = this.radius();
    const θ = -Math.PI / 4; // 45 degrees

    const cx = element.cx + r * Math.cos(θ);
    const cy = element.cy + r * Math.sin(θ);

    return <circle className={styles.errorIndicator} cx={cx} cy={cy} r={3} />;
  }

  sparkline(series: Readonly<Array<Series>> | undefined) {
    if (!series || series.length === 0) {
      return undefined;
    }
    const { element } = this.props;
    const isArrayed = element.var?.isArrayed || false;
    const arrayedOffset = isArrayed ? 3 : 0;
    const cx = element.cx - arrayedOffset;
    const cy = element.cy - arrayedOffset;
    const r = this.radius();

    return (
      <g transform={`translate(${cx + 1 - r / 2} ${cy + 1 - r / 2})`}>
        <Sparkline series={series} width={r - 2} height={r - 2} />
      </g>
    );
  }

  render() {
    const { element, isEditingName, isMovingArrow, isSelected, isValidTarget, series, sink } = this.props;

    const isArrayed = element.var?.isArrayed || false;
    const arrayedOffset = isArrayed ? 3 : 0;

    let pts = this.props.element.points;
    if (pts.size < 2) {
      throw new Error('expected at least two points on a flow');
    }

    if (sink instanceof CloudViewElement && !isMovingArrow) {
      const x = defined(pts.get(pts.size - 1)).x;
      const y = defined(pts.get(pts.size - 1)).y;
      const prevX = defined(pts.get(pts.size - 2)).x;
      const prevY = defined(pts.get(pts.size - 2)).y;

      if (prevX < x) {
        pts = pts.update(pts.size - 1, (pt) => defined(pt).set('x', x - CloudRadius));
      } else if (prevX > x) {
        pts = pts.update(pts.size - 1, (pt) => defined(pt).set('x', x + CloudRadius));
      }
      if (prevY < y) {
        pts = pts.update(pts.size - 1, (pt) => defined(pt).set('y', y - CloudRadius));
      } else if (prevY > y) {
        pts = pts.update(pts.size - 1, (pt) => defined(pt).set('y', y + CloudRadius));
      }
    }

    const finalAdjust = 7.5;
    let spath = '';
    let arrowθ = 0;
    for (let j = 0; j < pts.size; j++) {
      let x = defined(pts.get(j)).x;
      let y = defined(pts.get(j)).y;
      if (j === pts.size - 1) {
        const dx = x - defined(pts.get(j - 1)).x;
        const dy = y - defined(pts.get(j - 1)).y;
        let θ = (atan2(dy, dx) * 180) / PI;
        if (θ < 0) {
          θ += 360;
        }
        if (θ >= 315 || θ < 45) {
          x -= finalAdjust;
          arrowθ = 0;
        } else if (θ >= 45 && θ < 135) {
          y -= finalAdjust;
          arrowθ = 90;
        } else if (θ >= 135 && θ < 225) {
          x += finalAdjust;
          arrowθ = 180;
        } else {
          y += finalAdjust;
          arrowθ = 270;
        }
      }
      const prefix = j === 0 ? 'M' : 'L';
      spath += `${prefix}${x},${y}`;
    }

    const cx = element.cx;
    const cy = element.cy;
    const r = this.radius();

    const lastPt = defined(pts.get(pts.size - 1));
    const side = element.labelSide;
    const label = isEditingName ? undefined : (
      <Label
        uid={element.uid}
        cx={cx}
        cy={cy}
        side={side}
        rw={r + arrayedOffset}
        rh={r + arrayedOffset}
        text={displayName(defined(element.name))}
        onSelection={this.handleLabelSelection}
        onLabelDrag={this.props.onLabelDrag}
      />
    );

    const sparkline = this.sparkline(series);
    const indicator = this.indicators();

    const groupClassName = clsx(styles.flow, 'simlin-flow', {
      [styles.selected]: isSelected && isValidTarget === undefined,
      'simlin-selected': isSelected && isValidTarget === undefined,
      [styles.targetGood]: isValidTarget === true,
      [styles.targetBad]: isValidTarget === false,
    });

    let circles = [<circle key="1" cx={cx} cy={cy} r={r} />];
    if (isArrayed) {
      circles = [
        <circle key="0" cx={cx + arrayedOffset} cy={cy + arrayedOffset} r={r} />,
        <circle key="1" cx={cx} cy={cy} r={r} />,
        <circle key="2" cx={cx - arrayedOffset} cy={cy - arrayedOffset} r={r} />,
      ];
    }

    const outerClassName = isSelected
      ? clsx(styles.outerSelected, 'simlin-outer-selected')
      : clsx(styles.outer, 'simlin-outer');

    return (
      <g className={groupClassName}>
        <path
          d={spath}
          className={outerClassName}
          onPointerDown={this.handlePointerDown}
          onPointerUp={this.handlePointerUp}
        />
        <Arrowhead
          point={lastPt}
          angle={arrowθ}
          size={FlowArrowheadRadius}
          type="flow"
          isSelected={this.props.isSelected}
          onSelection={this.handlePointerDownArrowhead}
        />
        <path d={spath} className={clsx(styles.inner, 'simlin-inner')} />
        <g onPointerDown={this.handlePointerDown} onPointerUp={this.handlePointerUp}>
          {circles}
          {sparkline}
        </g>
        {indicator}
        {label}
      </g>
    );
  }
}
