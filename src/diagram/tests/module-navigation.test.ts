// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * @jest-environment node
 */

import type { Rect, UID } from '@simlin/core/datamodel';

import {
  breadcrumbSegments,
  currentModelName,
  isStdlibModel,
  navigateToLevel,
  popModule,
  pushModule,
} from '../module-navigation';
import type { ModuleStackEntry } from '../module-navigation';

function makeRect(x: number, y: number, width: number, height: number): Rect {
  return { x, y, width, height };
}

function makeEntry(modelName: string, moduleIdent: string, selectionItems: ReadonlyArray<UID> = []): ModuleStackEntry {
  return {
    modelName,
    moduleIdent,
    selection: new Set(selectionItems),
    viewBox: makeRect(0, 0, 800, 600),
    zoom: 1,
  };
}

describe('currentModelName', () => {
  it('returns "main" for empty stack', () => {
    expect(currentModelName([])).toBe('main');
  });

  it('returns modelName from single-entry stack', () => {
    const stack = [makeEntry('hares', 'hares')];
    expect(currentModelName(stack)).toBe('hares');
  });

  it('returns last entry modelName for multi-entry stack', () => {
    const stack = [makeEntry('hares', 'hares'), makeEntry('sub_pop', 'sub_pop')];
    expect(currentModelName(stack)).toBe('sub_pop');
  });
});

describe('pushModule', () => {
  it('creates single-entry stack when pushing onto empty stack', () => {
    const newStack = pushModule([], 'hares', 'hares', new Set([1, 2]), makeRect(10, 20, 800, 600), 1.5);
    expect(newStack).toHaveLength(1);
    expect(newStack[0].modelName).toBe('hares');
    expect(newStack[0].moduleIdent).toBe('hares');
    expect(newStack[0].selection).toEqual(new Set([1, 2]));
    expect(newStack[0].viewBox).toEqual(makeRect(10, 20, 800, 600));
    expect(newStack[0].zoom).toBe(1.5);
  });

  it('appends entry when pushing onto existing stack', () => {
    const initial = [makeEntry('hares', 'hares')];
    const newStack = pushModule(initial, 'sub_pop', 'sub_pop', new Set([5]), makeRect(0, 0, 1200, 900), 2.0);
    expect(newStack).toHaveLength(2);
    expect(newStack[0]).toBe(initial[0]);
    expect(newStack[1].modelName).toBe('sub_pop');
    expect(newStack[1].moduleIdent).toBe('sub_pop');
  });

  it('captures selection, viewBox, zoom correctly in entry', () => {
    const selection = new Set<UID>([10, 20, 30]);
    const viewBox = makeRect(100, 200, 1600, 1200);
    const zoom = 0.75;
    const newStack = pushModule([], 'deep_model', 'deep_mod', selection, viewBox, zoom);
    expect(newStack[0].selection).toEqual(selection);
    expect(newStack[0].viewBox).toEqual(viewBox);
    expect(newStack[0].zoom).toBe(zoom);
  });

  it('does not mutate the original stack (immutability)', () => {
    const original: ReadonlyArray<ModuleStackEntry> = [makeEntry('hares', 'hares')];
    const originalCopy = [...original];
    pushModule(original, 'sub_pop', 'sub_pop', new Set(), makeRect(0, 0, 800, 600), 1);
    expect(original).toEqual(originalCopy);
    expect(original).toHaveLength(1);
  });
});

describe('popModule', () => {
  it('pops single-entry stack and returns empty stack with "main" as restoredModelName', () => {
    const selection = new Set<UID>([1, 2]);
    const viewBox = makeRect(50, 60, 800, 600);
    const zoom = 1.25;
    const stack = [
      {
        modelName: 'hares',
        moduleIdent: 'hares',
        selection,
        viewBox,
        zoom,
      },
    ];

    const result = popModule(stack);
    expect(result.newStack).toHaveLength(0);
    expect(result.restoredModelName).toBe('main');
    expect(result.restoredSelection).toEqual(selection);
    expect(result.restoredViewBox).toEqual(viewBox);
    expect(result.restoredZoom).toBe(zoom);
  });

  it('pops multi-entry stack and returns correct parent model name', () => {
    const stack = [makeEntry('hares', 'hares'), makeEntry('sub_pop', 'sub_pop')];
    const result = popModule(stack);
    expect(result.newStack).toHaveLength(1);
    expect(result.restoredModelName).toBe('hares');
  });

  it('restores selection, viewBox, zoom from popped entry', () => {
    const selection = new Set<UID>([42]);
    const viewBox = makeRect(100, 200, 1000, 800);
    const zoom = 2.5;
    const stack: ReadonlyArray<ModuleStackEntry> = [
      makeEntry('hares', 'hares'),
      {
        modelName: 'sub_pop',
        moduleIdent: 'sub_pop',
        selection,
        viewBox,
        zoom,
      },
    ];

    const result = popModule(stack);
    expect(result.restoredSelection).toEqual(selection);
    expect(result.restoredViewBox).toEqual(viewBox);
    expect(result.restoredZoom).toBe(zoom);
  });

  it('throws when stack is empty', () => {
    expect(() => popModule([])).toThrow();
  });
});

