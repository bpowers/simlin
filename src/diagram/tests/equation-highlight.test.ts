// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import { EquationError, UnitError, UnitErrorKind, ErrorCode } from '@simlin/core/datamodel';

import {
  applyToAllPrefix,
  byteOffsetToUtf16,
  highlightRangeForField,
  highlightSpansForLines,
  slatePointForOffset,
} from '../equation-highlight';

describe('byteOffsetToUtf16', () => {
  it('is the identity for ASCII', () => {
    expect(byteOffsetToUtf16('a + b', 0)).toBe(0);
    expect(byteOffsetToUtf16('a + b', 4)).toBe(4);
    expect(byteOffsetToUtf16('a + b', 5)).toBe(5);
  });

  it('clamps negative and past-the-end offsets', () => {
    expect(byteOffsetToUtf16('abc', -1)).toBe(0);
    expect(byteOffsetToUtf16('abc', 99)).toBe(3);
  });

  it('accounts for 2-byte characters', () => {
    // 'é' is 2 bytes in UTF-8 but 1 UTF-16 code unit.
    const s = 'café + b';
    expect(byteOffsetToUtf16(s, 5)).toBe(4); // after 'café'
    expect(byteOffsetToUtf16(s, 7)).toBe(6); // before '+'
  });

  it('accounts for 3-byte characters', () => {
    // '時' is 3 bytes in UTF-8, 1 UTF-16 code unit.
    const s = '時間 + x';
    expect(byteOffsetToUtf16(s, 6)).toBe(2); // after the two CJK characters
  });

  it('accounts for 4-byte (surrogate pair) characters', () => {
    // '🌊' is 4 bytes in UTF-8, 2 UTF-16 code units.
    const s = '🌊x';
    expect(byteOffsetToUtf16(s, 4)).toBe(2);
    expect(byteOffsetToUtf16(s, 5)).toBe(3);
  });
});

describe('highlightSpansForLines', () => {
  it('returns plain lines when there is no range', () => {
    expect(highlightSpansForLines('a + b\nc', 0, undefined)).toEqual([[{ text: 'a + b' }], [{ text: 'c' }]]);
  });

  it('marks a range within a single line', () => {
    const spans = highlightSpansForLines('a + bad_ref', 0, { startByte: 4, endByte: 11, kind: 'error' });
    expect(spans).toEqual([[{ text: 'a + ' }, { text: 'bad_ref', error: true }]]);
  });

  it('marks a warning range', () => {
    const spans = highlightSpansForLines('x * y', 0, { startByte: 0, endByte: 1, kind: 'warning' });
    expect(spans).toEqual([[{ text: 'x', warning: true }, { text: ' * y' }]]);
  });

  it('marks a range on the second line of a multi-line equation', () => {
    // Range covers 'bad' on line 1 (offsets are into the whole string).
    const spans = highlightSpansForLines('a +\nbad + c', 0, { startByte: 4, endByte: 7, kind: 'error' });
    expect(spans).toEqual([[{ text: 'a +' }], [{ text: 'bad', error: true }, { text: ' + c' }]]);
  });

  it('marks a range spanning a newline across two lines', () => {
    const spans = highlightSpansForLines('ab\ncd', 0, { startByte: 1, endByte: 4, kind: 'error' });
    expect(spans).toEqual([
      [{ text: 'a' }, { text: 'b', error: true }],
      [{ text: 'c', error: true }, { text: 'd' }],
    ]);
  });

  it('shifts raw-equation offsets past the apply-to-all prefix', () => {
    const displayed = applyToAllPrefix + 'rate * 2';
    // Engine offsets are into the raw equation 'rate * 2'.
    const spans = highlightSpansForLines(displayed, applyToAllPrefix.length, {
      startByte: 0,
      endByte: 4,
      kind: 'error',
    });
    expect(spans).toEqual([[{ text: '{apply-to-all:}' }], [{ text: 'rate', error: true }, { text: ' * 2' }]]);
  });

  it('converts byte offsets in non-ASCII equations to UTF-16 indices', () => {
    // 'é' is 2 UTF-8 bytes: byte range [7, 8] of 'café + b' is just 'b'
    // (c=1,a=2,f=3,é=5,' '=6,+=7 ... offsets: after 'café + ' is byte 8).
    const spans = highlightSpansForLines('café + b', 0, { startByte: 8, endByte: 9, kind: 'error' });
    expect(spans).toEqual([[{ text: 'café + ' }, { text: 'b', error: true }]]);
  });
});

