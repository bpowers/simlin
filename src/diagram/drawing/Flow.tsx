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

    const midX = (newStockPoint.x + anchor.x) / 2;
    const midY = (newStockPoint.y + anchor.y) / 2;

    return flow.merge({
      x: midX,
      y: midY,
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

  const valveMidX = (anchor.x + corner.x) / 2;
  const valveMidY = (anchor.y + corner.y) / 2;

  return flow.merge({
    x: valveMidX,
    y: valveMidY,
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

export function UpdateFlow(
  flowEl: FlowViewElement,
  ends: List<StockViewElement | CloudViewElement>,
  moveDelta: IPoint,
): [FlowViewElement, List<CloudViewElement>] {
  const stocks = ends.filter((e) => e instanceof StockViewElement);
  const clouds = ends.filter((e) => e instanceof CloudViewElement);

  const center = new Point({
    x: flowEl.cx,
    y: flowEl.cy,
    attachedToUid: undefined,
  });

  let points = flowEl.points;
  const origPoints = points;
  const start = defined(points.get(0));
  const end = defined(points.get(points.size - 1));

  let proposed = new Point({
    x: center.x - moveDelta.x,
    y: center.y - moveDelta.y,
    attachedToUid: undefined,
  });

  // if we don't have any stocks, its a flow from cloud to cloud and as such
  // doesn't need to be constrained.

  // vertical line
  if (center.x === start.x && center.x === end.x && stocks.size > 0) {
    proposed = stocks.reduce((p, stock: ViewElement) => {
      let x = p.x;
      x = Math.max(x, stock.cx - StockWidth / 2 + 3);
      x = Math.min(x, stock.cx + StockWidth / 2 - 3);
      return p.set('x', x);
    }, proposed);

    const minY = points.reduce((m, p) => (p.y < m ? p.y : m), Infinity) + 20;
    const maxY = points.reduce((m, p) => (p.y > m ? p.y : m), -Infinity) - 20;
    const y = Math.max(minY, Math.min(maxY, proposed.y));
    proposed = proposed.set('y', y);

    points = points.map((p) => p.set('x', proposed.x));
  } else if (center.y === start.y && center.y === end.y && stocks.size > 0) {
    proposed = stocks.reduce((p, stock: ViewElement) => {
      let y = p.y;
      y = Math.max(y, stock.cy - StockHeight / 2 + 3);
      y = Math.min(y, stock.cy + StockHeight / 2 - 3);
      return p.set('y', y);
    }, proposed);

    const minX = points.reduce((m, p) => (p.x < m ? p.x : m), Infinity) + 20;
    const maxX = points.reduce((m, p) => (p.x > m ? p.x : m), -Infinity) - 20;
    const x = Math.max(minX, Math.min(maxX, proposed.x));
    proposed = proposed.set('x', x);

    points = points.map((p) => p.set('y', proposed.y));
  } else if (stocks.size === 0) {
    // if it is a cloud -> cloud flow, move all points uniformly
    points = points.map((p) => p.merge({ x: p.x - moveDelta.x, y: p.y - moveDelta.y }));
  } else {
    console.log('TODO: unknown constraint?');
  }

  const updatedClouds = clouds.map((cloud) => {
    const origPoint = defined(origPoints.find((pt) => pt.attachedToUid === cloud.uid));
    const updatedPoint = defined(points.find((pt) => pt.attachedToUid === cloud.uid));
    const delta = {
      x: updatedPoint.x - origPoint.x,
      y: updatedPoint.y - origPoint.y,
    };

    return cloud.merge({
      x: cloud.cx + delta.x,
      y: cloud.cy + delta.y,
    }) as CloudViewElement;
  });

  flowEl = flowEl.merge({
    x: proposed.x,
    y: proposed.y,
    points,
  });
  return [flowEl, updatedClouds];
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
  onSelection: (el: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean, isArrowhead?: boolean) => void;
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
    this.props.onSelection(this.props.element, e);
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