describe('navigateToLevel', () => {
  // AC3.4: Level 0 from depth 2 restores main state, stack becomes empty
  it('navigates to level 0 from depth 2, returning empty stack', () => {
    const rootSelection = new Set<UID>([1, 2, 3]);
    const rootViewBox = makeRect(0, 0, 800, 600);
    const rootZoom = 1.0;
    const stack: ReadonlyArray<ModuleStackEntry> = [
      {
        modelName: 'hares',
        moduleIdent: 'hares',
        selection: rootSelection,
        viewBox: rootViewBox,
        zoom: rootZoom,
      },
      makeEntry('sub_pop', 'sub_pop'),
    ];

    const result = navigateToLevel(stack, 0);
    expect(result.newStack).toHaveLength(0);
    expect(result.restoredModelName).toBe('main');
    expect(result.restoredSelection).toEqual(rootSelection);
    expect(result.restoredViewBox).toEqual(rootViewBox);
    expect(result.restoredZoom).toBe(rootZoom);
  });

  it('navigates to level 1 from depth 3, returning stack of length 1', () => {
    const level1Selection = new Set<UID>([10]);
    const level1ViewBox = makeRect(50, 50, 900, 700);
    const level1Zoom = 1.5;
    const stack: ReadonlyArray<ModuleStackEntry> = [
      makeEntry('hares', 'hares'),
      {
        modelName: 'sub_pop',
        moduleIdent: 'sub_pop',
        selection: level1Selection,
        viewBox: level1ViewBox,
        zoom: level1Zoom,
      },
      makeEntry('deep_level', 'deep_level'),
    ];

    const result = navigateToLevel(stack, 1);
    expect(result.newStack).toHaveLength(1);
    expect(result.restoredModelName).toBe('hares');
    expect(result.restoredSelection).toEqual(level1Selection);
    expect(result.restoredViewBox).toEqual(level1ViewBox);
    expect(result.restoredZoom).toBe(level1Zoom);
  });

  it('throws when navigating to current level (targetLevel === stack.length)', () => {
    const stack = [makeEntry('hares', 'hares')];
    expect(() => navigateToLevel(stack, 1)).toThrow();
  });

  it('throws for negative level', () => {
    const stack = [makeEntry('hares', 'hares')];
    expect(() => navigateToLevel(stack, -1)).toThrow();
  });

  it('throws for out-of-range positive level (targetLevel > stack.length)', () => {
    const stack = [makeEntry('hares', 'hares')];
    expect(() => navigateToLevel(stack, 5)).toThrow();
  });

  it('throws when stack is empty', () => {
    expect(() => navigateToLevel([], 0)).toThrow();
  });
});

describe('breadcrumbSegments', () => {
  // AC3.3: Empty stack returns just main
  it('returns only main segment for empty stack', () => {
    const segments = breadcrumbSegments([]);
    expect(segments).toEqual([{ label: 'main', level: 0 }]);
  });

  it('returns main + one segment for single-entry stack', () => {
    const stack = [makeEntry('hares', 'hares')];
    const segments = breadcrumbSegments(stack);
    expect(segments).toEqual([
      { label: 'main', level: 0 },
      { label: 'hares', level: 1 },
    ]);
  });

  // AC3.7: Three-entry stack returns main + three segments (3+ levels)
  it('returns main + three segments for three-entry stack', () => {
    const stack = [makeEntry('hares', 'hares'), makeEntry('sub_pop', 'sub_pop'), makeEntry('nested', 'nested')];
    const segments = breadcrumbSegments(stack);
    expect(segments).toEqual([
      { label: 'main', level: 0 },
      { label: 'hares', level: 1 },
      { label: 'sub_pop', level: 2 },
      { label: 'nested', level: 3 },
    ]);
  });
});

