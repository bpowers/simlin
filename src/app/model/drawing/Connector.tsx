// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { ViewElement } from '../../../engine/xmile';

import { Arrowhead } from './Arrowhead';
import { Circle, isInf, isZero, Point, Rect, square } from './common';
import { ArrowheadRadius, AuxRadius, StraightLineMax } from './default';

const styles = createStyles({
  connector: {
    strokeWidth: 0.5,
    stroke: 'gray',
    fill: 'none',
  },
  connectorSelected: {
    strokeWidth: 1,
    stroke: '#4444dd',
    fill: 'none',
  },
  connectorBg: {
    strokeWidth: 7,
    stroke: 'white',
    opacity: 0,
    fill: 'none',
  },
});

// math functions we care about
const atan2 = Math.atan2;
const sin = Math.sin;
const cos = Math.cos;
const tan = Math.tan;
const PI = Math.PI;
const sqrt = Math.sqrt;

// converts an angle associated with a connector (in degrees) into an
// angle in the coordinate system of SVG canvases where the origin is
// in the upper-left of the screen and Y grows down, and the domain is
// -180 to 180.
function xmileToCanvasAngle(inDeg: number): number {
  let outDeg = (360 - inDeg) % 360;
  if (outDeg > 180) {
    outDeg -= 360;
  }
  return outDeg;
}

export function canvasToXmileAngle(inDeg: number): number {
  if (inDeg < 0) {
    inDeg += 360;
  }
  return (360 - inDeg) % 360;
}

const degToRad = (d: number): number => {
  return (d / 180) * PI;
};

const radToDeg = (r: number): number => {
  return (r * 180) / PI;
};

export function takeoffθ(props: Pick<ConnectorProps, 'element' | 'from' | 'to' | 'arcPoint'>): number {
  const { element, from, to, arcPoint } = props;

  if (arcPoint && !(to.cx === arcPoint.x && to.cy === arcPoint.y)) {
    // special case moving
    // if (to.cx === arcPoint.x && to.cy === arcPoint.y) {
    //   if (element.angle) {
    //     return degToRad(xmileToCanvasAngle(element.angle));
    //   } else {
    //     // its a straight line
    //     return atan2(to.cy - from.cy, to.cx - from.cx);
    //   }
    // }
    const circ = circleFromPoints({ x: from.cx, y: from.cy }, { x: to.cx, y: to.cy }, arcPoint);
    const fromθ = Math.atan2(from.cy - circ.y, from.cx - circ.x);
    const toθ = atan2(to.cy - circ.y, to.cx - circ.x);
    let spanθ = toθ - fromθ;
    if (spanθ > degToRad(180)) {
      spanθ -= degToRad(360);
    }

    const inv: boolean = spanθ > 0 || spanθ <= degToRad(-180);

    // check if the user's cursor and the center of the arc's circle are on the same
    // side of the straight line connecting the from and to elements.  If so, we need
    // to set the sweep flag.
    const side1 = (circ.x - from.cx) * (to.cy - from.cy) - (circ.y - from.cy) * (to.cx - from.cx);
    const side2 = (arcPoint.x - from.cx) * (to.cy - from.cy) - (arcPoint.y - from.cy) * (to.cx - from.cx);
    // eslint-disable-next-line no-mixed-operators
    const sweep = side1 < 0 === side2 < 0;

    return fromθ + degToRad(((sweep && inv) || (!sweep && !inv) ? -1 : 1) * 90);
  }
  if (element.angle !== undefined) {
    // convert from counter-clockwise (XMILE) to
    // clockwise (display, where Y grows down
    // instead of up)
    return degToRad(xmileToCanvasAngle(element.angle));
  } else {
    console.log(`connector from ${element.from || ''} doesn't have x, y, or angle`);
    return NaN;
  }
}

export function intersectElementArc(element: ViewElement, circ: Circle, inv: boolean): Point {
  let r: number = AuxRadius;
  // FIXME: actually calculate intersections
  if (element.type === 'module') {
    r = 25;
  } else if (element.type === 'stock') {
    r = 15;
  } else if (element.isZeroRadius) {
    r = 0;
  }

  // the angle that when added or subtracted from
  // elementCenterθ results in the point where the arc
  // intersects with the shape
  const offθ = tan(r / circ.r);
  const elementCenterθ = atan2(element.cy - circ.y, element.cx - circ.x);

  // FIXME: can we do this without an inverse flag?

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
    console.log('blerg');
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

interface ConnectorPropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  from: ViewElement;
  element: ViewElement;
  to: ViewElement;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isArrowhead: boolean) => void;
  arcPoint?: Point;
}

export type ConnectorProps = Pick<
  ConnectorPropsFull,
  'isSelected' | 'element' | 'from' | 'to' | 'onSelection' | 'arcPoint'
>;

