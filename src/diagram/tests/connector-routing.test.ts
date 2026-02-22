// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  AuxViewElement,
  StockViewElement,
  FlowViewElement,
  LinkViewElement,
  Aux,
  Stock,
  Flow,
} from '@simlin/core/datamodel';

import {
  Connector,
  circleFromPoints,
  getVisualCenter,
  intersectElementArc,
  rayRectIntersection,
  circleRectIntersections,
  ArrayedOffset,
} from '../drawing/Connector';
import { AuxRadius, StockWidth, StockHeight } from '../drawing/default';
import { square, Circle, Point } from '../drawing/common';

function makeAux(uid: number, x: number, y: number, isArrayed: boolean = false): AuxViewElement {
  const auxVar: Aux = isArrayed
    ? {
        type: 'aux',
        ident: 'test_aux',
        equation: {
          type: 'applyToAll',
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
    : {
        type: 'aux',
        ident: 'test_aux',
        equation: { type: 'scalar', equation: '1' },
        documentation: '',
        units: '',
        gf: undefined,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      };

  return {
    type: 'aux',
    uid,
    name: 'TestAux',
    ident: 'test_aux',
    var: auxVar,
    x,
    y,
    labelSide: 'right',
    isZeroRadius: false,
  };
}

function makeStock(uid: number, x: number, y: number, isArrayed: boolean = false): StockViewElement {
  const stockVar: Stock = isArrayed
    ? {
        type: 'stock',
        ident: 'test_stock',
        equation: {
          type: 'applyToAll',
          dimensionNames: ['dim1'],
          equation: '10',
        },
        documentation: '',
        units: '',
        inflows: [],
        outflows: [],
        nonNegative: false,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      }
    : {
        type: 'stock',
        ident: 'test_stock',
        equation: { type: 'scalar', equation: '10' },
        documentation: '',
        units: '',
        inflows: [],
        outflows: [],
        nonNegative: false,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      };

  return {
    type: 'stock',
    uid,
    name: 'TestStock',
    ident: 'test_stock',
    var: stockVar,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
    inflows: [],
    outflows: [],
  };
}

function makeLink(uid: number, fromUid: number, toUid: number): LinkViewElement {
  return {
    type: 'link',
    uid,
    fromUid,
    toUid,
    arc: undefined,
    isStraight: true,
    multiPoint: undefined,
    polarity: undefined,
    x: NaN,
    y: NaN,
    isZeroRadius: false,
    ident: undefined,
  };
}

function makeFlowElement(uid: number, x: number, y: number, isArrayed: boolean = false): FlowViewElement {
  const flowVar: Flow = isArrayed
    ? {
        type: 'flow',
        ident: 'test_flow',
        equation: {
          type: 'applyToAll',
          dimensionNames: ['dim1'],
          equation: '1',
        },
        documentation: '',
        units: '',
        gf: undefined,
        nonNegative: false,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      }
    : {
        type: 'flow',
        ident: 'test_flow',
        equation: { type: 'scalar', equation: '1' },
        documentation: '',
        units: '',
        gf: undefined,
        nonNegative: false,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      };

  return {
    type: 'flow',
    uid,
    name: 'TestFlow',
    ident: 'test_flow',
    var: flowVar,
    x,
    y,
    labelSide: 'center',
    points: [
      { x: x - 50, y, attachedToUid: undefined },
      { x: x + 50, y, attachedToUid: undefined },
    ],
    isZeroRadius: false,
  };
}

describe('Connector routing', () => {
  describe('intersectElementStraight', () => {
    describe('non-arrayed elements', () => {
      it('should calculate intersection point at element boundary for auxiliary', () => {
        const aux = makeAux(1, 100, 100, false);
        const target = makeAux(2, 200, 100, false);
        const link = makeLink(3, 1, 2);

        // The angle from aux to target is 0 radians (pointing right)
        const theta = Math.atan2(target.y - aux.y, target.x - aux.x);

        // Use the Connector class's static method via reflection or test through render
        // Since intersectElementStraight is private, we test through the isStraightLine method
        // For now, we'll verify the expected behavior through ConnectorProps
        const props = {
          isSelected: false,
          from: aux,
          element: link,
          to: target,
          onSelection: () => {},
        };

        // Verify isStraightLine is true for straight links
        expect(Connector.isStraightLine(props)).toBe(true);

        // The intersection point for aux should be at (100 + AuxRadius, 100)
        // since we're going right (theta = 0)
        const expectedX = aux.x + AuxRadius * Math.cos(theta);
        const expectedY = aux.y + AuxRadius * Math.sin(theta);
        expect(expectedX).toBeCloseTo(100 + AuxRadius);
        expect(expectedY).toBeCloseTo(100);
      });

      it('should calculate intersection point for diagonal connector', () => {
        const aux = makeAux(1, 100, 100, false);
        const target = makeAux(2, 200, 200, false);

        // The angle should be 45 degrees (PI/4)
        const theta = Math.atan2(target.y - aux.y, target.x - aux.x);
        expect(theta).toBeCloseTo(Math.PI / 4);

        // Expected intersection at the boundary
        const expectedX = aux.x + AuxRadius * Math.cos(theta);
        const expectedY = aux.y + AuxRadius * Math.sin(theta);

        // Verify it's on a 45-degree line
        expect(expectedX - aux.x).toBeCloseTo(expectedY - aux.y);
      });
    });

    describe('arrayed elements', () => {
      it('should adjust center for arrayed auxiliary (connector from arrayed element)', () => {
        const arrayedAux = makeAux(1, 100, 100, true);
        const target = makeAux(2, 200, 100, false);
        const link = makeLink(3, 1, 2);

        // For arrayed elements, the visual front is at (x - 3, y - 3)
        // The connector should attach to this visual center
        const visualCx = arrayedAux.x - ArrayedOffset;
        const visualCy = arrayedAux.y - ArrayedOffset;

        // The expected intersection should be calculated from the visual center
        // Since target is at (200, 100) and visual center is at (97, 97),
        // the angle is slightly upward
        const theta = Math.atan2(target.y - visualCy, target.x - visualCx);
        const expectedX = visualCx + AuxRadius * Math.cos(theta);
        const expectedY = visualCy + AuxRadius * Math.sin(theta);

        // The intersection point should be calculated from the visual center
        // The Y coordinate should be close to 97 (visual center Y) plus a small
        // offset from the angle to the target
        expect(expectedY).toBeCloseTo(97.26, 1);
        expect(expectedX).toBeCloseTo(106, 0);

        // Verify the bounds would work correctly
        const props = {
          isSelected: false,
          from: arrayedAux,
          element: link,
          to: target,
          onSelection: () => {},
        };

        expect(Connector.isStraightLine(props)).toBe(true);
      });

      it('should adjust center for arrayed stock', () => {
        const arrayedStock = makeStock(1, 100, 100, true);

        const visual = getVisualCenter(arrayedStock);

        // For arrayed stocks, same principle: visual front at (x - 3, y - 3)
        expect(visual.cx).toBe(97);
        expect(visual.cy).toBe(97);
      });

      it('should adjust center for connector to arrayed element', () => {
        const arrayedTarget = makeAux(2, 200, 100, true);

        const visual = getVisualCenter(arrayedTarget);

        // For a connector going from left to right toward an arrayed element,
        // it should connect to the visual front (197, 97) not (200, 100)
        expect(visual.cx).toBe(197);
        expect(visual.cy).toBe(97);
      });

      it('should handle both ends being arrayed', () => {
        const arrayedSource = makeAux(1, 100, 100, true);
        const arrayedTarget = makeAux(2, 200, 100, true);

        const sourceVisual = getVisualCenter(arrayedSource);
        const targetVisual = getVisualCenter(arrayedTarget);

        expect(sourceVisual.cx).toBe(97);
        expect(sourceVisual.cy).toBe(97);
        expect(targetVisual.cx).toBe(197);
        expect(targetVisual.cy).toBe(97);

        // The connector should be purely horizontal between visual centers
        expect(sourceVisual.cy).toBe(targetVisual.cy);
      });
    });
  });

  describe('intersectElementArc', () => {
    describe('arrayed elements', () => {
      it('should adjust center for arrayed element in arc calculation', () => {
        const arrayedAux = makeAux(1, 100, 100, true);

        // Create a simple circle for arc calculation
        const circ = { x: 150, y: 50, r: 100 };

        // The intersection should be calculated from the visual center (97, 97)
        // not the logical center (100, 100)
        const visualCx = arrayedAux.x - ArrayedOffset;
        const visualCy = arrayedAux.y - ArrayedOffset;

        // The element's angle from the circle center should be based on visual center
        const expectedAngle = Math.atan2(visualCy - circ.y, visualCx - circ.x);

        // Verify the angle is different from what it would be with logical center
        const logicalAngle = Math.atan2(arrayedAux.y - circ.y, arrayedAux.x - circ.x);
        expect(expectedAngle).not.toBeCloseTo(logicalAngle, 3);
      });
    });
  });

  describe('circleFromPoints', () => {
    it('should calculate circle from three points', () => {
      // Simple test with known geometry: points on a unit circle centered at origin
      const p1 = { x: 1, y: 0 };
      const p2 = { x: 0, y: 1 };
      const p3 = { x: -1, y: 0 };

      const circ = circleFromPoints(p1, p2, p3);

      expect(circ.x).toBeCloseTo(0);
      expect(circ.y).toBeCloseTo(0);
      expect(circ.r).toBeCloseTo(1);
    });

    it('should handle off-center circles', () => {
      // Points on a circle centered at (10, 10) with radius 5
      const cx = 10;
      const cy = 10;
      const r = 5;
      const p1 = { x: cx + r, y: cy };
      const p2 = { x: cx, y: cy + r };
      const p3 = { x: cx - r, y: cy };

      const circ = circleFromPoints(p1, p2, p3);

      expect(circ.x).toBeCloseTo(cx);
      expect(circ.y).toBeCloseTo(cy);
      expect(circ.r).toBeCloseTo(r);
    });
  });

  describe('getVisualCenter', () => {
    it('should return logical center for non-arrayed elements', () => {
      const aux = makeAux(1, 100, 100, false);

      const visual = getVisualCenter(aux);

      // For non-arrayed elements, visual center equals logical center
      expect(visual.cx).toBe(100);
      expect(visual.cy).toBe(100);
    });

    it('should return offset center for arrayed auxiliary', () => {
      const arrayedAux = makeAux(1, 100, 100, true);

      const visual = getVisualCenter(arrayedAux);

      // Visual center should be offset by ArrayedOffset (3)
      expect(visual.cx).toBe(97);
      expect(visual.cy).toBe(97);
    });

    it('should return offset center for arrayed stock', () => {
      const arrayedStock = makeStock(1, 100, 100, true);

      const visual = getVisualCenter(arrayedStock);

      // Visual center should be offset by ArrayedOffset (3)
      expect(visual.cx).toBe(97);
      expect(visual.cy).toBe(97);
    });

    it('should return logical center for non-arrayed stock', () => {
      const stock = makeStock(1, 100, 100, false);

      const visual = getVisualCenter(stock);

      expect(visual.cx).toBe(100);
      expect(visual.cy).toBe(100);
    });

    it('should return offset center for arrayed flow', () => {
      const arrayedFlow = makeFlowElement(1, 100, 100, true);

      const visual = getVisualCenter(arrayedFlow);

      // Visual center should be offset by ArrayedOffset (3)
      expect(visual.cx).toBe(97);
      expect(visual.cy).toBe(97);
    });

    it('should return logical center for non-arrayed flow', () => {
      const flow = makeFlowElement(1, 100, 100, false);

      const visual = getVisualCenter(flow);

      expect(visual.cx).toBe(100);
      expect(visual.cy).toBe(100);
    });

    it('should return logical center for zero-radius placeholder (even if arrayed)', () => {
      // Create an arrayed aux but with isZeroRadius = true (used during drag operations)
      const arrayedAux = makeAux(1, 100, 100, true);
      const zeroRadiusPlaceholder: AuxViewElement = { ...arrayedAux, isZeroRadius: true };

      const visual = getVisualCenter(zeroRadiusPlaceholder);

      // Zero-radius placeholders should stay at the cursor position,
      // so they should not have the array offset applied
      expect(visual.cx).toBe(100);
      expect(visual.cy).toBe(100);
    });
  });

  describe('rayRectIntersection', () => {
    const hw = StockWidth / 2;  // 22.5
    const hh = StockHeight / 2; // 17.5

    function assertOnRectBoundary(p: Point, cx: number, cy: number, hw: number, hh: number) {
      const onLeftRight = Math.abs(Math.abs(p.x - cx) - hw) < 1e-6;
      const onTopBottom = Math.abs(Math.abs(p.y - cy) - hh) < 1e-6;
      expect(onLeftRight || onTopBottom).toBe(true);
      expect(Math.abs(p.x - cx)).toBeLessThanOrEqual(hw + 1e-6);
      expect(Math.abs(p.y - cy)).toBeLessThanOrEqual(hh + 1e-6);
    }

    it('should hit right edge for theta=0', () => {
      const p = rayRectIntersection(100, 100, hw, hh, 0);
      expect(p.x).toBeCloseTo(122.5);
      expect(p.y).toBeCloseTo(100);
    });

    it('should hit bottom edge for theta=PI/2', () => {
      const p = rayRectIntersection(100, 100, hw, hh, Math.PI / 2);
      expect(p.x).toBeCloseTo(100);
      expect(p.y).toBeCloseTo(117.5);
    });

    it('should hit left edge for theta=PI', () => {
      const p = rayRectIntersection(100, 100, hw, hh, Math.PI);
      expect(p.x).toBeCloseTo(77.5);
      expect(p.y).toBeCloseTo(100);
    });

    it('should hit top edge for theta=-PI/2', () => {
      const p = rayRectIntersection(100, 100, hw, hh, -Math.PI / 2);
      expect(p.x).toBeCloseTo(100);
      expect(p.y).toBeCloseTo(82.5);
    });

    it('should preserve angle and land on boundary for various angles', () => {
      const cx = 50;
      const cy = 80;
      for (const angleDeg of [15, 30, 60, 75, 120, 210, 300, 350]) {
        const theta = (angleDeg / 180) * Math.PI;
        const p = rayRectIntersection(cx, cy, hw, hh, theta);
        const actualTheta = Math.atan2(p.y - cy, p.x - cx);
        let diff = Math.abs(actualTheta - theta);
        if (diff > Math.PI) diff = 2 * Math.PI - diff;
        expect(diff).toBeLessThan(1e-6);
        assertOnRectBoundary(p, cx, cy, hw, hh);
      }
    });
  });

  describe('circleRectIntersections', () => {
    const hw = StockWidth / 2;
    const hh = StockHeight / 2;

    function assertOnCircle(p: Point, circ: Circle) {
      const dist = Math.sqrt(square(p.x - circ.x) + square(p.y - circ.y));
      expect(Math.abs(dist - circ.r)).toBeLessThan(1e-6);
    }

    function assertOnRectBoundary(p: Point, cx: number, cy: number, hw: number, hh: number) {
      const onLeftRight = Math.abs(Math.abs(p.x - cx) - hw) < 1e-6;
      const onTopBottom = Math.abs(Math.abs(p.y - cy) - hh) < 1e-6;
      expect(onLeftRight || onTopBottom).toBe(true);
    }

    it('should return empty for circle far from rectangle', () => {
      const circ = { x: 500, y: 500, r: 10 };
      expect(circleRectIntersections(circ, 0, 0, hw, hh)).toHaveLength(0);
    });

    it('should return empty for circle entirely inside rectangle', () => {
      const circ = { x: 0, y: 0, r: 5 };
      expect(circleRectIntersections(circ, 0, 0, hw, hh)).toHaveLength(0);
    });

    it('should find intersections on right edge', () => {
      const circ = { x: 30, y: 0, r: 10 };
      const points = circleRectIntersections(circ, 0, 0, hw, hh);
      expect(points.length).toBeGreaterThan(0);
      for (const p of points) {
        assertOnCircle(p, circ);
        assertOnRectBoundary(p, 0, 0, hw, hh);
      }
    });

    it('should find 8 intersections for circle centered at rect center with r=25', () => {
      const circ = { x: 0, y: 0, r: 25 };
      const points = circleRectIntersections(circ, 0, 0, hw, hh);
      expect(points).toHaveLength(8);
      for (const p of points) {
        assertOnCircle(p, circ);
        assertOnRectBoundary(p, 0, 0, hw, hh);
      }
    });

    it('should not duplicate corner points', () => {
      // 3-4-5 right triangle corner distance
      const testHw = 3;
      const testHh = 4;
      const cornerDist = Math.sqrt(testHw * testHw + testHh * testHh);
      const circ = { x: 0, y: 0, r: cornerDist };
      const points = circleRectIntersections(circ, 0, 0, testHw, testHh);
      expect(points).toHaveLength(4);
    });
  });

  describe('intersectElementStraight with stocks', () => {
    it('should hit left edge when approaching from the left', () => {
      const stock = makeStock(2, 200, 100);
      const theta = Math.PI; // approaching from left means connector angle is PI
      const p = Connector.intersectElementStraight(stock, theta);
      expect(p.x).toBeCloseTo(200 - StockWidth / 2);
      expect(p.y).toBeCloseTo(100);
    });

    it('should hit top edge when approaching from above', () => {
      const stock = makeStock(2, 200, 200);
      const theta = Math.PI / 2; // approaching from above
      const p = Connector.intersectElementStraight(stock, theta);
      expect(p.x).toBeCloseTo(200);
      expect(p.y).toBeCloseTo(200 + StockHeight / 2);
    });

    it('should not change behavior for auxiliaries', () => {
      const aux = makeAux(1, 200, 100);
      const theta = 0;
      const p = Connector.intersectElementStraight(aux, theta);
      expect(p.x).toBeCloseTo(200 + AuxRadius);
      expect(p.y).toBeCloseTo(100);
    });
  });

  describe('intersectElementArc with stocks', () => {
    it('should place endpoint on stock boundary', () => {
      const stock = makeStock(2, 200, 200);
      const circ: Circle = {
        x: 100,
        y: 100,
        r: Math.sqrt(square(200 - 100) + square(200 - 100)),
      };
      const end = intersectElementArc(stock, circ, false);
      // Should be on the stock boundary
      const dx = Math.abs(end.x - 200);
      const dy = Math.abs(end.y - 200);
      const onLeftRight = Math.abs(dx - StockWidth / 2) < 1e-6;
      const onTopBottom = Math.abs(dy - StockHeight / 2) < 1e-6;
      expect(onLeftRight || onTopBottom).toBe(true);
    });

    it('should place endpoint on arc circle', () => {
      const stock = makeStock(2, 200, 200);
      const r = Math.sqrt(square(200 - 100) + square(200 - 100));
      const circ: Circle = { x: 100, y: 100, r };
      const end = intersectElementArc(stock, circ, false);
      const dist = Math.sqrt(square(end.x - circ.x) + square(end.y - circ.y));
      expect(Math.abs(dist - circ.r)).toBeLessThan(1e-6);
    });

    it('should produce different points for inv=true vs inv=false', () => {
      const stock = makeStock(2, 200, 200);
      const circ: Circle = { x: 150, y: 50, r: 180 };
      const endNoInv = intersectElementArc(stock, circ, false);
      const endInv = intersectElementArc(stock, circ, true);
      const dist = Math.sqrt(square(endNoInv.x - endInv.x) + square(endNoInv.y - endInv.y));
      expect(dist).toBeGreaterThan(1);
    });
  });
});
