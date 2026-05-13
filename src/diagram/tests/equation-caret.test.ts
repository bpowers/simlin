// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { alignGlyphsToSource, caretOffsetForClick, glyphToAscii, RenderedGlyph } from '../equation-caret';

// Build a single-line run of glyphs from a list of [char, left, width] tuples.
// All glyphs share a vertical band so the 2D-nearest-boundary search reduces
// to picking the nearest horizontal boundary.
function lineOf(specs: Array<[string, number, number]>, top = 0, height = 12): RenderedGlyph[] {
  return specs.map(([char, left, width]) => ({
    char,
    left,
    right: left + width,
    top,
    bottom: top + height,
  }));
}

// `Incidents*average_effort_required_to_remediate_an_incident`, the user's
// example. KaTeX renders `*` as `\cdot` (a small `⋅` glyph) flanked by ~6px of
// CSS spacing on each side, so a click "on the *" usually lands in that
// spacing rather than on the tiny dot. The glyph layout below mimics that:
// "Incidents" packed 10px/char ending at x=90, the `⋅` at x=98..102 (a 4px
// dot centred in a ~16px slot), then "average..." starting at x=114.
function incidentsTimesAverage(): { glyphs: RenderedGlyph[]; eq: string } {
  const eq = 'Incidents*average_effort_required_to_remediate_an_incident';
  const specs: Array<[string, number, number]> = [];
  // "Incidents" -> indices 0..8 at x = 0..90
  for (let i = 0; i < 9; i++) {
    specs.push([eq[i], i * 10, 10]);
  }
  // `*` -> rendered as `⋅` at x = 98..102
  specs.push(['⋅', 98, 4]);
  // "average_effort_..." -> indices 10..eq.length-1 starting at x = 114
  for (let i = 10; i < eq.length; i++) {
    specs.push([eq[i], 114 + (i - 10) * 10, 10]);
  }
  return { glyphs: lineOf(specs), eq };
}

describe('glyphToAscii', () => {
  it('maps the multiplication glyphs back to "*"', () => {
    expect(glyphToAscii('⋅')).toBe('*'); // \cdot
    expect(glyphToAscii('·')).toBe('*'); // middle dot
    expect(glyphToAscii('×')).toBe('*'); // \times
  });

  it('maps the unary/binary minus sign back to "-"', () => {
    expect(glyphToAscii('−')).toBe('-'); // U+2212
  });

  it('maps the negation sign back to "!"', () => {
    expect(glyphToAscii('¬')).toBe('!'); // \neg
  });

  it('passes ordinary characters through unchanged', () => {
    expect(glyphToAscii('a')).toBe('a');
    expect(glyphToAscii('(')).toBe('(');
    expect(glyphToAscii('+')).toBe('+');
    expect(glyphToAscii('=')).toBe('=');
  });
});

describe('alignGlyphsToSource', () => {
  it('is the identity for a glyph stream that matches the source character-for-character', () => {
    const eq = 'a+b';
    const glyphs = lineOf([
      ['a', 0, 10],
      ['+', 10, 10],
      ['b', 20, 10],
    ]);
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 1, 2, 3]);
  });

  it('maps the `\\cdot` glyph back onto the `*` in the source', () => {
    const { glyphs, eq } = incidentsTimesAverage();
    const m = alignGlyphsToSource(glyphs, eq);
    // glyph 9 is the `⋅`; "caret before it" is offset 9 (right before the `*`),
    // "caret before glyph 10" is offset 10 (right after the `*`).
    expect(m[9]).toBe(9);
    expect(m[10]).toBe(10);
    expect(m[0]).toBe(0);
    expect(m[m.length - 1]).toBe(eq.length);
  });

  it('matches case-insensitively (LaTeX lower-cases identifiers)', () => {
    const eq = 'Foo*Bar';
    const glyphs = lineOf([
      ['f', 0, 10],
      ['o', 10, 10],
      ['o', 20, 10],
      ['⋅', 30, 4],
      ['b', 40, 10],
      ['a', 50, 10],
      ['r', 60, 10],
    ]);
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
  });

  it('skips source whitespace that the rendered math drops', () => {
    const eq = 'a + b';
    const glyphs = lineOf([
      ['a', 0, 10],
      ['+', 10, 10],
      ['b', 20, 10],
    ]);
    // glyph 0 -> source 0 (a), glyph 1 -> source 2 (+), glyph 2 -> source 4 (b)
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 2, 4, 5]);
  });

  it('resyncs across a `/` that rendered as a fraction bar', () => {
    const eq = 'a/b';
    // KaTeX stacks `a` over `b`; there is no `/` glyph.
    const glyphs: RenderedGlyph[] = [
      { char: 'a', left: 8, right: 12, top: 0, bottom: 10 },
      { char: 'b', left: 8, right: 12, top: 12, bottom: 22 },
    ];
    // glyph 0 -> source 0 (a); glyph 1 -> source 2 (b, after skipping `/`)
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 2, 3]);
  });

  it('resyncs across a `^` that rendered as a superscript', () => {
    const eq = 'a^bc';
    const glyphs: RenderedGlyph[] = [
      { char: 'a', left: 0, right: 10, top: 4, bottom: 14 },
      { char: 'b', left: 10, right: 16, top: 0, bottom: 8 },
      { char: 'c', left: 16, right: 22, top: 0, bottom: 8 },
    ];
    // a -> 0, b -> 2 (after skipping `^`), c -> 3
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 2, 3, 4]);
  });

  it('treats a glyph absent from the source as occupying no characters', () => {
    // pretend KaTeX added a paren for precedence that the user did not type
    const eq = 'a*b';
    const glyphs = lineOf([
      ['a', 0, 10],
      ['(', 10, 6],
      ['*', 16, 6],
      ['b', 22, 10],
    ]);
    // the stray `(` maps to the position before the `*` and consumes nothing
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 1, 1, 2, 3]);
  });
});