describe('highlightRangeForField', () => {
  const eqError = (start: number, end: number): EquationError => ({
    code: ErrorCode.UnknownDependency,
    start,
    end,
  });
  const codeForKind = (kind: UnitErrorKind): ErrorCode =>
    kind === 'definition' ? ErrorCode.NoAppInUnits : ErrorCode.UnitMismatch;
  const unitError = (kind: UnitErrorKind, start: number, end: number): UnitError => ({
    code: codeForKind(kind),
    start,
    end,
    kind,
    details: undefined,
  });

  it('marks an equation error range in the equation field', () => {
    const range = highlightRangeForField('a + bad', [eqError(4, 7)], undefined, false);
    expect(range).toEqual({ startByte: 4, endByte: 7, kind: 'error' });
  });

  it('returns no range for a zero-length equation error', () => {
    expect(highlightRangeForField('a + b', [eqError(0, 0)], undefined, false)).toBeUndefined();
  });

  it('equation errors take precedence over unit errors in the equation field', () => {
    const range = highlightRangeForField('a + bad', [eqError(4, 7)], [unitError('consistency', 0, 7)], false);
    expect(range).toEqual({ startByte: 4, endByte: 7, kind: 'error' });
  });

  it('underlines the whole units declaration for a consistency error', () => {
    // The consistency error's offsets point into the *equation*; the units
    // field underlines its entire declaration instead.
    const range = highlightRangeForField('blerz/second', undefined, [unitError('consistency', 0, 6)], true);
    expect(range).toEqual({ startByte: 0, endByte: Number.MAX_SAFE_INTEGER, kind: 'error' });
  });

  it('uses definition-error offsets in the units field', () => {
    const range = highlightRangeForField('bad(units)', undefined, [unitError('definition', 0, 3)], true);
    expect(range).toEqual({ startByte: 0, endByte: 3, kind: 'error' });
  });

  it('treats a definition error end of 0 as to-the-end', () => {
    const range = highlightRangeForField('bad', undefined, [unitError('definition', 1, 0)], true);
    expect(range).toEqual({ startByte: 1, endByte: Number.MAX_SAFE_INTEGER, kind: 'error' });
  });

  it('prefers definition offsets over a consistency underline in the units field', () => {
    const range = highlightRangeForField(
      'bad(units)',
      undefined,
      [unitError('consistency', 0, 6), unitError('definition', 0, 3)],
      true,
    );
    expect(range).toEqual({ startByte: 0, endByte: 3, kind: 'error' });
  });

  it('warns on the conflicting sub-expression in the equation field', () => {
    const range = highlightRangeForField('a + b*c', undefined, [unitError('consistency', 4, 7)], false);
    expect(range).toEqual({ startByte: 4, endByte: 7, kind: 'warning' });
  });

  it('suppresses a whole-equation consistency warning in the equation field', () => {
    // When the conflicting span is the entire equation the warning localizes
    // nothing; the units field carries the error underline instead.
    expect(highlightRangeForField('2*aux1', undefined, [unitError('consistency', 0, 6)], false)).toBeUndefined();
  });

  it('suppresses a to-the-end consistency warning that starts at 0', () => {
    expect(highlightRangeForField('2*aux1', undefined, [unitError('consistency', 0, 0)], false)).toBeUndefined();
  });

  it('ignores definition errors in the equation field', () => {
    expect(highlightRangeForField('a + b', undefined, [unitError('definition', 0, 3)], false)).toBeUndefined();
  });

  it('underlines nothing for an inference error in the units field', () => {
    // An inference error's offsets point into an equation (possibly another
    // variable's), never the units string; no span in this field is right.
    expect(highlightRangeForField('people/year', undefined, [unitError('inference', 2, 7)], true)).toBeUndefined();
  });

  it('underlines nothing for an inference error in the equation field', () => {
    expect(highlightRangeForField('a + b', undefined, [unitError('inference', 2, 5)], false)).toBeUndefined();
  });

  it('keeps the consistency underline when an inference error is also present', () => {
    const units = highlightRangeForField(
      'people',
      undefined,
      [unitError('inference', 2, 7), unitError('consistency', 0, 6)],
      true,
    );
    expect(units).toEqual({ startByte: 0, endByte: Number.MAX_SAFE_INTEGER, kind: 'error' });

    const eqn = highlightRangeForField(
      'a + b*c',
      undefined,
      [unitError('inference', 2, 7), unitError('consistency', 4, 7)],
      false,
    );
    expect(eqn).toEqual({ startByte: 4, endByte: 7, kind: 'warning' });
  });

  it('returns undefined when there are no errors', () => {
    expect(highlightRangeForField('a + b', undefined, undefined, false)).toBeUndefined();
    expect(highlightRangeForField('units', [], [], true)).toBeUndefined();
  });
});

describe('slatePointForOffset', () => {
  it('maps an offset on the first line', () => {
    expect(slatePointForOffset('abc\ndef', 2)).toEqual({ path: [0, 0], offset: 2 });
  });

  it('maps the end of the first line', () => {
    expect(slatePointForOffset('abc\ndef', 3)).toEqual({ path: [0, 0], offset: 3 });
  });

  it('maps an offset on the second line', () => {
    // offset 4 is the start of 'def' (the newline itself maps to line 1, col 0)
    expect(slatePointForOffset('abc\ndef', 4)).toEqual({ path: [1, 0], offset: 0 });
    expect(slatePointForOffset('abc\ndef', 6)).toEqual({ path: [1, 0], offset: 2 });
  });

  it('clamps past-the-end offsets to the document end', () => {
    expect(slatePointForOffset('abc\ndef', 99)).toEqual({ path: [1, 0], offset: 3 });
  });

  it('clamps negative offsets to the document start', () => {
    expect(slatePointForOffset('abc', -5)).toEqual({ path: [0, 0], offset: 0 });
  });
});
