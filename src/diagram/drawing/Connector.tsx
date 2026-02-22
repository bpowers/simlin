// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import {
  isNamedViewElement,
  LinkViewElement,
  variableIsArrayed,
  ViewElement,
} from '@simlin/core/datamodel';

import { Arrowhead } from './Arrowhead';
import { Circle, isInf, isZero, Point, Rect, square } from './common';
import { ArrowheadRadius, AuxRadius, StockWidth, StockHeight, ModuleWidth, ModuleHeight, StraightLineMax } from './default';
import styles from './Connector.module.css';

export const ArrayedOffset = 3;

function isElementArrayed(element: ViewElement): boolean {
  if (isNamedViewElement(element) && element.var) {
    return variableIsArrayed(element.var);
  }
  return false;
}

export function getVisualCenter(element: ViewElement): { cx: number; cy: number } {
  // Zero-radius elements are temporary placeholders (used during drag operations)
  // that should stay anchored to the cursor position
  if (element.isZeroRadius) {
    return { cx: element.x, cy: element.y };
  }
  const offset = isElementArrayed(element) ? ArrayedOffset : 0;
  return {
    cx: element.x - offset,
    cy: element.y - offset,
  };
}

// math functions we care about
const atan2 = Math.atan2;
const sin = Math.sin;
const cos = Math.cos;
const tan = Math.tan;
const PI = Math.PI;
const sqrt = Math.sqrt;

const degToRad = (d: number): number => {
  return (d / 180) * PI;
};

const radToDeg = (r: number): number => {
  return (r * 180) / PI;
};

export function rayRectIntersection(cx: number, cy: number, hw: number, hh: number, θ: number): Point {
  const cosT = cos(θ);
  const sinT = sin(θ);

  let t: number;
  if (isZero(cosT)) {
    t = hh / Math.abs(sinT);
  } else if (isZero(sinT)) {
    t = hw / Math.abs(cosT);
  } else {
    const tX = hw / Math.abs(cosT);
    const tY = hh / Math.abs(sinT);
    t = Math.min(tX, tY);
  }

  return {
    x: cx + t * cosT,
    y: cy + t * sinT,
  };
}

export function circleRectIntersections(circ: Circle, cx: number, cy: number, hw: number, hh: number): Point[] {
  const eps = 1e-9;
  const points: Point[] = [];

  // Horizontal edges (y = cy +/- hh)
  for (const yEdge of [cy - hh, cy + hh]) {
    const dy = yEdge - circ.y;
    const disc = circ.r * circ.r - dy * dy;
    if (disc >= 0) {
      const sqrtDisc = sqrt(disc);
      for (const x of [circ.x + sqrtDisc, circ.x - sqrtDisc]) {
        if (x >= cx - hw - eps && x <= cx + hw + eps) {
          points.push({ x, y: yEdge });
        }
      }
    }
  }

  // Vertical edges (x = cx +/- hw)
  for (const xEdge of [cx - hw, cx + hw]) {
    const dx = xEdge - circ.x;
    const disc = circ.r * circ.r - dx * dx;
    if (disc >= 0) {
      const sqrtDisc = sqrt(disc);
      for (const y of [circ.y + sqrtDisc, circ.y - sqrtDisc]) {
        if (y >= cy - hh - eps && y <= cy + hh + eps) {
          const isDup = points.some((p) => Math.abs(p.x - xEdge) < eps && Math.abs(p.y - y) < eps);
          if (!isDup) {
            points.push({ x: xEdge, y });
          }
        }
      }
    }
  }

  return points;
}

