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

function summarize(times: ReadonlyArray<number>): Stat {
  const minMs = times.length === 0 ? NaN : Math.min(...times);
  return { medianMs: median(times), minMs, iters: times.length };
}

// performance.now() returns fractional milliseconds; this is the only wall-clock
// read in the module, and it is the injectable default so tests stay deterministic.
function defaultNow(): number {
  return performance.now();
}
