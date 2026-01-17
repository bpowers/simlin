// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { updateArcAngle, radToDeg } from '../arc-utils';

describe('arc-utils', () => {
  describe('updateArcAngle', () => {
    describe('when arc is undefined (straight line)', () => {
      it('should preserve undefined when angle difference is zero', () => {
        expect(updateArcAngle(undefined, 0)).toBeUndefined();
      });

      it('should preserve undefined when angle difference is positive', () => {
        expect(updateArcAngle(undefined, 45)).toBeUndefined();
      });

      it('should preserve undefined when angle difference is negative', () => {
        expect(updateArcAngle(undefined, -30)).toBeUndefined();
      });

      it('should preserve undefined for large angle differences', () => {
        expect(updateArcAngle(undefined, 180)).toBeUndefined();
        expect(updateArcAngle(undefined, -180)).toBeUndefined();
        expect(updateArcAngle(undefined, 360)).toBeUndefined();
      });
    });

    describe('when arc is defined (curved line)', () => {
      it('should subtract the angle difference from the arc', () => {
        expect(updateArcAngle(180, 0)).toBe(180);
        expect(updateArcAngle(180, 10)).toBe(170);
        expect(updateArcAngle(180, -10)).toBe(190);
      });

      it('should handle zero arc value', () => {
        expect(updateArcAngle(0, 0)).toBe(0);
        expect(updateArcAngle(0, 45)).toBe(-45);
        expect(updateArcAngle(0, -45)).toBe(45);
      });

      it('should handle negative arc values', () => {
        expect(updateArcAngle(-90, 10)).toBe(-100);
        expect(updateArcAngle(-90, -10)).toBe(-80);
      });

      it('should handle fractional angle differences', () => {
        expect(updateArcAngle(180, 0.5)).toBeCloseTo(179.5);
        expect(updateArcAngle(180, -0.5)).toBeCloseTo(180.5);
      });

      it('should handle very small angle differences', () => {
        expect(updateArcAngle(180, 0.001)).toBeCloseTo(179.999);
      });
    });
  });

  describe('radToDeg', () => {
    it('should convert common radian values to degrees', () => {
      expect(radToDeg(0)).toBe(0);
      expect(radToDeg(Math.PI)).toBeCloseTo(180);
      expect(radToDeg(Math.PI / 2)).toBeCloseTo(90);
      expect(radToDeg(Math.PI / 4)).toBeCloseTo(45);
      expect(radToDeg(2 * Math.PI)).toBeCloseTo(360);
    });

    it('should handle negative radian values', () => {
      expect(radToDeg(-Math.PI)).toBeCloseTo(-180);
      expect(radToDeg(-Math.PI / 2)).toBeCloseTo(-90);
    });

    it('should handle small radian values', () => {
      expect(radToDeg(0.01)).toBeCloseTo(0.5729577951308232);
    });
  });
});