export function takeoffθ(props: Pick<ConnectorProps, 'element' | 'from' | 'to' | 'arcPoint'>): number {
  const { element, from, to, arcPoint } = props;

  const fromVisual = getVisualCenter(from);
  const toVisual = getVisualCenter(to);

  if (arcPoint && !(toVisual.cx === arcPoint.x && toVisual.cy === arcPoint.y)) {
    // special case moving
    // if (toVisual.cx === arcPoint.x && toVisual.cy === arcPoint.y) {
    //   if (element.angle) {
    //     return degToRad(xmileToCanvasAngle(element.angle));
    //   } else {
    //     // its a straight line
    //     return atan2(toVisual.cy - fromVisual.cy, toVisual.cx - fromVisual.cx);
    //   }
    // }
    const circ = circleFromPoints({ x: fromVisual.cx, y: fromVisual.cy }, { x: toVisual.cx, y: toVisual.cy }, arcPoint);
    const fromθ = Math.atan2(fromVisual.cy - circ.y, fromVisual.cx - circ.x);
    const toθ = atan2(toVisual.cy - circ.y, toVisual.cx - circ.x);
    let spanθ = toθ - fromθ;
    if (spanθ > degToRad(180)) {
      spanθ -= degToRad(360);
    }

    const inv: boolean = spanθ > 0 || spanθ <= degToRad(-180);

    // check if the user's cursor and the center of the arc's circle are on the same
    // side of the straight line connecting the from and to elements.  If so, we need
    // to set the sweep flag.
    const side1 =
      (circ.x - fromVisual.cx) * (toVisual.cy - fromVisual.cy) -
      (circ.y - fromVisual.cy) * (toVisual.cx - fromVisual.cx);
    const side2 =
      (arcPoint.x - fromVisual.cx) * (toVisual.cy - fromVisual.cy) -
      (arcPoint.y - fromVisual.cy) * (toVisual.cx - fromVisual.cx);

    const sweep = side1 < 0 === side2 < 0;

    return fromθ + degToRad(((sweep && inv) || (!sweep && !inv) ? -1 : 1) * 90);
  }
  if (element.arc !== undefined) {
    // convert from counter-clockwise (XMILE) to
    // clockwise (display, where Y grows down
    // instead of up)
    return degToRad(element.arc);
  } else {
    console.log(`connector from ${element.fromUid || ''} doesn't have x, y, or angle`);
    return NaN;
  }
}

export function intersectElementArc(element: ViewElement, circ: Circle, inv: boolean): Point {
  const { cx, cy } = getVisualCenter(element);

  if (element.type === 'stock' || element.type === 'module') {
    const hw = element.type === 'stock' ? StockWidth / 2 : ModuleWidth / 2;
    const hh = element.type === 'stock' ? StockHeight / 2 : ModuleHeight / 2;

    const intersections = circleRectIntersections(circ, cx, cy, hw, hh);
    if (intersections.length === 0) {
      const dir = atan2(cy - circ.y, cx - circ.x);
      return rayRectIntersection(cx, cy, hw, hh, dir);
    }

    // Use a reference point on the arc to pick the correct intersection.
    // atan (not tan) gives a monotonic, bounded angular offset that avoids
    // discontinuities when rApprox/circ.r crosses tan's asymptotes.
    const rApprox = Math.max(hw, hh);
    const offθ = Math.atan(rApprox / circ.r);
    const elementCenterθ = atan2(cy - circ.y, cx - circ.x);
    const targetθ = elementCenterθ + (inv ? 1 : -1) * offθ;
    const target: Point = {
      x: circ.x + circ.r * cos(targetθ),
      y: circ.y + circ.r * sin(targetθ),
    };

    let best = intersections[0];
    let bestDist = square(best.x - target.x) + square(best.y - target.y);
    for (let i = 1; i < intersections.length; i++) {
      const d = square(intersections[i].x - target.x) + square(intersections[i].y - target.y);
      if (d < bestDist) {
        best = intersections[i];
        bestDist = d;
      }
    }
    return best;
  }

  let r: number = AuxRadius;
  if (element.isZeroRadius) {
    r = 0;
  }

  const offθ = tan(r / circ.r);
  const elementCenterθ = atan2(cy - circ.y, cx - circ.x);

  return {
    x: circ.x + circ.r * cos(elementCenterθ + (inv ? 1 : -1) * offθ),
    y: circ.y + circ.r * sin(elementCenterθ + (inv ? 1 : -1) * offθ),
  };
}

