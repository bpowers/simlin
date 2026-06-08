// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { disambiguatedLabels } from './disambiguate';

describe('disambiguatedLabels', () => {
  test('returns the bare basename when no collision exists', () => {
    const items = [{ path: 'a/x.stmx' }, { path: 'b/y.stmx' }, { path: 'z.xmile' }];
    const out = disambiguatedLabels(items);
    expect(out.map((e) => e.label)).toEqual(['x.stmx', 'y.stmx', 'z.xmile']);
  });

  test('returns the full relative path when basenames collide', () => {
    const items = [{ path: 'a/x.stmx' }, { path: 'b/x.stmx' }, { path: 'y.xmile' }];
    const out = disambiguatedLabels(items);
    expect(out.map((e) => e.label)).toEqual(['a/x.stmx', 'b/x.stmx', 'y.xmile']);
  });

  test('handles three-way collisions by rendering all three full paths', () => {
    const items = [{ path: 'a/x' }, { path: 'b/x' }, { path: 'c/x' }];
    const out = disambiguatedLabels(items);
    expect(out.map((e) => e.label)).toEqual(['a/x', 'b/x', 'c/x']);
  });

  test('returns an empty array for an empty input', () => {
    expect(disambiguatedLabels([])).toEqual([]);
  });

  test('preserves the original item reference on each entry', () => {
    const items = [
      { path: 'a/x.stmx', extra: 'one' as const },
      { path: 'b/x.stmx', extra: 'two' as const },
    ];
    const out = disambiguatedLabels(items);
    expect(out[0].item).toBe(items[0]);
    expect(out[1].item).toBe(items[1]);
  });

  test('treats top-level vs nested files with the same basename as colliding', () => {
    // A bare top-level "x.stmx" colliding with "subdir/x.stmx" should render
    // the full path for the nested one and the bare basename for the top
    // level — but both share the basename "x.stmx" so both must be qualified
    // (otherwise the user can't tell which row corresponds to which file).
    const items = [{ path: 'x.stmx' }, { path: 'subdir/x.stmx' }];
    const out = disambiguatedLabels(items);
    expect(out.map((e) => e.label)).toEqual(['x.stmx', 'subdir/x.stmx']);
  });

  test('does not mutate the input array', () => {
    const items = [{ path: 'a/x.stmx' }, { path: 'b/x.stmx' }];
    const snapshot = items.map((i) => ({ ...i }));
    disambiguatedLabels(items);
    expect(items).toEqual(snapshot);
  });
});
