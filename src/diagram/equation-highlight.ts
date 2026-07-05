// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Pure helpers for mapping engine-reported source ranges onto the equation
// text shown in the details panel.
//
// Two coordinate mismatches have to be bridged:
//
//  1. The Rust engine reports error and `eqnloc` offsets as *byte* offsets
//     into the UTF-8 encoding of the equation, while JavaScript strings are
//     indexed by UTF-16 code units. They agree only for pure-ASCII text.
//  2. The panel sometimes displays a decorated version of the raw equation
//     (apply-to-all equations are shown with an `{apply-to-all:}` line
//     prepended), so a raw-equation offset must additionally be shifted to
//     its position in the displayed string, which may span multiple lines
//     (one Slate element per line).

import type { EquationError, UnitError } from '@simlin/core/datamodel';

import type { FormattedText } from './drawing/SlateEditor';

/** Displayed-string prefix for apply-to-all arrayed equations. Engine offsets
 *  are relative to the raw equation, which begins after this prefix. */
export const applyToAllPrefix = '{apply-to-all:}\n';

/**
 * Convert a byte offset into the UTF-8 encoding of `s` to a UTF-16 code-unit
 * index. Offsets that land inside a multi-byte character snap to the end of
 * that character; offsets past the end of the string clamp to `s.length`.
 */
export function byteOffsetToUtf16(s: string, byteOffset: number): number {
  if (byteOffset <= 0) {
    return 0;
  }
  let bytes = 0;
  let i = 0;
  while (i < s.length && bytes < byteOffset) {
    const cp = s.codePointAt(i) as number;
    bytes += cp <= 0x7f ? 1 : cp <= 0x7ff ? 2 : cp <= 0xffff ? 3 : 4;
    i += cp > 0xffff ? 2 : 1;
  }
  return i;
}

export interface HighlightRange {
  /** byte offset into the raw equation (inclusive) */
  readonly startByte: number;
  /** byte offset into the raw equation (exclusive) */
  readonly endByte: number;
  readonly kind: 'error' | 'warning';
}

/**
 * Split `displayed` into lines (one Slate element per line, matching
 * plainDeserialize) and apply `range` -- byte offsets into the raw equation,
 * which begins at UTF-16 index `rawStart` within `displayed` -- as
 * error/warning marks on the covered text. Lines (or line parts) outside the
 * range come back as plain text spans.
 */
export function highlightSpansForLines(
  displayed: string,
  rawStart: number,
  range: HighlightRange | undefined,
): FormattedText[][] {
  const lines = displayed.split('\n');

  if (!range) {
    return lines.map((line) => [{ text: line }]);
  }

  const raw = displayed.slice(rawStart);
  const start = rawStart + byteOffsetToUtf16(raw, range.startByte);
  const end = rawStart + byteOffsetToUtf16(raw, range.endByte);
  const mark: Partial<FormattedText> = range.kind === 'error' ? { error: true } : { warning: true };

  const result: FormattedText[][] = [];
  let lineStart = 0;
  for (const line of lines) {
    const lineEnd = lineStart + line.length;
    // Overlap of [start, end) with this line's [lineStart, lineEnd).
    const hlStart = Math.max(start, lineStart);
    const hlEnd = Math.min(end, lineEnd);
    if (hlStart < hlEnd) {
      const spans: FormattedText[] = [];
      const before = displayed.slice(lineStart, hlStart);
      const marked = displayed.slice(hlStart, hlEnd);
      const after = displayed.slice(hlEnd, lineEnd);
      if (before) spans.push({ text: before });
      spans.push({ text: marked, ...mark });
      if (after) spans.push({ text: after });
      result.push(spans);
    } else {
      result.push([{ text: line }]);
    }
    lineStart = lineEnd + 1; // skip the newline
  }
  return result;
}

/**
 * Pick the error/warning underline for one details-panel field (the equation
 * editor or the units editor) from the variable's errors. `raw` is the raw
 * field text (the displayed string minus any apply-to-all prefix).
 *
 * Equation field: a fatal equation error wins outright (and suppresses unit
 * underlines even when its own range is empty). Otherwise a unit consistency
 * error marks the sub-expression whose computed units conflict as a warning --
 * unless it spans the whole equation, where it localizes nothing (the units
 * field carries the underline for "the equation as a whole doesn't match the
 * declaration").
 *
 * Units field: a definition error's offsets point into the units string and
 * are used directly; a consistency error's offsets point into the *equation*,
 * so the entire declaration is underlined instead. Unit errors are non-fatal
 * project-wide, but within this field they are the error, hence 'error' red.
 *
 * Inference errors underline nothing in either field: they describe a
 * contradiction spanning several variables, so any single-field span would be
 * arbitrary -- and their offsets point into an equation (possibly another
 * variable's), never the units string. Their plain-language reason renders in
 * the details panel instead.
 *
 * An `end` of 0 is the engine's "to the end of the text" convention;
 * byteOffsetToUtf16 clamps, so any large value reads as "the end".
 */
export function highlightRangeForField(
  raw: string,
  errors: readonly EquationError[] | undefined,
  unitErrors: readonly UnitError[] | undefined,
  isUnits: boolean,
): HighlightRange | undefined {
  if (!isUnits && errors && errors.length > 0) {
    const err = errors[0];
    if (err.end > 0) {
      return { startByte: err.start, endByte: err.end, kind: 'error' };
    }
    return undefined;
  }

  if (!unitErrors || unitErrors.length === 0) {
    return undefined;
  }

  if (isUnits) {
    const definition = unitErrors.find((err) => err.kind === 'definition');
    if (definition) {
      const endByte = definition.end === 0 ? Number.MAX_SAFE_INTEGER : definition.end;
      return { startByte: definition.start, endByte, kind: 'error' };
    }
    if (unitErrors.some((err) => err.kind === 'consistency')) {
      return { startByte: 0, endByte: Number.MAX_SAFE_INTEGER, kind: 'error' };
    }
    return undefined;
  }

  const consistency = unitErrors.find((err) => err.kind === 'consistency');
  if (!consistency) {
    return undefined;
  }
  const endByte = consistency.end === 0 ? Number.MAX_SAFE_INTEGER : consistency.end;
  if (consistency.start === 0 && byteOffsetToUtf16(raw, endByte) >= raw.length) {
    return undefined;
  }
  return { startByte: consistency.start, endByte, kind: 'warning' };
}

export interface SlatePoint {
  readonly path: readonly [number, number];
  readonly offset: number;
}

/**
 * Convert a flat UTF-16 offset into `displayed` to a Slate point in the
 * one-element-per-line document produced by plainDeserialize. Out-of-range
 * offsets clamp to the document's start/end.
 */
export function slatePointForOffset(displayed: string, offset: number): SlatePoint {
  const lines = displayed.split('\n');
  let remaining = Math.max(0, Math.min(displayed.length, offset));
  for (let i = 0; i < lines.length; i++) {
    const len = lines[i].length;
    if (remaining <= len) {
      return { path: [i, 0], offset: remaining };
    }
    remaining -= len + 1; // the newline separating this line from the next
  }
  const lastLine = lines.length - 1;
  return { path: [lastLine, 0], offset: lines[lastLine].length };
}