// operates on angles in radian form.  adds 180 degrees, if we're now
// outside of our domain, clamp back into it.
const oppositeθ = (θ: number): number => {
  θ += PI;
  if (θ > PI) {
    θ -= 2 * PI;
  }
  return θ;
};

export function circleFromPoints(p1: Point, p2: Point, p3: Point): Circle {
  const off = square(p2.x) + square(p2.y);
  const bc = (square(p1.x) + square(p1.y) - off) / 2;
  const cd = (off - square(p3.x) - square(p3.y)) / 2;
  const det = (p1.x - p2.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p2.y);
  if (isZero(det)) {
    throw new Error('zero determinant');
  }
  const idet = 1 / det;
  const cx = (bc * (p2.y - p3.y) - cd * (p1.y - p2.y)) * idet;
  const cy = (cd * (p1.x - p2.x) - bc * (p2.x - p3.x)) * idet;
  return {
    x: cx,
    y: cy,
    r: sqrt(square(p2.x - cx) + square(p2.y - cy)),
  };
}

export interface ConnectorProps {
  isSelected: boolean;
  isDashed?: boolean;
  from: ViewElement;
  element: LinkViewElement;
  to: ViewElement;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isArrowhead: boolean) => void;
  arcPoint?: Point;
}

export class Connector extends React.PureComponent<ConnectorProps> {
  handlePointerDownArc = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    this.props.onSelection(this.props.element, e, false);
  };

  handlePointerDownArrowhead = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    this.props.onSelection(this.props.element, e, true);
  };

  static intersectElementStraight(element: ViewElement, θ: number): Point {
    const { cx, cy } = getVisualCenter(element);

    if (element.type === 'stock') {
      return rayRectIntersection(cx, cy, StockWidth / 2, StockHeight / 2, θ);
    } else if (element.type === 'module') {
      return rayRectIntersection(cx, cy, ModuleWidth / 2, ModuleHeight / 2, θ);
    }

    let r: number = AuxRadius;
    if (element.isZeroRadius) {
      r = 0;
    }

    return {
      x: cx + r * cos(θ),
      y: cy + r * sin(θ),
    };
  }

  static isStraightLine(props: ConnectorProps): boolean {
    const { element, arcPoint, from, to } = props;

    // If there's no arc defined and no arcPoint, draw a straight line
    if (element.arc === undefined && !arcPoint) {
      return true;
    }

    const takeoffAngle = takeoffθ(props);
    const fromVisual = getVisualCenter(from);
    const toVisual = getVisualCenter(to);
    const midθ = atan2(toVisual.cy - fromVisual.cy, toVisual.cx - fromVisual.cx);

    return Math.abs(midθ - takeoffAngle) < degToRad(StraightLineMax);
  }

  renderStraightLine() {
    const { from, to, isSelected, isDashed } = this.props;

    const fromVisual = getVisualCenter(from);
    const toVisual = getVisualCenter(to);
    const θ = atan2(toVisual.cy - fromVisual.cy, toVisual.cx - fromVisual.cx);
    const start = Connector.intersectElementStraight(from, θ);
    const end = Connector.intersectElementStraight(to, oppositeθ(θ));

    const arrowθ = radToDeg(θ);
    const path = `M${start.x},${start.y}L${end.x},${end.y}`;

    let connectorClass = isSelected
      ? `${styles.connectorSelected} simlin-connector simlin-connector-selected`
      : `${styles.connector} simlin-connector`;
    if (isDashed && !isSelected) {
      connectorClass = `${styles.connectorDashed} simlin-connector simlin-connector-dashed`;
    }

    return (
      <g key={this.props.element.uid}>
        <path
          d={path}
          className={`${styles.connectorBg} simlin-connector-bg`}
          onPointerDown={this.handlePointerDownArc}
        />
        <path d={path} className={connectorClass} onPointerDown={this.handlePointerDownArc} />
        <Arrowhead
          point={end}
          angle={arrowθ}
          isSelected={isSelected}
          size={ArrowheadRadius}
          onSelection={this.handlePointerDownArrowhead}
          type="connector"
        />
      </g>
    );
  }

  private static arcCircle(props: ConnectorProps): Circle | undefined {
    const { from, to, arcPoint } = props;

    const fromVisual = getVisualCenter(from);
    const toVisual = getVisualCenter(to);

    if (arcPoint && !(toVisual.cx === arcPoint.x && toVisual.cy === arcPoint.y)) {
      return circleFromPoints({ x: fromVisual.cx, y: fromVisual.cy }, { x: toVisual.cx, y: toVisual.cy }, arcPoint);
    }

    // Find cx, cy from 'takeoff angle', and center of
    // 'from', and center of 'to'.  This means we have 2
    // points on the edge of a cirlce, and the tangent at
    // point 1.
    //
    //     eqn of a circle: (x - cx)^2 + (y - cy)^2 = r^2
    //     line:            y = mx + b || 0 = mx - y + b

    const slopeTakeoff = tan(takeoffθ(props));
    // we need the slope of the line _perpendicular_ to
    // the tangent in order to find out the x,y center of
    // our circle
    let slopePerpToTakeoff = -1 / slopeTakeoff;
    if (isZero(slopePerpToTakeoff)) {
      slopePerpToTakeoff = 0;
    } else if (isInf(slopePerpToTakeoff)) {
      // we are either on the left or right edge of the circle.
      slopePerpToTakeoff = slopePerpToTakeoff > 0 ? Infinity : -Infinity;
    }

    // y = slope*x + b
    // fy = slope*fx + b
    // fy - slope*fx = b
    // b = fy - slope*fx
    const bFrom = fromVisual.cy - slopePerpToTakeoff * fromVisual.cx;
    let cx: number;
    let cy: number;

    if (fromVisual.cy === toVisual.cy) {
      cx = (fromVisual.cx + toVisual.cx) / 2;
      cy = slopePerpToTakeoff * cx + bFrom;
    } else {
      // find the slope of the line between the 2 points
      const slopeBisector = (fromVisual.cy - toVisual.cy) / (fromVisual.cx - toVisual.cx);
      const slopePerpToBisector = -1 / slopeBisector;
      const midx = (fromVisual.cx + toVisual.cx) / 2;
      const midy = (fromVisual.cy + toVisual.cy) / 2;
      // b = fy - slope*fx
      const bPerp = midy - slopePerpToBisector * midx;

      if (isInf(slopePerpToTakeoff)) {
        cx = fromVisual.cx;
        cy = slopePerpToBisector * cx + bPerp;
      } else {
        // y = perpSlopeTakeoff*x + bFrom
        // y = perpSlopeBisector*x + bPerp
        // perpSlopeTakeoff*x + bFrom = perpSlopeBisector*x + bPerp
        // bFrom - bPerp = perpSlopeBisector*x - perpSlopeTakeoff*x
        // bFrom - bPerp = (perpSlopeBisector- perpSlopeTakeoff)*x
        // (bFrom - bPerp)/(perpSlopeBisector- perpSlopeTakeoff) = x
        cx = (bFrom - bPerp) / (slopePerpToBisector - slopePerpToTakeoff);
        cy = slopePerpToTakeoff * cx + bFrom;
      }
    }

    const cr = sqrt(square(fromVisual.cx - cx) + square(fromVisual.cy - cy));

    return { r: cr, x: cx, y: cy };
  }

  renderArc() {
    const { from, to, isSelected, isDashed } = this.props;

    const fromVisual = getVisualCenter(from);
    const toVisual = getVisualCenter(to);

    const takeoffAngle = takeoffθ(this.props);
    const circ = Connector.arcCircle(this.props);
    if (circ === undefined) {
      console.log('FIXME: arcCircle returned null');
      return <g key={this.props.element.uid} />;
    }

    const fromθ = atan2(fromVisual.cy - circ.y, fromVisual.cx - circ.x);
    const toθ = atan2(toVisual.cy - circ.y, toVisual.cx - circ.x);
    let spanθ = toθ - fromθ;
    if (spanθ > degToRad(180)) {
      spanθ -= degToRad(360);
    }

    // if the sweep flag is set, we need to negate the
    // inverse flag
    let inv: boolean = spanθ > 0 || spanθ <= degToRad(-180);

    const side1 =
      (circ.x - fromVisual.cx) * (toVisual.cy - fromVisual.cy) -
      (circ.y - fromVisual.cy) * (toVisual.cx - fromVisual.cx);
    const startA = intersectElementArc(from, circ, inv);
    const startR = sqrt(square(startA.x - fromVisual.cx) + square(startA.y - fromVisual.cy));
    const takeoffPoint = {
      x: fromVisual.cx + startR * cos(takeoffAngle),
      y: fromVisual.cy + startR * sin(takeoffAngle),
    };
    const side2 =
      (takeoffPoint.x - fromVisual.cx) * (toVisual.cy - fromVisual.cy) -
      (takeoffPoint.y - fromVisual.cy) * (toVisual.cx - fromVisual.cx);

    const sweep = side1 < 0 === side2 < 0;

    if (sweep) {
      inv = !inv;
    }
    const start = { x: fromVisual.cx, y: fromVisual.cy };
    const arcEnd = { x: toVisual.cx, y: toVisual.cy };
    const end = intersectElementArc(to, circ, !inv);

    const path = `M${start.x},${start.y}A${circ.r},${circ.r} 0 ${+sweep},${+inv} ${arcEnd.x},${arcEnd.y}`;

    let arrowθ = radToDeg(atan2(end.y - circ.y, end.x - circ.x)) - 90;
    if (inv) {
      arrowθ += 180;
    }

    let connectorClass = isSelected
      ? `${styles.connectorSelected} simlin-connector simlin-connector-selected`
      : `${styles.connector} simlin-connector`;
    if (isDashed && !isSelected) {
      connectorClass = `${styles.connectorDashed} simlin-connector simlin-connector-dashed`;
    }

    return (
      <g key={this.props.element.uid}>
        <path
          d={path}
          className={`${styles.connectorBg} simlin-connector-bg`}
          onPointerDown={this.handlePointerDownArc}
        />
        <path d={path} className={connectorClass} onPointerDown={this.handlePointerDownArc} />
        <Arrowhead
          point={end}
          angle={arrowθ}
          isSelected={isSelected}
          size={ArrowheadRadius}
          type="connector"
          onSelection={this.handlePointerDownArrowhead}
        />
      </g>
    );
  }

  static boundStraightLine(props: ConnectorProps): Rect {
    const { to, from } = props;
    const fromVisual = getVisualCenter(from);
    const toVisual = getVisualCenter(to);
    return {
      top: Math.min(toVisual.cy, fromVisual.cy),
      left: Math.min(toVisual.cx, fromVisual.cx),
      right: Math.max(toVisual.cx, fromVisual.cx),
      bottom: Math.max(toVisual.cy, fromVisual.cy),
    };
  }

  static boundArc(props: ConnectorProps): Rect | undefined {
    const circ = Connector.arcCircle(props);
    if (!circ) {
      return undefined;
    }

    const bounds = {
      top: circ.y - circ.r,
      left: circ.x - circ.r,
      right: circ.x + circ.r,
      bottom: circ.y + circ.r,
    };
    return bounds;
  }

  static bounds(props: ConnectorProps): Rect | undefined {
    if (Connector.isStraightLine(props)) {
      return Connector.boundStraightLine(props);
    } else {
      return Connector.boundArc(props);
    }
  }

  render() {
    if (Connector.isStraightLine(this.props)) {
      return this.renderStraightLine();
    } else {
      return this.renderArc();
    }
  }
}