export const Connector = withStyles(styles)(
  class Conn extends React.PureComponent<ConnectorPropsFull> {
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

    private static intersectElementStraight(element: ViewElement, θ: number): Point {
      let r: number = AuxRadius;
      // FIXME: actually calculate intersections
      if (element.type === 'module') {
        r = 25;
      } else if (element.type === 'stock') {
        r = 15;
      } else if (element.isZeroRadius) {
        r = 0;
      }

      return {
        x: element.cx + r * cos(θ),
        y: element.cy + r * sin(θ),
      };
    }

    static isStraightLine(props: ConnectorProps): boolean {
      const from = props.from;
      const to = props.to;

      const takeoffAngle = takeoffθ(props);
      const midθ = atan2(to.cy - from.cy, to.cx - from.cx);

      return Math.abs(midθ - takeoffAngle) < degToRad(StraightLineMax);
    }

    renderStraightLine() {
      const { from, to, classes, isSelected } = this.props;

      const θ = atan2(to.cy - from.cy, to.cx - from.cx);
      const start = Conn.intersectElementStraight(from, θ);
      const end = Conn.intersectElementStraight(to, oppositeθ(θ));

      const arrowθ = radToDeg(θ);
      const path = `M${start.x},${start.y}L${end.x},${end.y}`;

      return (
        <g key={this.props.element.uid}>
          <path d={path} className={classes.connectorBg} onPointerDown={this.handlePointerDownArc} />
          <path
            d={path}
            className={isSelected ? classes.connectorSelected : classes.connector}
            onPointerDown={this.handlePointerDownArc}
          />
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

      if (arcPoint && !(to.cx === arcPoint.x && to.cy === arcPoint.y)) {
        return circleFromPoints({ x: from.cx, y: from.cy }, { x: to.cx, y: to.cy }, arcPoint);
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
      const bFrom = from.cy - slopePerpToTakeoff * from.cx;
      let cx: number;
      let cy: number;

      if (from.cy === to.cy) {
        cx = (from.cx + to.cx) / 2;
        cy = slopePerpToTakeoff * cx + bFrom;
      } else {
        // find the slope of the line between the 2 points
        const slopeBisector = (from.cy - to.cy) / (from.cx - to.cx);
        const slopePerpToBisector = -1 / slopeBisector;
        const midx = (from.cx + to.cx) / 2;
        const midy = (from.cy + to.cy) / 2;
        // b = fy - slope*fx
        const bPerp = midy - slopePerpToBisector * midx;

        if (isInf(slopePerpToTakeoff)) {
          cx = from.cx;
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

      const cr = sqrt(square(from.cx - cx) + square(from.cy - cy));

      return { r: cr, x: cx, y: cy };
    }

    renderArc() {
      const { from, to, classes, isSelected } = this.props;

      const takeoffAngle = takeoffθ(this.props);
      const circ = Conn.arcCircle(this.props);
      if (circ === undefined) {
        console.log('FIXME: arcCircle returned null');
        return <g key={this.props.element.uid} />;
      }

      const fromθ = atan2(from.cy - circ.y, from.cx - circ.x);
      const toθ = atan2(to.cy - circ.y, to.cx - circ.x);
      let spanθ = toθ - fromθ;
      if (spanθ > degToRad(180)) {
        spanθ -= degToRad(360);
      }

      // if the sweep flag is set, we need to negate the
      // inverse flag
      let inv: boolean = spanθ > 0 || spanθ <= degToRad(-180);

      const side1 = (circ.x - from.cx) * (to.cy - from.cy) - (circ.y - from.cy) * (to.cx - from.cx);
      const startA = intersectElementArc(from, circ, inv);
      const startR = sqrt(square(startA.x - from.cx) + square(startA.y - from.cy));
      const takeoffPoint = {
        x: from.cx + startR * cos(takeoffAngle),
        y: from.cy + startR * sin(takeoffAngle),
      };
      const side2 = (takeoffPoint.x - from.cx) * (to.cy - from.cy) - (takeoffPoint.y - from.cy) * (to.cx - from.cx);
      // eslint-disable-next-line no-mixed-operators
      const sweep = side1 < 0 === side2 < 0;

      if (sweep) {
        inv = !inv;
      }
      const start = { x: from.cx, y: from.cy };
      const end = intersectElementArc(to, circ, !inv);

      const path = `M${start.x},${start.y}A${circ.r},${circ.r} 0 ${+sweep},${+inv} ${end.x},${end.y}`;

      let arrowθ = radToDeg(atan2(end.y - circ.y, end.x - circ.x)) - 90;
      if (inv) {
        arrowθ += 180;
      }

      return (
        <g>
          <path d={path} className={classes.connectorBg} onPointerDown={this.handlePointerDownArc} />
          <path
            d={path}
            className={isSelected ? classes.connectorSelected : classes.connector}
            onPointerDown={this.handlePointerDownArc}
          />
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
      return {
        top: Math.min(to.cy, from.cy),
        left: Math.min(to.cx, from.cx),
        right: Math.max(to.cx, from.cx),
        bottom: Math.max(to.cy, from.cy),
      };
    }

    static boundArc(props: ConnectorProps): Rect | undefined {
      const circ = Conn.arcCircle(props);
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
      if (Conn.isStraightLine(props)) {
        return Conn.boundStraightLine(props);
      } else {
        return Conn.boundArc(props);
      }
    }

    render() {
      if (Conn.isStraightLine(this.props)) {
        return this.renderStraightLine();
      } else {
        return this.renderArc();
      }
    }
  },
);
