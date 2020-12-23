// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { Point, FlowViewElement, ViewElement, StockViewElement, CloudViewElement } from '../../datamodel';

import { Arrowhead } from './Arrowhead';
import { displayName, Point as IPoint } from './common';
import { AuxRadius, CloudRadius, FlowArrowheadRadius } from './default';
import { Label } from './Label';
import { Sparkline } from './Sparkline';
import { StockHeight, StockWidth } from './Stock';

import { defined, Series } from '../../common';

const styles = createStyles({
  flowOuter: {
    fill: 'none',
    strokeWidth: 4,
    stroke: 'black',
  },
  flowOuterSelected: {
    fill: 'none',
    strokeWidth: 4,
    stroke: '#4444dd',
  },
  flowInner: {
    fill: 'none',
    strokeWidth: 2,
    stroke: 'white',
  },
  aux: {
    fill: 'white',
    strokeWidth: 1,
    stroke: 'black',
  },
  targetGood: {
    '& circle': {
      stroke: 'rgb(76, 175, 80)',
      strokeWidth: 2,
    },
  },
  targetBad: {
    '& circle': {
      stroke: 'rgb(244, 67, 54)',
      strokeWidth: 2,
    },
  },
  selected: {
    '& text': {
      fill: '#4444dd',
    },
    '& circle': {
      stroke: '#4444dd',
    },
  },
  indicator: {
    fill: 'rgb(255, 152, 0)',
    strokeWidth: 0,
  },
});

const atan2 = Math.atan2;
const PI = Math.PI;

// similar to Python's isclose, which is Apache 2 licensed
function eq(a: number, b: number, relTol = 1e-9, absTol = 0.0): boolean {
  if (relTol < 0 || absTol < 0) {
    throw new Error(`relative and absolute tolerances must be non-negative.`);
  }
  if (a === b) {
    return true;
  }

  const diff = Math.abs(b - a);
  return diff <= Math.abs(relTol * b) || diff <= Math.abs(relTol * a) || diff <= absTol;
}

function isAdjacent(
  stockEl: StockViewElement,
  flow: FlowViewElement,
  side: 'left' | 'right' | 'top' | 'bottom',
): boolean {
  // want to look at first point and last point.
  const points = flow.points.filter((_, i) => {
    return i === 0 || i === flow.points.size - 1;
  });

  for (const point of points) {
    if (side === 'left' && eq(point.x, stockEl.cx - StockWidth / 2)) {
      return true;
    } else if (side === 'right' && eq(point.x, stockEl.cx + StockWidth / 2)) {
      return true;
    } else if (side === 'top' && eq(point.y, stockEl.cy - StockHeight / 2)) {
      return true;
    } else if (side === 'bottom' && eq(point.y, stockEl.cy + StockHeight / 2)) {
      return true;
    }
  }

  return false;
}

function getComparePoint(flow: FlowViewElement, _stock: ViewElement): IPoint {
  if (flow.points.size !== 2) {
    console.log(`TODO: multipoint flows for ${flow.ident()}`);
  }
  return {
    x: flow.cx,
    y: flow.cy,
  };
}

