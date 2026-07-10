// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect } from '@rstest/core';

import { formatSimSpecValue, resolveSimSpecDraft } from '../sim-spec-draft';

describe('formatSimSpecValue', () => {
  test('renders numbers as their default string form', () => {
    expect(formatSimSpecValue(0)).toBe('0');
    expect(formatSimSpecValue(100)).toBe('100');
    expect(formatSimSpecValue(0.25)).toBe('0.25');
  });

  test('passes strings through unchanged', () => {
    expect(formatSimSpecValue('years')).toBe('years');
    expect(formatSimSpecValue('')).toBe('');
  });
});

describe('resolveSimSpecDraft numeric fields', () => {
  test('commits a changed, finite value', () => {
    expect(resolveSimSpecDraft('startTime', '1900', 0)).toEqual({ shouldCommit: true, value: 1900 });
    expect(resolveSimSpecDraft('stopTime', '50', 100)).toEqual({ shouldCommit: true, value: 50 });
  });

  test('tolerates surrounding whitespace', () => {
    expect(resolveSimSpecDraft('startTime', '  12 ', 0)).toEqual({ shouldCommit: true, value: 12 });
  });

  test('does not commit an unchanged value', () => {
    expect(resolveSimSpecDraft('startTime', '0', 0)).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('stopTime', '100', 100)).toEqual({ shouldCommit: false });
  });

  test('rejects empty input (Number("") is 0, so this must be special-cased)', () => {
    expect(resolveSimSpecDraft('startTime', '', 5)).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('startTime', '   ', 5)).toEqual({ shouldCommit: false });
  });

  test('rejects non-numeric garbage', () => {
    expect(resolveSimSpecDraft('startTime', 'abc', 5)).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('startTime', '-', 5)).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('stopTime', '1.2.3', 5)).toEqual({ shouldCommit: false });
  });

  test('rejects non-finite values', () => {
    expect(resolveSimSpecDraft('startTime', 'Infinity', 5)).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('startTime', 'NaN', 5)).toEqual({ shouldCommit: false });
  });

  test('accepts a negative start time (the engine, not the drawer, owns start<stop)', () => {
    expect(resolveSimSpecDraft('startTime', '-10', 0)).toEqual({ shouldCommit: true, value: -10 });
  });
});

describe('resolveSimSpecDraft dt', () => {
  test('commits a positive dt', () => {
    expect(resolveSimSpecDraft('dt', '0.5', 1)).toEqual({ shouldCommit: true, value: 0.5 });
  });

  test('rejects a non-positive dt', () => {
    expect(resolveSimSpecDraft('dt', '0', 1)).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('dt', '-1', 1)).toEqual({ shouldCommit: false });
  });
});

describe('resolveSimSpecDraft timeUnits', () => {
  test('commits a changed free string', () => {
    expect(resolveSimSpecDraft('timeUnits', 'months', 'years')).toEqual({ shouldCommit: true, value: 'months' });
  });

  test('allows clearing to an empty string', () => {
    expect(resolveSimSpecDraft('timeUnits', '', 'years')).toEqual({ shouldCommit: true, value: '' });
  });

  test('does not commit an unchanged string', () => {
    expect(resolveSimSpecDraft('timeUnits', 'years', 'years')).toEqual({ shouldCommit: false });
    expect(resolveSimSpecDraft('timeUnits', '', '')).toEqual({ shouldCommit: false });
  });
});
