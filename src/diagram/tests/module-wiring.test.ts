// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * @jest-environment node
 */

import type { ModuleReference } from '@simlin/core/datamodel';

import {
  isDuplicateReference,
  addReference,
  removeReference,
  updateReferenceSrc,
  updateReferenceDst,
  getAvailableSrcVariables,
} from '../module-wiring';

// -- Tests --

describe('isDuplicateReference', () => {
  it('returns false for empty references array', () => {
    expect(isDuplicateReference([], 'food', 'input_food')).toBe(false);
  });

  it('returns true when exact src/dst pair exists', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    expect(isDuplicateReference(refs, 'food', 'input_food')).toBe(true);
  });

  it('returns false when src matches but dst differs', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    expect(isDuplicateReference(refs, 'food', 'input_water')).toBe(false);
  });

  it('returns false when dst matches but src differs', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    expect(isDuplicateReference(refs, 'water', 'input_food')).toBe(false);
  });
});

describe('addReference', () => {
  it('adding to empty array creates single-element array with correct src/dst', () => {
    const result = addReference([], 'food', 'input_food');
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ src: 'food', dst: 'input_food' });
  });

  it('adding to existing array appends without modifying original', () => {
    const original: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    const result = addReference(original, 'water', 'input_water');
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ src: 'food', dst: 'input_food' });
    expect(result[1]).toEqual({ src: 'water', dst: 'input_water' });
  });

  it('original array is not mutated', () => {
    const original: Array<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    addReference(original, 'water', 'input_water');
    expect(original).toHaveLength(1);
    expect(original[0]).toEqual({ src: 'food', dst: 'input_food' });
  });

  it('adding duplicate (same non-empty src and dst) returns original array unchanged', () => {
    const original: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    const result = addReference(original, 'food', 'input_food');
    expect(result).toBe(original);
  });

  it('adding with empty src allows addition (for new row placeholder)', () => {
    const original: ReadonlyArray<ModuleReference> = [
      { src: '', dst: 'input_food' },
    ];
    const result = addReference(original, '', 'input_food');
    expect(result).toHaveLength(2);
  });

  it('adding with empty dst allows addition (for new row placeholder)', () => {
    const original: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: '' },
    ];
    const result = addReference(original, 'food', '');
    expect(result).toHaveLength(2);
  });

  it('adding with both empty src and dst allows addition', () => {
    const result = addReference([], '', '');
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ src: '', dst: '' });
  });
});

describe('removeReference', () => {
  it('removing from single-element array returns empty array', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    const result = removeReference(refs, 0);
    expect(result).toHaveLength(0);
  });

  it('removing from multi-element array preserves other elements in order', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
      { src: 'shelter', dst: 'input_shelter' },
    ];
    const result = removeReference(refs, 1);
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ src: 'food', dst: 'input_food' });
    expect(result[1]).toEqual({ src: 'shelter', dst: 'input_shelter' });
  });

  it('removing at index 0 removes first element', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
    ];
    const result = removeReference(refs, 0);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ src: 'water', dst: 'input_water' });
  });

  it('original array is not mutated', () => {
    const original: Array<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
    ];
    removeReference(original, 0);
    expect(original).toHaveLength(2);
    expect(original[0]).toEqual({ src: 'food', dst: 'input_food' });
  });
});

describe('updateReferenceSrc', () => {
  it('updates only the target index', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
    ];
    const result = updateReferenceSrc(refs, 0, 'grain');
    expect(result[0]).toEqual({ src: 'grain', dst: 'input_food' });
  });

  it('other elements are unchanged', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
    ];
    const result = updateReferenceSrc(refs, 0, 'grain');
    expect(result[1]).toEqual({ src: 'water', dst: 'input_water' });
  });

  it('original array is not mutated', () => {
    const original: Array<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    updateReferenceSrc(original, 0, 'grain');
    expect(original[0]).toEqual({ src: 'food', dst: 'input_food' });
  });
});

describe('updateReferenceDst', () => {
  it('updates only the target index', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
    ];
    const result = updateReferenceDst(refs, 1, 'input_drink');
    expect(result[1]).toEqual({ src: 'water', dst: 'input_drink' });
  });

  it('other elements are unchanged', () => {
    const refs: ReadonlyArray<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
      { src: 'water', dst: 'input_water' },
    ];
    const result = updateReferenceDst(refs, 1, 'input_drink');
    expect(result[0]).toEqual({ src: 'food', dst: 'input_food' });
  });

  it('original array is not mutated', () => {
    const original: Array<ModuleReference> = [
      { src: 'food', dst: 'input_food' },
    ];
    updateReferenceDst(original, 0, 'input_grain');
    expect(original[0]).toEqual({ src: 'food', dst: 'input_food' });
  });
});

describe('getAvailableSrcVariables', () => {
  it('returns stocks, flows, and auxes', () => {
    const vars = new Map<string, { type: string; ident: string }>([
      ['alpha', { type: 'aux', ident: 'alpha' }],
      ['beta', { type: 'stock', ident: 'beta' }],
      ['gamma', { type: 'flow', ident: 'gamma' }],
    ]);
    const result = getAvailableSrcVariables(vars);
    expect(result).toContain('alpha');
    expect(result).toContain('beta');
    expect(result).toContain('gamma');
    expect(result).toHaveLength(3);
  });

  it('excludes modules', () => {
    const vars = new Map<string, { type: string; ident: string }>([
      ['alpha', { type: 'aux', ident: 'alpha' }],
      ['sub_mod', { type: 'module', ident: 'sub_mod' }],
    ]);
    const result = getAvailableSrcVariables(vars);
    expect(result).toContain('alpha');
    expect(result).not.toContain('sub_mod');
    expect(result).toHaveLength(1);
  });

  it('returns sorted list', () => {
    const vars = new Map<string, { type: string; ident: string }>([
      ['zebra', { type: 'aux', ident: 'zebra' }],
      ['alpha', { type: 'stock', ident: 'alpha' }],
      ['middle', { type: 'flow', ident: 'middle' }],
    ]);
    const result = getAvailableSrcVariables(vars);
    expect(result).toEqual(['alpha', 'middle', 'zebra']);
  });

  it('empty variables map returns empty array', () => {
    const vars = new Map<string, { type: string; ident: string }>();
    const result = getAvailableSrcVariables(vars);
    expect(result).toEqual([]);
  });
});
