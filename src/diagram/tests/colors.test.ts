// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Test our in-tree Dark2 palette implementation
import { Dark2 } from '../colors';

describe('Dark2 palette', () => {
  it('has exactly 8 colors', () => {
    expect(Dark2).toHaveLength(8);
  });

  it('contains valid hex color codes', () => {
    const hexPattern = /^#[0-9a-f]{6}$/i;
    for (const color of Dark2) {
      expect(color).toMatch(hexPattern);
    }
  });

  it('contains the expected ColorBrewer Dark2 colors', () => {
    // These values were verified against chroma-js brewer.Dark2
    expect(Dark2[0]).toBe('#1b9e77'); // teal
    expect(Dark2[1]).toBe('#d95f02'); // orange
    expect(Dark2[2]).toBe('#7570b3'); // purple
    expect(Dark2[3]).toBe('#e7298a'); // pink
    expect(Dark2[4]).toBe('#66a61e'); // green
    expect(Dark2[5]).toBe('#e6ab02'); // yellow
    expect(Dark2[6]).toBe('#a6761d'); // brown
    expect(Dark2[7]).toBe('#666666'); // gray
  });
});