describe('caretOffsetForClick', () => {
  it('returns 0 when there are no glyphs (caller falls back proportionally)', () => {
    expect(caretOffsetForClick([], 100, 50, 'anything')).toBe(0);
  });

  it('places the caret right at the `*` when clicking the rendered multiplication dot', () => {
    const { glyphs, eq } = incidentsTimesAverage();
    // dead-centre on the `⋅` (x in 98..102) -> just before the `*`
    expect(caretOffsetForClick(glyphs, 100, 6, eq)).toBe(9);
    // right edge of the `⋅` and into its right margin -> just after the `*`
    expect(caretOffsetForClick(glyphs, 108, 6, eq)).toBe(10);
    // left margin of the `⋅`, between "Incidents" and the dot -> just before
    expect(caretOffsetForClick(glyphs, 94, 6, eq)).toBe(9);
  });

  it('does not snap a click near the `*` into the middle of the next identifier', () => {
    const { glyphs, eq } = incidentsTimesAverage();
    // The bug: clicking near the `*` used to land one character into
    // "average...". The caret must stay adjacent to the `*` (offset 9 or 10).
    for (const x of [92, 96, 100, 104, 108, 112]) {
      const off = caretOffsetForClick(glyphs, x, 6, eq);
      expect(off === 9 || off === 10).toBe(true);
    }
  });

  it('places the caret before/after an identifier character based on which half was clicked', () => {
    const eq = 'abc*d';
    const glyphs = lineOf([
      ['a', 0, 10],
      ['b', 10, 10],
      ['c', 20, 10],
      ['⋅', 30, 4],
      ['d', 38, 10],
    ]);
    expect(caretOffsetForClick(glyphs, 12, 6, eq)).toBe(1); // left half of `b` -> before it
    expect(caretOffsetForClick(glyphs, 18, 6, eq)).toBe(2); // right half of `b` -> after it
    expect(caretOffsetForClick(glyphs, -100, 6, eq)).toBe(0); // far left -> start
    expect(caretOffsetForClick(glyphs, 9999, 6, eq)).toBe(eq.length); // far right -> end
  });

  it('uses the click Y to pick a wrapped line', () => {
    // Two visual lines: "ab" on top, "cd" below, source "ab+cd".
    const eq = 'ab+cd';
    const glyphs: RenderedGlyph[] = [
      { char: 'a', left: 0, right: 10, top: 0, bottom: 12 },
      { char: 'b', left: 10, right: 20, top: 0, bottom: 12 },
      { char: '+', left: 20, right: 30, top: 0, bottom: 12 },
      { char: 'c', left: 0, right: 10, top: 20, bottom: 32 },
      { char: 'd', left: 10, right: 20, top: 20, bottom: 32 },
    ];
    // Click near the start of the second line -> before `c` (source offset 3).
    expect(caretOffsetForClick(glyphs, 2, 26, eq)).toBe(3);
    // Click near the end of the first line -> after `b` (source offset 2),
    // which the `+` glyph occupies, so this lands right before the `+`.
    expect(caretOffsetForClick(glyphs, 18, 6, eq)).toBe(2);
  });
});
