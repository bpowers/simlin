// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { calcViewBox, mergeBounds, Rect } from '../drawing/common';

describe('common', () => {
  describe('mergeBounds', () => {
    it('should merge two non-overlapping bounds', () => {
      const a: Rect = { top: 0, left: 0, right: 10, bottom: 10 };
      const b: Rect = { top: 20, left: 20, right: 30, bottom: 30 };
      const result = mergeBounds(a, b);
      expect(result).toEqual({ top: 0, left: 0, right: 30, bottom: 30 });
    });

    it('should merge overlapping bounds', () => {
      const a: Rect = { top: 0, left: 0, right: 20, bottom: 20 };
      const b: Rect = { top: 10, left: 10, right: 30, bottom: 30 };
      const result = mergeBounds(a, b);
      expect(result).toEqual({ top: 0, left: 0, right: 30, bottom: 30 });
    });

    it('should handle bounds with negative coordinates', () => {
      const a: Rect = { top: -10, left: -10, right: 10, bottom: 10 };
      const b: Rect = { top: 0, left: 0, right: 20, bottom: 20 };
      const result = mergeBounds(a, b);
      expect(result).toEqual({ top: -10, left: -10, right: 20, bottom: 20 });
    });
  });

  describe('calcViewBox', () => {
    it('should return undefined for empty list', () => {
      const elements = List<Rect | undefined>();
      expect(calcViewBox(elements)).toBeUndefined();
    });

    it('should return the bounds of a single element', () => {
      const elements = List<Rect | undefined>([
        { top: 100, left: 150, right: 200, bottom: 180 },
      ]);
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: 100, left: 150, right: 200, bottom: 180 });
    });

    it('should calculate tight bounds from multiple elements', () => {
      const elements = List<Rect | undefined>([
        { top: 100, left: 150, right: 200, bottom: 180 },
        { top: 200, left: 300, right: 400, bottom: 280 },
        { top: 50, left: 100, right: 250, bottom: 150 },
      ]);
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: 50, left: 100, right: 400, bottom: 280 });
    });

    it('should skip undefined elements in the list', () => {
      const elements = List<Rect | undefined>([
        { top: 100, left: 150, right: 200, bottom: 180 },
        undefined,
        { top: 200, left: 300, right: 400, bottom: 280 },
        undefined,
      ]);
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: 100, left: 150, right: 400, bottom: 280 });
    });

    it('should handle all undefined elements by returning Infinity bounds', () => {
      const elements = List<Rect | undefined>([undefined, undefined]);
      const result = calcViewBox(elements);
      expect(result).toEqual({
        top: Infinity,
        left: Infinity,
        right: -Infinity,
        bottom: -Infinity,
      });
    });

    it('should handle elements with negative coordinates', () => {
      const elements = List<Rect | undefined>([
        { top: -50, left: -100, right: 50, bottom: 50 },
        { top: 0, left: 0, right: 100, bottom: 100 },
      ]);
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: -50, left: -100, right: 100, bottom: 100 });
    });
  });
});
