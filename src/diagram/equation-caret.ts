// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mapping a click on the rendered (KaTeX) equation preview back to a caret
// offset in the editable equation text.
//
// The rendered equation is a *transformed* view of the source text -- `*`
// becomes `\cdot`, `/` becomes a fraction, identifiers are lower-cased,
// whitespace is dropped, etc. -- so we can't read the source offset directly
// off the DOM. This module takes the glyphs the preview rendered (extracted by
// the imperative shell, with their on-screen boxes) and:
//
//   1. finds the glyph *boundary* nearest the click in 2D (so a click in the
//      whitespace around a small operator glyph snaps to the side of that
//      operator instead of landing inside an adjacent identifier, and so
//      clicks on a wrapped line work);
//   2. aligns the rendered glyph string to the source text -- character for
//      character in the common case, with a little slack for the few places
//      they disagree -- to translate that boundary into a source offset.
//
// The functions here are pure and DOM-free; `VariableDetails.tsx` owns the DOM
// walk that produces `RenderedGlyph`s.

/** A single glyph from the rendered equation, with its on-screen bounding box
 *  in client (viewport) coordinates -- the same space as a `MouseEvent`'s
 *  `clientX`/`clientY`. */
export interface RenderedGlyph {
  readonly char: string;
  readonly left: number;
  readonly right: number;
  readonly top: number;
  readonly bottom: number;
}

/** Map a rendered glyph back to the ASCII character it stands for in the
 *  source equation. KaTeX renders `*` (multiplication) as `\cdot`/`\times`,
 *  binary `-` as a true minus sign, and `not` as `\neg`; everything else --
 *  letters, digits, parentheses, `+`, comparison operators -- renders as the
 *  same character that appears in the source. */
export function glyphToAscii(ch: string): string {
  switch (ch) {
    case '·': // MIDDLE DOT
    case '×': // MULTIPLICATION SIGN  (\times)
    case '⋅': // DOT OPERATOR         (\cdot)
      return '*';
    case '−': // MINUS SIGN           (binary -)
      return '-';
    case '¬': // NOT SIGN             (\neg)
      return '!';
    default:
      return ch;
  }
}

// How many "extra" source characters the alignment will skip over to resync
// with the glyph stream (e.g. a `/` that rendered as a fraction bar, or a `^`
// that rendered as a superscript). Kept small so a genuinely-extra glyph
// doesn't get matched to a far-away same-letter source character.
const MAX_RESYNC_SKIP = 2;

const isWhitespace = (ch: string | undefined): boolean => ch !== undefined && /\s/.test(ch);

/**
 * Greedily align the rendered glyph stream to the source equation text,
 * returning a `glyphs.length + 1` long array where entry `k` is the source
 * offset for "caret immediately before glyph `k`" (and entry `glyphs.length`
 * is the end of the equation). The result is monotonically non-decreasing.
 *
 * The match is case-insensitive (identifiers are lower-cased in LaTeX) and
 * skips whitespace in the source (the rendered math drops it). Where a glyph
 * has no source counterpart at the current position the alignment first tries
 * to resync by skipping up to `MAX_RESYNC_SKIP` source characters (the `/`
 * of a fraction, the `^` of an exponent); failing that it treats the glyph as
 * "extra" -- rendered but absent from the source, e.g. a parenthesis KaTeX
 * added for precedence -- and lets it occupy no source characters.
 */
export function alignGlyphsToSource(glyphs: readonly RenderedGlyph[], equationStr: string): number[] {
  const n = glyphs.length;
  const eqLower = equationStr.toLowerCase();
  const len = equationStr.length;

  const mapping = new Array<number>(n + 1);
  let ei = 0;
  for (let gi = 0; gi < n; gi++) {
    const g = glyphToAscii(glyphs[gi].char).toLowerCase();
    while (ei < len && isWhitespace(equationStr[ei])) ei++;

    if (g === '') {
      // An invisible glyph (shouldn't normally reach us) -- occupies nothing.
      mapping[gi] = ei;
      continue;
    }

    if (ei < len && eqLower[ei] === g) {
      mapping[gi] = ei;
      ei++;
      continue;
    }

    // Mismatch: probe ahead for the glyph, skipping a few non-matching
    // (and whitespace) source characters.
    let probe = ei;
    let resynced = -1;
    for (let step = 0; step < MAX_RESYNC_SKIP && probe < len; step++) {
      probe++;
      while (probe < len && isWhitespace(equationStr[probe])) probe++;
      if (probe < len && eqLower[probe] === g) {
        resynced = probe;
        break;
      }
    }
    if (resynced >= 0) {
      mapping[gi] = resynced;
      ei = resynced + 1;
      continue;
    }

    // The glyph isn't in the source here -- it's extra (e.g. an added paren).
    mapping[gi] = ei;
  }
  mapping[n] = len;
  return mapping;
}

/**
 * Given the glyphs of a rendered equation preview (in DOM/reading order), a
 * click point in client coordinates, and the source equation text, return the
 * character offset in `equationStr` where the caret should be placed.
 *
 * Returns 0 when there are no glyphs (the caller is expected to fall back to a
 * coarse proportional mapping in that case, since it needs the DOM rect).
 */
export function caretOffsetForClick(
  glyphs: readonly RenderedGlyph[],
  clickX: number,
  clickY: number,
  equationStr: string,
): number {
  const len = equationStr.length;
  if (glyphs.length === 0) {
    return 0;
  }

  // A caret boundary `k` (0..glyphs.length) sits "before glyph k" and "after
  // glyph k-1". Visually those are two points -- the left edge of glyph k and
  // the right edge of glyph k-1 -- which coincide except across a KaTeX line
  // break, where the same logical boundary shows up at the end of one line and
  // the start of the next. Take the boundary whose nearest visual point is
  // closest to the click; on a tie prefer the lower index (the left side),
  // which makes a click dead-centre on a glyph land before it.
  let bestK = 0;
  let bestDistSq = Number.POSITIVE_INFINITY;
  const consider = (k: number, x: number, y: number): void => {
    const dx = clickX - x;
    const dy = clickY - y;
    const d = dx * dx + dy * dy;
    if (d < bestDistSq) {
      bestDistSq = d;
      bestK = k;
    }
  };
  for (let k = 0; k <= glyphs.length; k++) {
    if (k < glyphs.length) {
      const g = glyphs[k];
      consider(k, g.left, (g.top + g.bottom) / 2);
    }
    if (k > 0) {
      const g = glyphs[k - 1];
      consider(k, g.right, (g.top + g.bottom) / 2);
    }
  }

  const mapping = alignGlyphsToSource(glyphs, equationStr);
  const offset = mapping[bestK];
  return Math.max(0, Math.min(len, offset));
}
