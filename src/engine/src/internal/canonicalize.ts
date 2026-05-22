// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure name canonicalization: same input always yields the same output, no I/O.

// The literal-period sentinel: a `.` inside a quoted identifier maps to U+2024
// (ONE DOT LEADER) rather than a raw `.`. A raw `.` is not canonical, so a
// re-canonicalization pass would treat the now-unquoted period as the U+00B7
// module separator and corrupt the identity (simlin-engine issue #559). Mapping
// it to a dedicated sentinel keeps canonicalize idempotent while preserving the
// literal-period-vs-module-separator distinction. Mirrors
// simlin-engine/src/common.rs `LITERAL_PERIOD_SENTINEL`.
const LITERAL_PERIOD_SENTINEL = '\u{2024}';

// The module-hierarchy separator: an unquoted `.` (e.g. `model.variable`) maps
// to U+00B7 (MIDDLE DOT). Mirrors the `·` substitution in `canonicalize`.
const MODULE_SEPARATOR = '\u{00B7}';

/**
 * Split a trimmed identifier into its quoted and unquoted parts, quote-aware.
 *
 * A `.` is NOT a split boundary; the parts retain their dots, and the caller
 * substitutes them per-part (a `.` inside a quoted part is a literal period; a
 * `.` in an unquoted part is the module separator). A quoted part keeps its
 * surrounding quotes so the caller can detect it. Escaped quotes (`\"`) inside
 * a quoted section do not close it.
 *
 * Faithful port of Rust's `IdentifierPartIterator` (simlin-engine/src/common.rs):
 * matches the regex `[^"]+|"((\\")|[^"])*"`.
 */
function splitIdentifierParts(s: string): Array<string> {
  const parts: Array<string> = [];
  let remaining = s;

  while (remaining.length > 0) {
    if (remaining.charCodeAt(0) === 0x22 /* '"' */) {
      // Quoted section: find the closing quote, skipping escaped quotes.
      let i = 1;
      let closed = false;
      while (i < remaining.length) {
        if (remaining.charCodeAt(i) === 0x5c /* '\' */ && i + 1 < remaining.length && remaining.charCodeAt(i + 1) === 0x22) {
          i += 2; // skip the escaped quote
        } else if (remaining.charCodeAt(i) === 0x22) {
          parts.push(remaining.slice(0, i + 1));
          remaining = remaining.slice(i + 1);
          closed = true;
          break;
        } else {
          i += 1;
        }
      }
      if (!closed) {
        // Unclosed quote: emit the rest as-is.
        parts.push(remaining);
        remaining = '';
      }
    } else {
      // Unquoted section: run up to the next quote (or the end).
      const next = remaining.indexOf('"');
      const end = next === -1 ? remaining.length : next;
      // `end` is always > 0 here (index 0 is not a quote), so the part is non-empty.
      parts.push(remaining.slice(0, end));
      remaining = remaining.slice(end);
    }
  }

  return parts;
}

/**
 * Collapse whitespace runs into a single underscore.
 *
 * Handles the two-character escape sequences `\n` and `\r` (a backslash
 * followed by `n`/`r`) as well as actual whitespace characters (space, `\t`,
 * `\r`, `\n`, U+00A0); consecutive matches collapse to one underscore. A
 * backslash NOT starting an `\n`/`\r` escape passes through unchanged (and
 * resets the run). Faithful port of Rust's `replace_whitespace_with_underscore`.
 */
function replaceWhitespaceWithUnderscore(s: string): string {
  let result = '';
  let inWhitespace = false;

  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === '\\' && i + 1 < s.length && (s[i + 1] === 'n' || s[i + 1] === 'r')) {
      i += 1; // consume the 'n' or 'r'
      if (!inWhitespace) {
        result += '_';
        inWhitespace = true;
      }
    } else if (c === '\\') {
      // Not an escape sequence we handle; pass through.
      inWhitespace = false;
      result += c;
    } else if (c === '\n' || c === '\r' || c === '\t' || c === ' ' || c === '\u{00A0}') {
      if (!inWhitespace) {
        result += '_';
        inWhitespace = true;
      }
    } else {
      inWhitespace = false;
      result += c;
    }
  }

  return result;
}

/**
 * Canonicalize a variable/model name into the engine's normalized form.
 *
 * Reproduces Rust `simlin-engine/src/common.rs` `canonicalize` exactly so that
 * a raw caller name resolves to the same canonical key the wasm `WasmLayout`
 * (and the VM's `get_var_names`) uses. The steps, per quote-aware part:
 *   1. trim the whole input;
 *   2. split into quote-aware parts (a `.` does not split a quoted segment);
 *   3. per part: a quoted part's inner `.` -> U+2024 (literal-period sentinel),
 *      an unquoted part's `.` -> U+00B7 (module separator);
 *   4. per part: `\\` -> `\`, collapse whitespace runs (and the literal `\n`/`\r`
 *      escapes) to a single `_`, then lowercase;
 *   5. concatenate the parts (the sentinel/separator substitutions carry the join).
 *
 * This is intentionally a separate, fully-correct copy rather than reusing the
 * incomplete `@simlin/core` `canonicalize` (which has no dot/quote handling and
 * is shared by consumers whose behavior must not shift). The two should later
 * be unified into one Rust-faithful implementation.
 *
 * @param name The raw identifier to canonicalize.
 * @returns The canonical form.
 */
export function canonicalizeIdent(name: string): string {
  const trimmed = name.trim();

  let canonical = '';
  for (const part of splitIdentifierParts(trimmed)) {
    const isQuoted = part.length >= 2 && part.charCodeAt(0) === 0x22 && part.charCodeAt(part.length - 1) === 0x22;

    let mapped: string;
    if (isQuoted) {
      const inner = part.slice(1, part.length - 1);
      // A literal period inside quotes becomes the canonical-stable sentinel,
      // not a raw `.` (which would re-canonicalize into the module separator).
      mapped = inner.includes('.') ? inner.split('.').join(LITERAL_PERIOD_SENTINEL) : inner;
    } else {
      // An unquoted `.` is a module-hierarchy separator.
      mapped = part.split('.').join(MODULE_SEPARATOR);
    }

    mapped = mapped.split('\\\\').join('\\');
    mapped = replaceWhitespaceWithUnderscore(mapped);
    mapped = mapped.toLowerCase();

    canonical += mapped;
  }

  return canonical;
}
