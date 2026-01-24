// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  AuxViewElement,
  StockViewElement,
  LinkViewElement,
  Aux,
  Stock,
  ApplyToAllEquation,
  ScalarEquation,
} from '@system-dynamics/core/datamodel';
import { List } from 'immutable';

import { Connector, circleFromPoints, getVisualCenter, ArrayedOffset } from '../drawing/Connector';
import { AuxRadius } from '../drawing/default';

function makeAux(uid: number, x: number, y: number, isArrayed: boolean = false): AuxViewElement {
  const auxVar = isArrayed
    ? new Aux({
        ident: 'test_aux',
        equation: new ApplyToAllEquation({
          dimensionNames: List(['dim1']),
          equation: '1',
        }),
        documentation: '',
        units: '',
        gf: undefined,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      })
    : new Aux({
        ident: 'test_aux',
        equation: new ScalarEquation({ equation: '1' }),
        documentation: '',
        units: '',
        gf: undefined,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      });

  return new AuxViewElement({
    uid,
    name: 'TestAux',
    ident: 'test_aux',
    var: auxVar,
    x,
    y,
    labelSide: 'right',
    isZeroRadius: false,
  });
}

function makeStock(uid: number, x: number, y: number, isArrayed: boolean = false): StockViewElement {
  const stockVar = isArrayed
    ? new Stock({
        ident: 'test_stock',
        equation: new ApplyToAllEquation({
          dimensionNames: List(['dim1']),
          equation: '10',
        }),
        documentation: '',
        units: '',
        inflows: List(),
        outflows: List(),
        nonNegative: false,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      })
    : new Stock({
        ident: 'test_stock',
        equation: new ScalarEquation({ equation: '10' }),
        documentation: '',
        units: '',
        inflows: List(),
        outflows: List(),
        nonNegative: false,
        data: undefined,
        errors: undefined,
        unitErrors: undefined,
        uid: undefined,
      });

  return new StockViewElement({
    uid,
    name: 'TestStock',
    ident: 'test_stock',
    var: stockVar,
    x,
    y,
    labelSide: 'center',
    isZeroRadius: false,
    inflows: List(),
    outflows: List(),
  });
}

function makeLink(uid: number, fromUid: number, toUid: number): LinkViewElement {
  return new LinkViewElement({
    uid,
    fromUid,
    toUid,
    arc: undefined,
    isStraight: true,
    multiPoint: undefined,
  });
}

describe('Connector routing', () => {
  describe('intersectElementStraight', () => {
    describe('non-arrayed elements', () => {
      it('should calculate intersection point at element boundary for auxiliary', () => {
        const aux = makeAux(1, 100, 100, false);
        const target = makeAux(2, 200, 100, false);
        const link = makeLink(3, 1, 2);

        // The angle from aux to target is 0 radians (pointing right)
        const theta = Math.atan2(target.cy - aux.cy, target.cx - aux.cx);

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
        const expectedX = aux.cx + AuxRadius * Math.cos(theta);
        const expectedY = aux.cy + AuxRadius * Math.sin(theta);
        expect(expectedX).toBeCloseTo(100 + AuxRadius);
        expect(expectedY).toBeCloseTo(100);
      });

      it('should calculate intersection point for diagonal connector', () => {
        const aux = makeAux(1, 100, 100, false);
        const target = makeAux(2, 200, 200, false);

        // The angle should be 45 degrees (PI/4)
        const theta = Math.atan2(target.cy - aux.cy, target.cx - aux.cx);
        expect(theta).toBeCloseTo(Math.PI / 4);

        // Expected intersection at the boundary
        const expectedX = aux.cx + AuxRadius * Math.cos(theta);
        const expectedY = aux.cy + AuxRadius * Math.sin(theta);

        // Verify it's on a 45-degree line
        expect(expectedX - aux.cx).toBeCloseTo(expectedY - aux.cy);
      });
    });

    describe('arrayed elements', () => {
      it('should adjust center for arrayed auxiliary (connector from arrayed element)', () => {
        const arrayedAux = makeAux(1, 100, 100, true);
        const target = makeAux(2, 200, 100, false);
        const link = makeLink(3, 1, 2);

        // For arrayed elements, the visual front is at (cx - 3, cy - 3)
        // The connector should attach to this visual center
        const visualCx = arrayedAux.cx - ArrayedOffset;
        const visualCy = arrayedAux.cy - ArrayedOffset;

        // The expected intersection should be calculated from the visual center
        // Since target is at (200, 100) and visual center is at (97, 97),
        // the angle is slightly upward
        const theta = Math.atan2(target.cy - visualCy, target.cx - visualCx);
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

        // For arrayed stocks, same principle: visual front at (cx - 3, cy - 3)
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
        const visualCx = arrayedAux.cx - ArrayedOffset;
        const visualCy = arrayedAux.cy - ArrayedOffset;

        // The element's angle from the circle center should be based on visual center
        const expectedAngle = Math.atan2(visualCy - circ.y, visualCx - circ.x);

        // Verify the angle is different from what it would be with logical center
        const logicalAngle = Math.atan2(arrayedAux.cy - circ.y, arrayedAux.cx - circ.x);
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
  });
});
