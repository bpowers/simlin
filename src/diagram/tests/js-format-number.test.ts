// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { jsFormatNumber } from '../render-common';

// Mirror of the Rust contract in
// `src/simlin-engine/src/diagram/common.rs::test_js_format_number*`. The two
// formatters MUST agree byte-for-byte so the cross-language SVG parity at
// `svg-rendering.test.ts` (and the Rust regression guard
// `diagram::connector::tests::test_render_arc_svg_byte_identical`) holds.
describe('jsFormatNumber', () => {
  describe('JS Number.toString() parity (no fractional input)', () => {
    it('integer-valued doubles drop the decimal point', () => {
      expect(jsFormatNumber(45.0)).toBe('45');
      expect(jsFormatNumber(0.0)).toBe('0');
      expect(jsFormatNumber(1.0)).toBe('1');
      expect(jsFormatNumber(-1.0)).toBe('-1');
      expect(jsFormatNumber(100.0)).toBe('100');
    });

    it('-0 normalizes to "0"', () => {
      expect(jsFormatNumber(-0.0)).toBe('0');
    });

    it('fractional values keep minimal digits', () => {
      expect(jsFormatNumber(0.5)).toBe('0.5');
      expect(jsFormatNumber(-3.125)).toBe('-3.125');
    });

    it('non-finite values match JS string forms', () => {
      expect(jsFormatNumber(NaN)).toBe('NaN');
      expect(jsFormatNumber(Infinity)).toBe('Infinity');
      expect(jsFormatNumber(-Infinity)).toBe('-Infinity');
    });
  });

  describe('quantization to six decimals', () => {
    it('values rounding to a clean integer collapse to the integer', () => {
      expect(jsFormatNumber(100.0000004)).toBe('100');
      expect(jsFormatNumber(2.0000004)).toBe('2');
    });

    it('rounds the seventh decimal half-up', () => {
      expect(jsFormatNumber(0.1234567)).toBe('0.123457');
      expect(jsFormatNumber(0.1234564)).toBe('0.123456');
      expect(jsFormatNumber(-0.1234567)).toBe('-0.123457');
    });

    it('1-ULP siblings of a clean 6-decimal value collapse to it', () => {
      const clean = 273.205081;
      const cleanBits = new Float64Array([clean]);
      const intView = new BigInt64Array(cleanBits.buffer);
      const ulpAboveBits = new BigInt64Array([intView[0] + 1n]);
      const ulpBelowBits = new BigInt64Array([intView[0] - 1n]);
      const ulpAbove = new Float64Array(ulpAboveBits.buffer)[0];
      const ulpBelow = new Float64Array(ulpBelowBits.buffer)[0];
      expect(jsFormatNumber(ulpAbove)).toBe(jsFormatNumber(clean));
      expect(jsFormatNumber(ulpBelow)).toBe(jsFormatNumber(clean));
    });

    it('two ULP-different values from the bug repro print the same string', () => {
      expect(jsFormatNumber(273.2050807568877)).toBe(jsFormatNumber(273.20508075688764));
    });

    it('strips trailing zeros', () => {
      expect(jsFormatNumber(1.5)).toBe('1.5');
      // A value below .5 in the seventh decimal rounds down to 1.500000,
      // then trailing-zero trimming reduces that to "1.5".
      expect(jsFormatNumber(1.5000004)).toBe('1.5');
    });

    it('renormalizes -0 even after rounding a tiny negative to zero', () => {
      expect(jsFormatNumber(-0.0000001)).toBe('0');
    });
  });
});
