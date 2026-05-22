// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// The pure statistic + warmup/measure policy shared by the node VM-vs-wasm
// benchmark (backend-bench.ts). The timed `body` and the wall clock `now` are
// both injected, so this module is deterministic and side-effect-free: it never
// reads the clock itself nor touches a model/WASM. It mirrors the methodology
// of `src/simlin-engine/examples/backend_bench.rs` (the `Stat`/`bench` pair),
// adding the explicit warmup phase AC9.1 requires.

/** One phase's timing summary. `iters` makes the sample size visible so a
 *  single-iteration heavy phase is never silently treated as an average. */
export type Stat = {
  readonly medianMs: number;
  readonly minMs: number;
  readonly iters: number;
};

/**
 * The median of `times`, in milliseconds.
 *
 * Sorts a copy ascending (the input is left unmodified) and returns
 * `times[times.length >> 1]` -- the upper-middle element for an even length,
 * matching `backend_bench.rs:181`'s `times[iters/2]` (NOT an averaged median).
 * An empty input returns `NaN`.
 */
export function median(times: ReadonlyArray<number>): number {
  if (times.length === 0) {
    return NaN;
  }
  const sorted = [...times].sort((a, b) => a - b);
  return sorted[sorted.length >> 1];
}

/** The warmup + adaptive-measure policy. `warmup` iterations are discarded,
 *  then timings are collected until `maxIters` or (after `minIters`) the
 *  wall-clock `budgetMs` elapses. */
export type BenchOpts = {
  readonly warmup: number;
  readonly minIters: number;
  readonly maxIters: number;
  readonly budgetMs: number;
};

/**
 * Run `body` under the warmup/adaptive-measure policy and summarize the timings.
 *
 * `body()` returns the elapsed milliseconds of one measured run; it does any
 * per-iteration setup untimed and times only the precise region itself (exactly
 * as `backend_bench.rs`'s closure returns `ms_since(t0)`). This function does
 * NOT wrap `body` in a `now()` measurement -- `now` is consulted only for the
 * adaptive wall-clock budget, and is injectable so callers/tests can supply a
 * deterministic fake clock.
 *
 * First, `opts.warmup` iterations are run and discarded (the explicit warmup
 * AC9.1 requires). Then, with `start = now()`, timings are collected while
 * `times.length < maxIters && (times.length < minIters || now() - start < budgetMs)`.
 * So `minIters` is always honored (even if the budget is already spent) and the
 * loop never exceeds `maxIters`.
 */
export function runTimed(opts: Readonly<BenchOpts>, body: () => number, now: () => number = defaultNow): Stat {
  for (let i = 0; i < opts.warmup; i++) {
    body();
  }

  const times: Array<number> = [];
  const start = now();
  while (times.length < opts.maxIters && (times.length < opts.minIters || now() - start < opts.budgetMs)) {
    times.push(body());
  }

  return summarize(times);
}

/**
 * The async twin of {@link runTimed}: identical warmup-discard + adaptive-median
 * policy, but it `await`s `asyncBody()` each iteration. The benchmark's eval is
 * async (it drives the Promise-based public API), so keeping the policy here
 * single-sources it -- the imperative runner never hand-rolls an inline loop.
 */
export async function runTimedAsync(
  opts: Readonly<BenchOpts>,
  asyncBody: () => Promise<number>,
  now: () => number = defaultNow,
): Promise<Stat> {
  for (let i = 0; i < opts.warmup; i++) {
    await asyncBody();
  }

  const times: Array<number> = [];
  const start = now();
  while (times.length < opts.maxIters && (times.length < opts.minIters || now() - start < opts.budgetMs)) {
    times.push(await asyncBody());
  }

  return summarize(times);
}

/** The result of comparing two series element-wise. On a mismatch it carries
 *  the offending step (or -1 for a length mismatch) and the two values, so the
 *  caller can build a precise diagnostic. */
export type SeriesCloseResult =
  | { readonly match: true }
  | { readonly match: false; readonly index: number; readonly expected: number; readonly actual: number };

// VM-vs-wasm parity tolerance for the same salsa-compiled simulation. These are
// the engine's corpus-wide VM-vs-wasm bounds (src/simlin-engine/tests/test_helpers.rs
// `ensure_results`, the comparator the heavy C-LEARN/WORLD3 wasm tests clear),
// NOT the far tighter teacup-only 1e-9 in tests/wasm-model.test.ts: a large model
// run to its final time accumulates floating-point reassociation noise far above
// 1e-9 (e.g. C-LEARN's ~2.5e-9 on an O(0.1) value) that is benign, not a parity
// bug. The non-Vensim branch applies because this compares two engines' output of
// the SAME compiled model, not against Vensim-sourced reference data.
const PARITY_ABS_TOL = 2e-3;
const NEAR_ZERO_EXPECTED = 3e-6;
const NEAR_ZERO_ACTUAL = 1e-6;

/**
 * Element-wise comparison of two simulation series within the engine's VM-vs-wasm
 * parity tolerance (a faithful port of `ensure_results`'s non-Vensim, near-zero-
 * robust isclose). Returns a match, or the first divergence with its step index
 * and values. A length mismatch is reported with `index === -1`.
 *
 * Pure: no I/O, no clock. The benchmark's `crossCheck` is the imperative shell
 * that fetches the series and turns a non-match into a thrown error.
 */
export function seriesClose(expected: Readonly<Float64Array>, actual: Readonly<Float64Array>): SeriesCloseResult {
  if (expected.length !== actual.length) {
    return { match: false, index: -1, expected: expected.length, actual: actual.length };
  }
  for (let i = 0; i < expected.length; i++) {
    const e = expected[i];
    const a = actual[i];
    const aroundZero = Math.abs(e) <= NEAR_ZERO_EXPECTED && Math.abs(a) <= NEAR_ZERO_ACTUAL;
    if (aroundZero) {
      continue;
    }
    if (Math.abs(e - a) > PARITY_ABS_TOL) {
      return { match: false, index: i, expected: e, actual: a };
    }
  }
  return { match: true };
}

function summarize(times: ReadonlyArray<number>): Stat {
  const minMs = times.length === 0 ? NaN : Math.min(...times);
  return { medianMs: median(times), minMs, iters: times.length };
}

// performance.now() returns fractional milliseconds; this is the only wall-clock
// read in the module, and it is the injectable default so tests stay deterministic.
function defaultNow(): number {
  return performance.now();
}
