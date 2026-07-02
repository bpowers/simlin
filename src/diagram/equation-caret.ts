// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mapping a click on the rendered (KaTeX) equation preview back to a caret
// offset in the editable equation text.
//
// Primary path: the engine's `Ast::to_latex` (used by the FFI) wraps every
// node in a `\htmlData{eqnloc=START_END}` annotation carrying the source byte
// range it covers, so KaTeX renders each atom inside a span with a
// `data-eqnloc` attribute. The click handler picks the most specific annotated
// span for the click point geometrically (`chooseSpanForClick` -- the smallest
// box containing it) and maps the click within it (`caretOffsetWithinSpan`) --
// this is exact rather than heuristic. Geometry rather than DOM ancestry is
// what lets a click on layout chrome that has no annotation of its own (a
// fraction bar, the `\frac` v-list wrapper) resolve to the operand it visually
// sits in instead of the composite span that merely encloses it.
//
// Fallback path: when the rendered LaTeX has no annotations (e.g. the engine
// couldn't produce LaTeX and the UI rendered the raw equation text instead),
// `caretOffsetForClick` reconstructs the mapping from the glyph boxes: it
// finds the nearest glyph boundary in 2D, then aligns the rendered glyph
// string to the source text -- character for character in the common case,
// with a little slack for the few places they disagree.
//
// The functions here are pure and DOM-free; `VariableDetails.tsx` owns the DOM
// walk that produces `RenderedGlyph`s and reads the `data-eqnloc` attributes.

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

/** Map a rendered glyph back to the source text it stands for. KaTeX renders
 *  `*` (multiplication) as `\cdot`/`\times` and binary `-` as a true minus
 *  sign, so those map back to single ASCII operators. The engine's
 *  `Ast::to_latex` renders the `not` keyword (XMILE spells logical negation as
 *  the word `NOT`, not `!` -- `!` isn't a token in Simlin equations at all)
 *  as `\neg`, which KaTeX draws as a `¬` glyph; that one glyph stands for the
 *  whole three-letter keyword, so it maps to the string `'not'` and the
 *  alignment consumes that many source characters. Everything else -- letters,
 *  digits, parentheses, `+`, comparison operators -- renders as the same
 *  character that appears in the source and passes through unchanged. */
export function glyphToAscii(ch: string): string {
  switch (ch) {
    case '·': // MIDDLE DOT
    case '×': // MULTIPLICATION SIGN  (\times)
    case '⋅': // DOT OPERATOR         (\cdot)
      return '*';
    case '−': // MINUS SIGN           (binary -)
      return '-';
    case '¬': // NOT SIGN             (\neg, i.e. the `not` keyword)
      return 'not';
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
 * skips whitespace in the source (the rendered math drops it). A glyph can
 * stand for more than one source character -- the `¬` glyph stands for the
 * `not` keyword (see `glyphToAscii`) -- in which case the whole substring is
 * matched and consumed. Where a glyph has no source counterpart at the current
 * position the alignment first tries to resync by skipping up to
 * `MAX_RESYNC_SKIP` source characters (the `/` of a fraction, the `^` of an
 * exponent); failing that it treats the glyph as "extra" -- rendered but
 * absent from the source, e.g. a parenthesis KaTeX added for precedence -- and
 * lets it occupy no source characters.
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

    if (eqLower.startsWith(g, ei)) {
      mapping[gi] = ei;
      ei += g.length;
      continue;
    }

    // Mismatch: probe ahead for the glyph, skipping a few non-matching
    // (and whitespace) source characters.
    let probe = ei;
    let resynced = -1;
    for (let step = 0; step < MAX_RESYNC_SKIP && probe < len; step++) {
      probe++;
      while (probe < len && isWhitespace(equationStr[probe])) probe++;
      if (probe < len && eqLower.startsWith(g, probe)) {
        resynced = probe;
        break;
      }
    }
    if (resynced >= 0) {
      mapping[gi] = resynced;
      ei = resynced + g.length;
      continue;
    }

    // The glyph isn't in the source here -- it's extra (e.g. an added paren).
    mapping[gi] = ei;
  }
  mapping[n] = len;
  return mapping;
}

/**
 * Find the caret boundary nearest a click among `glyphs` (in DOM/reading
 * order), returning an index in `[0, glyphs.length]` -- boundary `k` sits
 * "before glyph k" / "after glyph k-1".
 *
 * Each boundary has up to two visual points: the left edge of glyph `k` and
 * the right edge of glyph `k-1`. Those coincide except across a KaTeX line
 * break, where the same logical boundary appears at the end of one line and
 * the start of the next; taking whichever point is closest in 2D handles both
 * the wrapped-line case and a click in the whitespace around a small operator
 * glyph (it snaps to the operator's side rather than into an adjacent atom).
 * On a tie the lower index wins, so a click dead-centre on a glyph lands
 * before it.
 */
export function nearestGlyphBoundary(glyphs: readonly RenderedGlyph[], clickX: number, clickY: number): number {
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
  return bestK;
}

/**
 * Given the glyphs of a rendered equation preview (in DOM/reading order), a
 * click point in client coordinates, and the source equation text, return the
 * character offset in `equationStr` where the caret should be placed. This is
 * the heuristic path used when the rendered LaTeX carries no source-range
 * annotations (e.g. when the engine couldn't produce LaTeX and we fall back to
 * rendering the raw equation text).
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
  const k = nearestGlyphBoundary(glyphs, clickX, clickY);
  const mapping = alignGlyphsToSource(glyphs, equationStr);
  return Math.max(0, Math.min(len, mapping[k]));
}

/** The on-screen bounding box of a source-annotated span, in client
 *  (viewport) coordinates -- the same space as a `MouseEvent`'s
 *  `clientX`/`clientY`. `VariableDetails.tsx` collects one per
 *  `data-eqnloc`/`data-oploc` element under the preview. */
