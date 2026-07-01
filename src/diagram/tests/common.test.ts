// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  calcViewBox,
  encodeNameNewlines,
  mergeBounds,
  Rect,
  sanitizeLabelInput,
  searchableName,
} from '../drawing/common';

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
      const elements: (Rect | undefined)[] = [];
      expect(calcViewBox(elements)).toBeUndefined();
    });

    it('should return the bounds of a single element', () => {
      const elements = [{ top: 100, left: 150, right: 200, bottom: 180 }];
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: 100, left: 150, right: 200, bottom: 180 });
    });

    it('should calculate tight bounds from multiple elements', () => {
      const elements = [
        { top: 100, left: 150, right: 200, bottom: 180 },
        { top: 200, left: 300, right: 400, bottom: 280 },
        { top: 50, left: 100, right: 250, bottom: 150 },
      ];
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: 50, left: 100, right: 400, bottom: 280 });
    });

    it('should skip undefined elements in the list', () => {
      const elements = [
        { top: 100, left: 150, right: 200, bottom: 180 },
        undefined,
        { top: 200, left: 300, right: 400, bottom: 280 },
        undefined,
      ];
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: 100, left: 150, right: 400, bottom: 280 });
    });

    it('should handle all undefined elements by returning Infinity bounds', () => {
      const elements = [undefined, undefined];
      const result = calcViewBox(elements);
      expect(result).toEqual({
        top: Infinity,
        left: Infinity,
        right: -Infinity,
        bottom: -Infinity,
      });
    });

    it('should handle elements with negative coordinates', () => {
      const elements = [
        { top: -50, left: -100, right: 50, bottom: 50 },
        { top: 0, left: 0, right: 100, bottom: 100 },
      ];
      const result = calcViewBox(elements);
      expect(result).toEqual({ top: -50, left: -100, right: 100, bottom: 100 });
    });
  });

  describe('searchableName', () => {
    it('should convert actual newlines to spaces', () => {
      const name = 'maximum\ngrowth rate';
      expect(searchableName(name)).toBe('maximum growth rate');
    });

    it('should convert escaped newlines to spaces', () => {
      const name = 'maximum\\ngrowth rate';
      expect(searchableName(name)).toBe('maximum growth rate');
    });

    it('should handle multiple actual newlines', () => {
      const name = 'fraction\nof carrying\ncapacity used';
      expect(searchableName(name)).toBe('fraction of carrying capacity used');
    });

    it('should handle multiple escaped newlines', () => {
      const name = 'fraction\\nof carrying\\ncapacity used';
      expect(searchableName(name)).toBe('fraction of carrying capacity used');
    });

    it('should handle mixed actual and escaped newlines', () => {
      const name = 'mixed\nnewline\\nformat';
      expect(searchableName(name)).toBe('mixed newline format');
    });

    it('should return unchanged names without newlines', () => {
      const name = 'simple name';
      expect(searchableName(name)).toBe('simple name');
    });

    it('should handle empty strings', () => {
      expect(searchableName('')).toBe('');
    });
  });

  describe('sanitizeLabelInput', () => {
    it('trims surrounding whitespace on a single-line name', () => {
      expect(sanitizeLabelInput('  births  ')).toBe('births');
    });

    it('drops a trailing newline left by an accidental line break', () => {
      expect(sanitizeLabelInput('drain\n')).toBe('drain');
    });

    it('drops leading and interior blank lines but keeps intentional breaks', () => {
      expect(sanitizeLabelInput('\nfraction of\n\n  carrying capacity  \n')).toBe('fraction of\ncarrying capacity');
    });

    it('collapses all-whitespace input to the empty string (treated as cancel)', () => {
      expect(sanitizeLabelInput('  \n \n\t')).toBe('');
      expect(sanitizeLabelInput('')).toBe('');
    });
  });

  describe('encodeNameNewlines', () => {
    it('encodes a single newline to a literal backslash-n', () => {
      expect(encodeNameNewlines('a\nb')).toBe('a\\nb');
    });

    it('encodes EVERY newline (the old single-occurrence replace missed the rest)', () => {
      expect(encodeNameNewlines('a\nb\nc')).toBe('a\\nb\\nc');
    });

    it('leaves names without newlines unchanged', () => {
      expect(encodeNameNewlines('plain name')).toBe('plain name');
    });
  });
});
