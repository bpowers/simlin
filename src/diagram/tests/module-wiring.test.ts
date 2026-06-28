// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * @jest-environment node
 */

import type { ModuleReference, Module, Aux } from '@simlin/core/datamodel';

import {
  isDuplicateReference,
  addReference,
  removeReference,
  updateReferenceSrc,
  updateReferenceDst,
  getAvailableSrcVariables,
  qualifyDst,
  unqualifyDst,
  buildModuleReferencePayload,
} from '../module-wiring';

function makeModule(overrides: Partial<Module> = {}): Module {
  return {
    type: 'module',
    ident: 'hares_inst',
    modelName: 'hares',
    documentation: 'doc',
    units: 'widgets',
    references: [{ src: 'predators', dst: 'hares_inst·input_food' }],
    canBeModuleInput: true,
    isPublic: true,
    dataSource: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: 7,
    ...overrides,
  };
}

// -- Tests --

describe('qualifyDst', () => {
  it('prefixes a bare port with the module ident and the canonical separator', () => {
    expect(qualifyDst('hares_mod', 'input_food')).toBe('hares_mod·input_food');
  });

  it('keeps an empty port empty (placeholder rows are not dangling prefixes)', () => {
    expect(qualifyDst('hares_mod', '')).toBe('');
  });
});

describe('unqualifyDst', () => {
  it('recovers the bare port from a module-qualified dst', () => {
    expect(unqualifyDst('hares_mod·input_food')).toBe('input_food');
  });

  it('tolerates a legacy period separator (XMILE-imported, pre-patch)', () => {
    expect(unqualifyDst('hares_mod.input_food')).toBe('input_food');
  });

  it('returns an already-bare value unchanged', () => {
    expect(unqualifyDst('input_food')).toBe('input_food');
  });

  it('returns empty for empty input', () => {
    expect(unqualifyDst('')).toBe('');
  });

  it('round-trips with qualifyDst', () => {
    expect(unqualifyDst(qualifyDst('m', 'port'))).toBe('port');
  });
});

describe('isDuplicateReference', () => {
  it('returns false for empty references array', () => {
    expect(isDuplicateReference([], 'food', 'input_food')).toBe(false);
  });

  it('returns true when exact src/dst pair exists', () => {
    const refs: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
    expect(isDuplicateReference(refs, 'food', 'input_food')).toBe(true);
  });

  it('returns false when src matches but dst differs', () => {
    const refs: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
    expect(isDuplicateReference(refs, 'food', 'input_water')).toBe(false);
  });

  it('returns false when dst matches but src differs', () => {
    const refs: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
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
    const original: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
    const result = addReference(original, 'water', 'input_water');
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ src: 'food', dst: 'input_food' });
    expect(result[1]).toEqual({ src: 'water', dst: 'input_water' });
  });

  it('original array is not mutated', () => {
    const original: Array<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
    addReference(original, 'water', 'input_water');
    expect(original).toHaveLength(1);
    expect(original[0]).toEqual({ src: 'food', dst: 'input_food' });
  });

  it('adding duplicate (same non-empty src and dst) returns original array unchanged', () => {
    const original: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
    const result = addReference(original, 'food', 'input_food');
    expect(result).toBe(original);
  });

  it('adding with empty src allows addition (for new row placeholder)', () => {
    const original: ReadonlyArray<ModuleReference> = [{ src: '', dst: 'input_food' }];
    const result = addReference(original, '', 'input_food');
    expect(result).toHaveLength(2);
  });

  it('adding with empty dst allows addition (for new row placeholder)', () => {
    const original: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: '' }];
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
    const refs: ReadonlyArray<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
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
    const original: Array<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
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
    const original: Array<ModuleReference> = [{ src: 'food', dst: 'input_food' }];
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

describe('buildModuleReferencePayload', () => {
  it('preserves compat (canBeModuleInput/isPublic/dataSource) while re-pointing the model', () => {
    // upsertModule is a full-replace by UID, so re-pointing a module at a new
    // model must carry every field forward -- compat is the data-loss trap.
    const existing = makeModule({
      canBeModuleInput: true,
      isPublic: true,
      dataSource: { kind: 'data', file: 'd.csv', tabOrDelimiter: ',', rowOrCol: 'A', cell: 'B2' },
    });
    const payload = buildModuleReferencePayload(existing, 'hares_inst', 'hares_copy');
    expect(payload.name).toBe('hares_inst');
    expect(payload.modelName).toBe('hares_copy');
    expect(payload.compat?.canBeModuleInput).toBe(true);
    expect(payload.compat?.isPublic).toBe(true);
    expect(payload.compat?.dataSource).toEqual({
      kind: 'data',
      file: 'd.csv',
      tabOrDelimiter: ',',
      rowOrCol: 'A',
      cell: 'B2',
    });
    // existing references/units/documentation also carry forward
    expect(payload.references).toEqual([{ src: 'predators', dst: 'hares_inst·input_food' }]);
    expect(payload.units).toBe('widgets');
    expect(payload.documentation).toBe('doc');
  });

  it('returns a bare {name, modelName} when there is no existing module', () => {
    const payload = buildModuleReferencePayload(undefined, 'new_inst', 'new_model');
    expect(payload).toEqual({ name: 'new_inst', modelName: 'new_model' });
  });

  it('returns a bare payload when the existing variable is not a module', () => {
    const aux: Aux = {
      type: 'aux',
      ident: 'not_a_module',
      equation: { type: 'scalar', equation: '1' },
      documentation: '',
      units: '',
      gf: undefined,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 3,
    };
    const payload = buildModuleReferencePayload(aux, 'x', 'y');
    expect(payload).toEqual({ name: 'x', modelName: 'y' });
  });
});
