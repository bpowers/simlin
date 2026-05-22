// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Unit tests for the engine-local, Rust-faithful canonicalizeIdent.
 *
 * The expected sentinel codepoints are written as explicit \uXXXX escapes,
 * NEVER bare glyphs: U+2024 (ONE DOT LEADER, the literal-period sentinel),
 * U+2025 (TWO DOT LEADER), and U+00B7 (MIDDLE DOT, the module separator) are
 * visually indistinguishable in many fonts, so a copied glyph would silently
 * assert the wrong codepoint and corrupt name resolution.
 *
 * Vectors are taken verbatim from the Rust test vectors in
 * simlin-engine/src/common.rs (test_canonicalize and
 * test_canonicalize_non_period_idents_byte_unchanged), the oracle this
 * function must reproduce exactly.
 */

import { canonicalizeIdent } from '../src/internal/canonicalize';

describe('canonicalizeIdent', () => {
  it('lowercases and collapses whitespace to underscore', () => {
    expect(canonicalizeIdent('Hello World')).toBe('hello_world');
  });

  it('maps an unquoted dot to the U+00B7 module separator', () => {
    expect(canonicalizeIdent('a.b')).toBe('a\u{00B7}b');
  });

  it('maps a quoted-inner dot to the U+2024 literal-period sentinel', () => {
    expect(canonicalizeIdent('"a.b"')).toBe('a\u{2024}b');
  });

  it('handles an unquoted part followed by a quoted part', () => {
    // a."b c" -> parts ["a.", "\"b c\""]: the unquoted "a." dot -> U+00B7,
    // the quoted "b c" -> b_c.
    expect(canonicalizeIdent('a."b c"')).toBe('a\u{00B7}b_c');
  });

  it('maps a module path separator to U+00B7', () => {
    expect(canonicalizeIdent('model.variable')).toBe('model\u{00B7}variable');
  });

  it('treats the dot between two quoted parts as a module separator', () => {
    expect(canonicalizeIdent('"a/d"."b c"')).toBe('a/d\u{00B7}b_c');
  });

  it('treats the dot between a quoted and an unquoted part as a module separator', () => {
    expect(canonicalizeIdent('"a/d".b')).toBe('a/d\u{00B7}b');
  });

  it('strips surrounding quotes from a simple quoted ident', () => {
    expect(canonicalizeIdent('"quoted"')).toBe('quoted');
  });

  it('strips quotes and collapses inner whitespace', () => {
    expect(canonicalizeIdent('"b c"')).toBe('b_c');
  });

  it('passes non-ASCII through, lowercased', () => {
    expect(canonicalizeIdent('café')).toBe('café');
  });

  it('lowercases non-ASCII and turns a literal \\n escape into underscore', () => {
    // 'Å\nb' is the three-char string Å, backslash-n's actual newline; Rust's
    // expectation for this vector is 'å_b'.
    expect(canonicalizeIdent('Å\nb')).toBe('å_b');
  });

  it('trims leading whitespace and collapses interior whitespace', () => {
    expect(canonicalizeIdent('   a b')).toBe('a_b');
  });

  it('is a no-op on already-canonical input', () => {
    expect(canonicalizeIdent('room_temperature')).toBe('room_temperature');
  });

  // Additional vectors from common.rs that exercise the per-part order of
  // operations (quote handling, backslash unescape, whitespace collapse).
  it('collapses a run of mixed whitespace inside quotes to a single underscore', () => {
    expect(canonicalizeIdent('a \n b')).toBe('a_b');
  });

  it('lowercases a bare uppercase identifier', () => {
    expect(canonicalizeIdent('Population')).toBe('population');
  });

  it('collapses multiple unquoted whitespace tokens', () => {
    expect(canonicalizeIdent('a b c')).toBe('a_b_c');
  });

  it('treats a non-breaking space (U+00A0) as whitespace', () => {
    expect(canonicalizeIdent('a\u{00A0}b')).toBe('a_b');
  });

  it('collapses a literal \\r escape to a single underscore', () => {
    // Two characters: backslash then r.
    expect(canonicalizeIdent('a\\rb')).toBe('a_b');
  });

  it('unescapes a doubled backslash to a single backslash', () => {
    expect(canonicalizeIdent('a\\\\b')).toBe('a\\b');
  });

  it('leaves the synthetic separator U+205A and the module dot untouched', () => {
    expect(canonicalizeIdent('stdlib\u{205A}smth1')).toBe('stdlib\u{205A}smth1');
    expect(canonicalizeIdent('model\u{00B7}variable')).toBe('model\u{00B7}variable');
  });

  it('canonicalizes a quoted ident with an escaped inner quote', () => {
    // "a/d"."b \"c\"" -> a/d·b_\"c\" (Rust: "a/d·b_\\\"c\\\"").
    expect(canonicalizeIdent('"a/d"."b \\"c\\""')).toBe('a/d\u{00B7}b_\\"c\\"');
  });

  it('returns an empty string for an empty or all-whitespace input', () => {
    expect(canonicalizeIdent('')).toBe('');
    expect(canonicalizeIdent('   ')).toBe('');
  });

  describe('idempotency (canonicalizeIdent(canonicalizeIdent(x)) === canonicalizeIdent(x))', () => {
    // Includes the #559 quoted-literal-period family that the Rust oracle pins
    // (test_canonicalize_idempotent_quoted_period): a literal period inside a
    // quoted name must canonicalize to a form that re-canonicalizes unchanged.
    const inputs: ReadonlyArray<string> = [
      'Hello World',
      'a.b',
      '"a.b"',
      'a."b c"',
      'model.variable',
      '"a/d"."b c"',
      '"a/d".b',
      '"quoted"',
      '"b c"',
      'café',
      'Å\nb',
      '   a b',
      'room_temperature',
      '"a.b c"',
      '"Goal 1.5 for Temperature"',
      '"goal_1.5_for_temperature"',
      '"Fig. 3"',
      '"v1.2 target"',
      'stdlib\u{205A}smth1',
      'model\u{00B7}variable',
      'a\\\\b',
    ];

    for (const input of inputs) {
      it(`is idempotent for ${JSON.stringify(input)}`, () => {
        const once = canonicalizeIdent(input);
        const twice = canonicalizeIdent(once);
        expect(twice).toBe(once);
      });
    }

    it('never leaves a raw "." or maps a quoted period to U+00B7 (the #559 corruption)', () => {
      for (const input of ['"a.b"', '"Goal 1.5 for Temperature"', '"Fig. 3"']) {
        const once = canonicalizeIdent(input);
        expect(once.includes('.')).toBe(false);
        expect(once.includes('\u{00B7}')).toBe(false);
      }
    });
  });
});
