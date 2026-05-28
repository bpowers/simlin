// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure helpers for LTM test assertions shared across wasm-ltm.test.ts and
// worker-wasm.test.ts. Variable names in this project's canonical form never
// contain spaces, so a literal space is a safe and human-readable separator
// for composite link keys.

import type { Link } from '../src/types';

// LTM scores from VM-vs-wasm are produced by the same analysis function over
// the same per-step f64 series. 1e-6 is more permissive than the eval-parity
// 1e-9 in wasm-backend.test.ts to leave room for cumulative reassociation
// noise in the wasm evaluator.
export const LTM_SCORE_TOL = 1e-6;

export function expectScoresClose(actual: Float64Array, expected: Float64Array): void {
  expect(actual.length).toBe(expected.length);
  for (let i = 0; i < expected.length; i++) {
    expect(Math.abs(actual[i] - expected[i])).toBeLessThanOrEqual(LTM_SCORE_TOL);
  }
}

export function linkKey(link: Link): string {
  return link.from + ' ' + link.to;
}

export function linksByKey(links: readonly Link[]): Map<string, Link> {
  const out = new Map<string, Link>();
  for (const link of links) {
    out.set(linkKey(link), link);
  }
  return out;
}