export interface SpanBox {
  readonly left: number;
  readonly right: number;
  readonly top: number;
  readonly bottom: number;
}

/**
 * Pick which source-annotated span a click belongs to, returning its index in
 * `boxes` (or -1 when `boxes` is empty).
 *
 * Annotated spans nest -- a composite node (a fraction, a function call, an
 * operator expression) wraps its operands, each of which is itself annotated --
 * so a click point is typically inside several boxes at once. We want the
 * *most specific* one, so among the boxes that contain the point we take the
 * smallest by area: a click inside `average_lifespan` picks that leaf, not the
 * whole `population/average_lifespan` fraction that also contains it.
 *
 * This is what fixes clicks that land on KaTeX layout chrome rather than a
 * glyph -- the fraction bar, or the `\frac` v-list wrapper that overlays the
 * denominator row. Walking DOM *ancestors* (`Element.closest`) from such a
 * target stops at the composite fraction span (the bar and the wrapper are not
 * inside either operand), which then maps the click by a coarse interpolation
 * across the entire equation. Selecting by geometry instead reaches the operand
 * the pixel actually sits in.
 *
 * When the point is inside no box (padding, or slightly outside the rendered
 * math) we fall back to the nearest box by squared distance to its rect, so a
 * near-miss still resolves to a sensible span rather than nothing.
 */
export function chooseSpanForClick(boxes: readonly SpanBox[], clickX: number, clickY: number): number {
  let best = -1;
  let bestArea = Number.POSITIVE_INFINITY;
  for (let i = 0; i < boxes.length; i++) {
    const b = boxes[i];
    if (clickX >= b.left && clickX <= b.right && clickY >= b.top && clickY <= b.bottom) {
      const area = (b.right - b.left) * (b.bottom - b.top);
      // Strictly-smaller keeps the earliest (outermost-listed) box on an exact
      // area tie, which is deterministic and never matters in practice.
      if (area < bestArea) {
        bestArea = area;
        best = i;
      }
    }
  }
  if (best >= 0) {
    return best;
  }
  let bestDistSq = Number.POSITIVE_INFINITY;
  for (let i = 0; i < boxes.length; i++) {
    const b = boxes[i];
    const dx = clickX < b.left ? b.left - clickX : clickX > b.right ? clickX - b.right : 0;
    const dy = clickY < b.top ? b.top - clickY : clickY > b.bottom ? clickY - b.bottom : 0;
    const d = dx * dx + dy * dy;
    if (d < bestDistSq) {
      bestDistSq = d;
      best = i;
    }
  }
  return best;
}

// A parenthesis. Trimmed off the ends of an *operator-gap* span (in addition
// to whitespace): that span is the gap between the parser's two operand
// ranges, which can include the grouping parens those ranges exclude -- e.g.
// in `(a+b)*c` the gap between `a+b` and `c` is `)*`, so trimming the `)`
// homes in on the `*`. Leaf/node spans never have a `(`/`)` at their edges
// except a function call's trailing `)`, which we must keep, so the trim is
// applied only when the span was tagged as an operator gap (`data-oploc`).
const isParen = (ch: string | undefined): boolean => ch === '(' || ch === ')';

/**
 * Map a click within a single source-annotated span -- an element the engine
 * tagged with `\htmlData{eqnloc=…}` (a syntax-node span) or `\htmlData{oploc=…}`
 * (the gap around a binary/unary operator); see `latex_eqn_expr0_annotated` on
 * the Rust side -- to a caret offset in the equation text.
 *
 * `[spanStart, spanEnd)` is the half-open byte range the span covers. We trim
 * whitespace (and, for an operator gap, parentheses) off both ends so the
 * caret homes in on the operator token / on the leaf's text; within the
 * trimmed range the click is mapped by glyph -- for the common case where the
 * span renders one glyph per source character (identifiers, numbers, single-
 * character operators) the boundary index *is* the offset; otherwise it
 * interpolates.
 */
export function caretOffsetWithinSpan(
  glyphs: readonly RenderedGlyph[],
  clickX: number,
  clickY: number,
  sourceText: string,
  spanStart: number,
  spanEnd: number,
  isOperatorGap: boolean,
): number {
  const len = sourceText.length;
  const lo = Math.max(0, Math.min(len, Math.min(spanStart, spanEnd)));
  const hi = Math.max(0, Math.min(len, Math.max(spanStart, spanEnd)));
  const trim = (ch: string | undefined): boolean => isWhitespace(ch) || (isOperatorGap && isParen(ch));
  let ts = lo;
  while (ts < hi && trim(sourceText[ts])) ts++;
  let te = hi;
  while (te > ts && trim(sourceText[te - 1])) te--;
  if (ts >= te) {
    // The span is empty or all whitespace -- shouldn't happen for a real
    // `eqnloc`, but fall back to the span start.
    return lo;
  }
  if (glyphs.length === 0) {
    return ts;
  }
  const k = nearestGlyphBoundary(glyphs, clickX, clickY); // 0..glyphs.length
  const offset = ts + Math.round((k * (te - ts)) / glyphs.length);
  return Math.max(ts, Math.min(te, offset));
}
