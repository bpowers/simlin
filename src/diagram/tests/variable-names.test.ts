// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import { allocateVariableName, nameCollisionError } from '../variable-names';

describe('allocateVariableName', () => {
  it('returns the base name when it is free', () => {
    expect(allocateVariableName('New Variable', new Set(['stock_a']))).toBe('New Variable');
  });

  it('suffixes past every used candidate, matching canonically', () => {
    expect(allocateVariableName('New Variable', new Set(['new_variable', 'new_variable_1']))).toBe('New Variable 2');
  });

  it('gives up at the cap and returns the base name (the commit then reports the collision)', () => {
    const used = new Set<string>(['new_flow']);
    for (let i = 1; i < 1024; i++) {
      used.add(`new_flow_${i}`);
    }
    expect(allocateVariableName('New Flow', used)).toBe('New Flow');
  });
});

describe('nameCollisionError', () => {
  const used = new Set(['stock_a', 'aux_x']);

  it('a free name is not a collision', () => {
    expect(nameCollisionError('Brand New', undefined, used)).toBeUndefined();
  });

  it('a used name is a collision for a create (no current ident)', () => {
    expect(nameCollisionError('Stock A', undefined, used)).toContain('Stock A');
  });

  it('a canonical match is a collision', () => {
    expect(nameCollisionError('STOCK a', undefined, used)).toBeDefined();
  });

  it('a rename onto another used name is a collision', () => {
    expect(nameCollisionError('Stock A', 'aux_x', used)).toBeDefined();
  });

  it("a rename to the element's own ident (a case-only rename) is not a collision", () => {
    expect(nameCollisionError('AUX X', 'aux_x', used)).toBeUndefined();
  });
});
