// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  alignGlyphsToSource,
  caretOffsetForClick,
  caretOffsetWithinSpan,
  chooseSpanForClick,
  glyphToAscii,
  nearestGlyphBoundary,
  RenderedGlyph,
  SpanBox,
} from '../equation-caret';

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

  it('maps the negation sign back to the `not` keyword', () => {
    // KaTeX draws `\neg` (the engine's rendering of `not`) as a single `¬`
    // glyph, but XMILE spells logical negation as the word `not` -- there is
    // no `!` operator in Simlin equations -- so the glyph stands for the
    // three-letter keyword.
    expect(glyphToAscii('¬')).toBe('not'); // \neg
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

  it('consumes the `not` keyword for the `¬` glyph that `\\neg` renders', () => {
    // `not running` -> `\neg \mathrm{running}` -> a single `¬` glyph followed
    // by the identifier glyphs. The `¬` stands for the 3-char `not` keyword;
    // its trailing space is then skipped before the operand.
    const eq = 'not running';
    const glyphs = lineOf([
      ['¬', 0, 12],
      ['r', 12, 10],
      ['u', 22, 10],
      ['n', 32, 10],
      ['n', 42, 10],
      ['i', 52, 10],
      ['n', 62, 10],
      ['g', 72, 10],
    ]);
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 4, 5, 6, 7, 8, 9, 10, 11]);
  });

  it('resyncs the operand past parentheses when `not` has no space (`not(x)`)', () => {
    const eq = 'not(running)';
    const glyphs = lineOf([
      ['¬', 0, 12],
      ['r', 12, 10],
      ['u', 22, 10],
      ['n', 32, 10],
      ['n', 42, 10],
      ['i', 52, 10],
      ['n', 62, 10],
      ['g', 72, 10],
    ]);
    // `¬` -> "not"; the `(` is skipped to land `r` on its source index; the
    // trailing `)` falls outside any glyph so the end maps to eq.length.
    expect(alignGlyphsToSource(glyphs, eq)).toEqual([0, 4, 5, 6, 7, 8, 9, 10, 12]);
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

  it('places the caret inside the operand of `not <ident>`, not at offset 0', () => {
    // Regression: `\neg` renders as one `¬` glyph for the 3-char `not`
    // keyword; before this was handled, the alignment stalled and clicks on
    // the operand landed at offset 0 (or the end of the equation).
    const eq = 'not running'; // indices: n0 o1 t2 _3 r4 u5 n6 n7 i8 n9 g10
    const glyphs = lineOf([
      ['¬', 0, 12],
      ['r', 12, 10],
      ['u', 22, 10],
      ['n', 32, 10],
      ['n', 42, 10],
      ['i', 52, 10],
      ['n', 62, 10],
      ['g', 72, 10],
    ]);
    // clicking the `¬` glyph lands the caret right at the `not` keyword
    expect(caretOffsetForClick(glyphs, 6, 6, eq)).toBe(0); // left/centre of `¬`
    expect(caretOffsetForClick(glyphs, 11, 6, eq)).toBe(4); // right edge of `¬` -> after `not `
    // clicking anywhere across "running" lands the caret inside (or at an edge of) it
    for (const x of [13, 27, 47, 81]) {
      const off = caretOffsetForClick(glyphs, x, 6, eq);
      expect(off >= 4 && off <= 11).toBe(true);
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

describe('nearestGlyphBoundary', () => {
  it('returns 0 for no glyphs', () => {
    expect(nearestGlyphBoundary([], 100, 50)).toBe(0);
  });

  it('picks the near side of a single glyph', () => {
    const glyphs = lineOf([['x', 10, 10]], 0, 12); // box [10,20], centre x=15, centre y=6
    expect(nearestGlyphBoundary(glyphs, 11, 6)).toBe(0); // left half -> before
    expect(nearestGlyphBoundary(glyphs, 19, 6)).toBe(1); // right half -> after
    expect(nearestGlyphBoundary(glyphs, 15, 6)).toBe(0); // dead centre -> before
    expect(nearestGlyphBoundary(glyphs, -50, 6)).toBe(0);
    expect(nearestGlyphBoundary(glyphs, 999, 6)).toBe(1);
  });

  it('uses the click Y to pick a wrapped line', () => {
    const glyphs: RenderedGlyph[] = [
      { char: 'a', left: 0, right: 10, top: 0, bottom: 12 },
      { char: 'b', left: 0, right: 10, top: 20, bottom: 32 },
    ];
    // near the start of the second line -> the boundary between a and b
    expect(nearestGlyphBoundary(glyphs, 2, 26)).toBe(1);
    // near the end of the first line -> also the boundary between a and b
    expect(nearestGlyphBoundary(glyphs, 8, 6)).toBe(1);
  });
});

describe('caretOffsetWithinSpan', () => {
  it('maps boundary to offset 1:1 within an identifier span (eqnloc, not an operator gap)', () => {
    // source: "x = average + 1"; the identifier `average` is bytes [4,11)
    const src = 'x = average + 1';
    const glyphs = lineOf([
      ['a', 0, 10],
      ['v', 10, 10],
      ['e', 20, 10],
      ['r', 30, 10],
      ['a', 40, 10],
      ['g', 50, 10],
      ['e', 60, 10],
    ]);
    expect(caretOffsetWithinSpan(glyphs, 5, 6, src, 4, 11, false)).toBe(4); // before `a`
    expect(caretOffsetWithinSpan(glyphs, 15, 6, src, 4, 11, false)).toBe(5); // before `v`
    expect(caretOffsetWithinSpan(glyphs, 35, 6, src, 4, 11, false)).toBe(7); // before `r`
    expect(caretOffsetWithinSpan(glyphs, -50, 6, src, 4, 11, false)).toBe(4); // clamps to span start
    expect(caretOffsetWithinSpan(glyphs, 999, 6, src, 4, 11, false)).toBe(11); // clamps to span end
  });

  it('trims surrounding whitespace from an operator-gap span to the operator', () => {
    // source: "a   *  b"; the gap between operands `a` [0,1) and `b` [7,8) is
    // bytes [1,7) = "   *  "; the `*` itself is at byte 4. KaTeX draws it as a
    // single `⋅` glyph.
    const src = 'a   *  b';
    const glyphs = lineOf([['⋅', 20, 6]]); // box [20,26], centre x=23
    expect(caretOffsetWithinSpan(glyphs, 21, 6, src, 1, 7, true)).toBe(4); // left half -> before `*`
    expect(caretOffsetWithinSpan(glyphs, 25, 6, src, 1, 7, true)).toBe(5); // right half -> after `*`
  });

  it('trims grouping parentheses from an operator-gap span (e.g. `(a+b)*c`)', () => {
    // source: "(a+b)*c"; the parser's operand ranges are `a+b` [1,4) and `c`
    // [6,7), so the `*` gap is bytes [4,6) = ")*". The `)` is part of the
    // left operand's grouping, not the operator, so it's trimmed: the `*` is
    // byte 5.
    const src = '(a+b)*c';
    const glyphs = lineOf([['⋅', 30, 6]]); // box [30,36]
    expect(caretOffsetWithinSpan(glyphs, 31, 6, src, 4, 6, true)).toBe(5); // before `*`
    expect(caretOffsetWithinSpan(glyphs, 35, 6, src, 4, 6, true)).toBe(6); // after `*`
  });

  it('returns the trimmed span start when there are no glyphs or the span is empty', () => {
    expect(caretOffsetWithinSpan([], 100, 50, 'a + b', 1, 4, true)).toBe(2); // " + " trims to start at byte 2
    const glyphs = lineOf([['x', 0, 10]]);
    expect(caretOffsetWithinSpan(glyphs, 5, 6, '   ', 0, 3, false)).toBe(0); // all-whitespace span
  });

  it('keeps a function call’s trailing paren (eqnloc spans are not trimmed of parens)', () => {
    // source: "min(a, b)" [0,9) but KaTeX renders 8 glyphs (the space after the
    // comma is dropped); a click on the function name / parens lands roughly
    // right and never escapes the span -- and the trailing `)` is *not*
    // trimmed (this is a node span, not an operator gap), so a far-right click
    // lands after it at byte 9.
    const src = 'min(a, b)';
    const glyphs = lineOf([
      ['m', 0, 10],
      ['i', 10, 10],
      ['n', 20, 10],
      ['(', 30, 10],
      ['a', 40, 10],
      [',', 50, 10],
      ['b', 60, 10],
      [')', 70, 10],
    ]);
    expect(caretOffsetWithinSpan(glyphs, -1, 6, src, 0, 9, false)).toBe(0);
    expect(caretOffsetWithinSpan(glyphs, 999, 6, src, 0, 9, false)).toBe(9);
    for (const x of [5, 25, 45, 65, 79]) {
      const off = caretOffsetWithinSpan(glyphs, x, 6, src, 0, 9, false);
      expect(off).toBeGreaterThanOrEqual(0);
      expect(off).toBeLessThanOrEqual(9);
    }
  });
});

describe('chooseSpanForClick', () => {
  it('returns -1 when there are no spans', () => {
    expect(chooseSpanForClick([], 10, 10)).toBe(-1);
  });

  it('picks the smallest box containing the point (the most specific node)', () => {
    // A composite [0,100] wrapping a leaf [10,40]; both share the same band.
    const boxes: SpanBox[] = [
      { left: 0, right: 100, top: 0, bottom: 20 },
      { left: 10, right: 40, top: 0, bottom: 20 },
    ];
    expect(chooseSpanForClick(boxes, 20, 10)).toBe(1); // inside both -> the leaf
    expect(chooseSpanForClick(boxes, 60, 10)).toBe(0); // only the composite here
  });

  it('prefers a small operator-gap box over the enclosing expression', () => {
    // `a * b`: leaf `a`, the operator gap around `*`, leaf `b`, and the whole
    // expression. A click on the operator must resolve to the gap, not the
    // expression that also contains it.
    const boxes: SpanBox[] = [
      { left: 0, right: 60, top: 0, bottom: 12 }, // whole `a * b`
      { left: 0, right: 10, top: 0, bottom: 12 }, // leaf a
      { left: 24, right: 36, top: 0, bottom: 12 }, // gap around *
      { left: 50, right: 60, top: 0, bottom: 12 }, // leaf b
    ];
    expect(chooseSpanForClick(boxes, 30, 6)).toBe(2); // on the `*` -> the gap
    expect(chooseSpanForClick(boxes, 5, 6)).toBe(1); // on `a`
    expect(chooseSpanForClick(boxes, 55, 6)).toBe(3); // on `b`
  });

  it('falls back to the nearest box when the point is outside every box', () => {
    const boxes: SpanBox[] = [
      { left: 0, right: 10, top: 0, bottom: 10 },
      { left: 100, right: 110, top: 0, bottom: 10 },
    ];
    expect(chooseSpanForClick(boxes, 200, 5)).toBe(1); // nearer the right box
    expect(chooseSpanForClick(boxes, -50, 5)).toBe(0); // nearer the left box
  });

  // The `population/average_lifespan` regression (issue: clicking the fraction
  // bar or the denominator placed the caret inside the numerator). These boxes
  // are the real KaTeX client rects measured in the browser; the annotated
  // spans are the whole fraction (`0_27`), the denominator (`11_27`), and the
  // numerator (`0_10`), in DOM order. A click on the fraction bar / v-list
  // lands on layout chrome whose only annotated ancestor is `0_27`, so an
  // ancestry walk mis-mapped it -- geometry resolves it to the operand row.
  describe('fraction: population/average_lifespan', () => {
    const OUTER = 0;
    const DENOM = 1;
    const NUMER = 2;
    const boxes: SpanBox[] = [
      { left: 48, right: 200, top: 49, bottom: 83 }, // 0_27 whole fraction
      { left: 51, right: 196, top: 67, bottom: 91 }, // 11_27 denominator
      { left: 76, right: 171, top: 44, bottom: 68 }, // 0_10 numerator
    ];

    it('resolves a click in the denominator to the denominator span', () => {
      expect(chooseSpanForClick(boxes, 124, 79)).toBe(DENOM); // middle
      expect(chooseSpanForClick(boxes, 55, 79)).toBe(DENOM); // near the start
      expect(chooseSpanForClick(boxes, 192, 79)).toBe(DENOM); // near the end
    });

    it('resolves a click in the numerator to the numerator span', () => {
      expect(chooseSpanForClick(boxes, 124, 56)).toBe(NUMER);
      expect(chooseSpanForClick(boxes, 80, 56)).toBe(NUMER);
    });

    it('resolves a click on the fraction bar to an operand row, never the whole fraction', () => {
      // The bar sits in the numerator/denominator overlap band; either operand
      // is acceptable, but it must not fall through to the whole-fraction span
      // (which would map the click by interpolation across the entire eqn).
      expect(chooseSpanForClick(boxes, 124, 67)).not.toBe(OUTER);
      expect(chooseSpanForClick(boxes, 60, 67)).not.toBe(OUTER);
    });
  });
});

// End-to-end of the annotation path: choose the span geometrically, then map
// the click within it. This is the composition VariableDetails.tsx performs;
// it pins the fraction regression at the boundary the user actually sees --
// the caret offset -- not just the span index.
describe('span selection + within-span mapping (annotation path)', () => {
  const eq = 'population/average_lifespan'; // population [0,10), `/` [10,11), average_lifespan [11,27)
  // KaTeX rects (from the browser): numerator over a bar over the denominator.
  const numerBox: SpanBox = { left: 76, right: 171, top: 44, bottom: 68 };
  const denomBox: SpanBox = { left: 51, right: 196, top: 67, bottom: 91 };
  const outerBox: SpanBox = { left: 48, right: 200, top: 49, bottom: 83 };
  const boxes = [outerBox, denomBox, numerBox];

  // One glyph per source character, laid out left-to-right across a box.
  const glyphsAcross = (box: SpanBox, chars: string): RenderedGlyph[] => {
    const w = (box.right - box.left) / chars.length;
    return Array.from(chars, (ch, i) => ({
      char: ch,
      left: box.left + i * w,
      right: box.left + (i + 1) * w,
      top: box.top,
      bottom: box.bottom,
    }));
  };

  const mapClick = (clickX: number, clickY: number): number => {
    const idx = chooseSpanForClick(boxes, clickX, clickY);
    if (idx === 0) {
      return caretOffsetWithinSpan(glyphsAcross(outerBox, eq), clickX, clickY, eq, 0, 27, false);
    }
    if (idx === 1) {
      return caretOffsetWithinSpan(glyphsAcross(denomBox, 'average_lifespan'), clickX, clickY, eq, 11, 27, false);
    }
    return caretOffsetWithinSpan(glyphsAcross(numerBox, 'population'), clickX, clickY, eq, 0, 10, false);
  };

  it('places the caret inside the denominator when clicking average_lifespan', () => {
    // Every click across the denominator maps into [11, 27] -- never the
    // numerator range the ancestry-based lookup produced.
    for (const x of [55, 90, 124, 160, 192]) {
      const off = mapClick(x, 79);
      expect(off).toBeGreaterThanOrEqual(11);
      expect(off).toBeLessThanOrEqual(27);
    }
  });

  it('places the caret inside the numerator when clicking population', () => {
    for (const x of [80, 110, 124, 160]) {
      const off = mapClick(x, 56);
      expect(off).toBeGreaterThanOrEqual(0);
      expect(off).toBeLessThanOrEqual(10);
    }
  });
});