describe('isStdlibModel', () => {
  // AC3.8: Returns true for each of the 9 stdlib model names
  const stdlibModels = [
    'delay1',
    'delay3',
    'npv',
    'smth1',
    'smth3',
    'systems_conversion',
    'systems_leak',
    'systems_rate',
    'trend',
  ];

  for (const name of stdlibModels) {
    it(`returns true for stdlib model "${name}"`, () => {
      expect(isStdlibModel(name)).toBe(true);
    });
  }

  it('returns false for user model "hares"', () => {
    expect(isStdlibModel('hares')).toBe(false);
  });

  it('returns false for "main"', () => {
    expect(isStdlibModel('main')).toBe(false);
  });

  it('returns false for empty string', () => {
    expect(isStdlibModel('')).toBe(false);
  });
});

describe('selection/viewport restoration (AC3.5)', () => {
  it('push with specific state then pop restores exactly', () => {
    const sel = new Set<UID>([7, 8, 9]);
    const vb = makeRect(111, 222, 333, 444);
    const z = 3.14;
    const stack = pushModule([], 'target_model', 'my_module', sel, vb, z);

    const result = popModule(stack);
    expect(result.restoredSelection).toEqual(sel);
    expect(result.restoredViewBox).toEqual(vb);
    expect(result.restoredZoom).toBe(z);
  });

  it('restores intermediate and root state in 3-level deep stack', () => {
    const rootSel = new Set<UID>([1]);
    const rootVb = makeRect(0, 0, 100, 100);
    const rootZoom = 1.0;

    const midSel = new Set<UID>([2, 3]);
    const midVb = makeRect(10, 10, 200, 200);
    const midZoom = 2.0;

    // Push level 1 (capturing root state)
    const stack1 = pushModule([], 'level1', 'mod1', rootSel, rootVb, rootZoom);

    // Push level 2 (capturing level 1 state)
    const stack2 = pushModule(stack1, 'level2', 'mod2', midSel, midVb, midZoom);

    // Pop once: should restore level 1 (mid) state
    const result1 = popModule(stack2);
    expect(result1.restoredModelName).toBe('level1');
    expect(result1.restoredSelection).toEqual(midSel);
    expect(result1.restoredViewBox).toEqual(midVb);
    expect(result1.restoredZoom).toBe(midZoom);

    // Pop again: should restore root state
    const result2 = popModule(result1.newStack);
    expect(result2.restoredModelName).toBe('main');
    expect(result2.restoredSelection).toEqual(rootSel);
    expect(result2.restoredViewBox).toEqual(rootVb);
    expect(result2.restoredZoom).toBe(rootZoom);
  });
});

describe('no depth special-casing (AC5.4)', () => {
  it('pushModule produces identical structural result at any depth', () => {
    const sel = new Set<UID>([99]);
    const vb = makeRect(0, 0, 800, 600);
    const z = 1.0;

    // Push at depth 0 (empty stack)
    const stack1 = pushModule([], 'model_a', 'mod_a', sel, vb, z);
    expect(stack1).toHaveLength(1);
    expect(stack1[0].modelName).toBe('model_a');
    expect(stack1[0].selection).toEqual(sel);

    // Push at depth 1
    const stack2 = pushModule(stack1, 'model_b', 'mod_b', sel, vb, z);
    expect(stack2).toHaveLength(2);
    expect(stack2[1].modelName).toBe('model_b');
    expect(stack2[1].selection).toEqual(sel);

    // Push at depth 2
    const stack3 = pushModule(stack2, 'model_c', 'mod_c', sel, vb, z);
    expect(stack3).toHaveLength(3);
    expect(stack3[2].modelName).toBe('model_c');
    expect(stack3[2].selection).toEqual(sel);
  });

  it('popModule produces identical structural result at any depth', () => {
    const sel = new Set<UID>([99]);
    const vb = makeRect(0, 0, 800, 600);
    const z = 1.0;

    const stack1 = pushModule([], 'a', 'a', sel, vb, z);
    const stack2 = pushModule(stack1, 'b', 'b', sel, vb, z);
    const stack3 = pushModule(stack2, 'c', 'c', sel, vb, z);

    // Pop from depth 3
    const r3 = popModule(stack3);
    expect(r3.newStack).toHaveLength(2);
    expect(r3.restoredSelection).toEqual(sel);

    // Pop from depth 2
    const r2 = popModule(r3.newStack);
    expect(r2.newStack).toHaveLength(1);
    expect(r2.restoredSelection).toEqual(sel);

    // Pop from depth 1
    const r1 = popModule(r2.newStack);
    expect(r1.newStack).toHaveLength(0);
    expect(r1.restoredSelection).toEqual(sel);
  });
});