function adjustFlows(
  origStock: StockViewElement | CloudViewElement,
  stock: StockViewElement | CloudViewElement,
  flows: List<FlowViewElement>,
  isCloud?: boolean,
): List<FlowViewElement> {
  let otherEnd: IPoint | undefined;
  return flows.map((flow: FlowViewElement) => {
    const points = flow.points.map((point, i) => {
      // if its not the start or end point, don't change it.
      if (!(i === 0 || i === flow.points.size - 1)) {
        return point;
      }

      if (point.attachedToUid !== stock.uid) {
        otherEnd = point;
        return point;
      }

      const compare = getComparePoint(flow, stock);
      const d = {
        x: stock.cx - compare.x,
        y: stock.cy - compare.y,
      };

      const θ = (Math.atan2(d.x, d.y) * 180) / Math.PI;

      const adjust = {
        x: StockWidth / 2,
        y: StockHeight / 2,
      };
      if (stock instanceof CloudViewElement || stock.isZeroRadius) {
        adjust.x = 0;
        adjust.y = 0;
      }

      if (-45 <= θ && θ < 45) {
        // top
        point = point.set('y', stock.cy - adjust.y);
      } else if (45 <= θ && θ < 135) {
        // left
        point = point.set('x', stock.cx - adjust.x);
      } else if (135 <= θ || θ < -135) {
        // bottom
        point = point.set('y', stock.cy + adjust.y);
      } else if (-135 <= θ && θ < -45) {
        // right
        point = point.set('x', stock.cx + adjust.x);
      } else {
        throw new Error(`unreachable, θ=${θ}`);
      }

      return point;
    });

    // FIXME: reduce this duplication
    if (otherEnd && isCloud) {
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
    } else if (otherEnd) {
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
  const left = flows.filter((e) => isAdjacent(stockEl, e, 'left'));
  const right = flows.filter((e) => isAdjacent(stockEl, e, 'right'));
  const top = flows.filter((e) => isAdjacent(stockEl, e, 'top'));
  const bottom = flows.filter((e) => isAdjacent(stockEl, e, 'bottom'));

  let proposed = new Point({
    x: stockEl.cx - moveDelta.x,
    y: stockEl.cy - moveDelta.y,
    attachedToUid: undefined,
  });

  proposed = left.concat(right).reduce((p, flowEl: FlowViewElement) => {
    let y = p.y;
    y = Math.max(y, flowEl.cy - StockHeight / 2 + 3);
    y = Math.min(y, flowEl.cy + StockHeight / 2 - 3);
    return p.set('y', y);
  }, proposed);

  proposed = top.concat(bottom).reduce((p, flowEl: FlowViewElement) => {
    let x = p.x;
    x = Math.max(x, flowEl.cx - StockWidth / 2 + 3);
    x = Math.min(x, flowEl.cx + StockWidth / 2 - 3);
    return p.set('x', x);
  }, proposed);

  const origStockEl = stockEl;
  stockEl = stockEl.merge({
    x: proposed.x,
    y: proposed.y,
  });

  flows = adjustFlows(origStockEl, stockEl, flows);

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

export interface FlowPropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  isMovingArrow: boolean;
  hasWarning?: boolean;
  series: Series | undefined;
  onSelection: (el: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean, isArrowhead?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  source: StockViewElement | CloudViewElement;
  element: FlowViewElement;
  sink: StockViewElement | CloudViewElement;
}

export type FlowProps = Pick<
  FlowPropsFull,
  | 'isSelected'
  | 'isMovingArrow'
  | 'isEditingName'
  | 'isValidTarget'
  | 'hasWarning'
  | 'series'
  | 'onSelection'
  | 'onLabelDrag'
  | 'source'
  | 'element'
  | 'sink'
>;

export const Flow = withStyles(styles)(
  class Flow extends React.PureComponent<FlowPropsFull> {
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

      const { classes, element } = this.props;
      const r = this.radius();
      const θ = -Math.PI / 4; // 45 degrees

      const cx = element.cx + r * Math.cos(θ);
      const cy = element.cy + r * Math.sin(θ);

      return <circle className={classes.indicator} cx={cx} cy={cy} r={3} />;
    }

    sparkline(series: Series | undefined) {
      if (!series) {
        return undefined;
      }
      const { element } = this.props;
      const cx = element.cx;
      const cy = element.cy;
      const r = this.radius();

      return (
        <g transform={`translate(${cx + 1 - r / 2} ${cy + 1 - r / 2})`}>
          <Sparkline series={series} width={r - 2} height={r - 2} />
        </g>
      );
    }

    render() {
      const { classes, element, isEditingName, isMovingArrow, isSelected, isValidTarget, series, sink } = this.props;

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
          pts = pts.update(pts.size - 1, (pt) => pt.set('x', x - CloudRadius));
        } else if (prevX > x) {
          pts = pts.update(pts.size - 1, (pt) => pt.set('x', x + CloudRadius));
        }
        if (prevY < y) {
          pts = pts.update(pts.size - 1, (pt) => pt.set('y', y - CloudRadius));
        } else if (prevY > y) {
          pts = pts.update(pts.size - 1, (pt) => pt.set('y', y + CloudRadius));
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
          text={displayName(defined(element.name))}
          onSelection={this.handleLabelSelection}
          onLabelDrag={this.props.onLabelDrag}
        />
      );

      const sparkline = this.sparkline(series);
      const indicator = this.indicators();

      let groupClassName = isSelected ? classes.selected : undefined;
      if (isValidTarget !== undefined) {
        groupClassName = isValidTarget ? classes.targetGood : classes.targetBad;
      }

      return (
        <g className={groupClassName}>
          <path
            d={spath}
            className={isSelected ? classes.flowOuterSelected : classes.flowOuter}
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
          <path d={spath} className={classes.flowInner} />
          <g onPointerDown={this.handlePointerDown} onPointerUp={this.handlePointerUp}>
            <circle className={classes.aux} cx={cx} cy={cy} r={r} />
            {sparkline}
          </g>
          {indicator}
          {label}
        </g>
      );
    }
  },
);
