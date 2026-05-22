// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Unit tests for the pure benchmark harness: the median statistic and the
// adaptive warmup/measure policy. Both `body` and `now` are injected, so these
// tests are deterministic and never touch the wall clock or any model/WASM.

import { median, runTimed, runTimedAsync, seriesClose, type BenchOpts } from './bench-stats';

describe('median', () => {
  it('returns the middle element of an odd-length input', () => {
    // length 3, len>>1 == 1: the middle of the sorted copy.
    expect(median([3, 1, 2])).toBe(2);
  });

  it('returns the upper-middle element of an even-length input (len>>1)', () => {
    // length 4, len>>1 == 2: the third element of the sorted copy (mirrors
    // backend_bench.rs `times[iters/2]`, NOT an averaged median).
    expect(median([4, 1, 3, 2])).toBe(3);
  });

  it('sorts a copy ascending and does not mutate the input', () => {
    const input = [5, 2, 9, 1];
    const result = median(input);
    // sorted: [1, 2, 5, 9], len>>1 == 2 -> 5
    expect(result).toBe(5);
    expect(input).toEqual([5, 2, 9, 1]);
  });

  it('returns the single element for a one-element input', () => {
    expect(median([42])).toBe(42);
  });

  it('returns NaN for an empty input', () => {
    expect(Number.isNaN(median([]))).toBe(true);
  });
});

// A body that returns a fixed sequence of "elapsed ms" values, advancing one
// element per call. Lets a test assert exactly which iterations were measured
// vs. discarded as warmup.
function sequenceBody(values: ReadonlyArray<number>): () => number {
  let i = 0;
  return () => {
    const v = values[i] ?? 0;
    i += 1;
    return v;
  };
}

// A monotonic fake clock: each read advances by `step` ms. Independent of
// `body`, mirroring how the real loop samples wall-clock between iterations.
function fakeClock(step: number, startAt = 0): () => number {
  let t = startAt;
  return () => {
    const cur = t;
    t += step;
    return cur;
  };
}

describe('runTimed', () => {
  it('discards exactly `warmup` iterations before measuring', () => {
    // warmup=2 discards the first two (100, 200); measurement sees 3,4,5.
    const opts: BenchOpts = { warmup: 2, minIters: 3, maxIters: 3, budgetMs: 1e9 };
    const body = sequenceBody([100, 200, 3, 4, 5]);
    const stat = runTimed(opts, body, fakeClock(0));

    expect(stat.iters).toBe(3);
    // median of [3,4,5] (len>>1==1) is 4; min is 3.
    expect(stat.medianMs).toBe(4);
    expect(stat.minMs).toBe(3);
  });

  it('stops at maxIters even when the budget allows more', () => {
    const opts: BenchOpts = { warmup: 0, minIters: 1, maxIters: 4, budgetMs: 1e9 };
    const body = sequenceBody([1, 2, 3, 4, 5, 6]);
    const stat = runTimed(opts, body, fakeClock(0));

    expect(stat.iters).toBe(4);
    // median of [1,2,3,4] (len>>1==2) is 3.
    expect(stat.medianMs).toBe(3);
    expect(stat.minMs).toBe(1);
  });

  it('stops early once the budget is exceeded after minIters', () => {
    // budget 50ms, clock advances 20ms per read. Deterministic trace with
    // fakeClock(20): the first read is consumed as `start` (=0); the first two
    // pushes short-circuit on `times.length < minIters` so the clock is NOT
    // read. From iteration 3 the guard reads the clock: 20 (<50 push), 40
    // (<50 push), 60 (>=50 stop). So the loop runs EXACTLY 4 iterations -- the
    // exact count is pinned so an off-by-one in the budget guard (`<` -> `<=`)
    // is caught (a loose `>= 2 && < 100` would not notice it).
    const opts: BenchOpts = { warmup: 0, minIters: 2, maxIters: 100, budgetMs: 50 };
    const body = sequenceBody([10, 11, 12, 13, 14, 15, 16, 17]);
    const stat = runTimed(opts, body, fakeClock(20));

    expect(stat.iters).toBe(4);
    // measured = [10, 11, 12, 13]; sorted, len>>1==2 -> 12; min is 10.
    expect(stat.medianMs).toBe(12);
    expect(stat.minMs).toBe(10);
  });

  it('honors minIters even when the budget is already exceeded', () => {
    // budget is 0ms, so the wall-clock predicate is false from the start; only
    // minIters keeps the loop running. It must still collect exactly minIters.
    const opts: BenchOpts = { warmup: 0, minIters: 3, maxIters: 100, budgetMs: 0 };
    const body = sequenceBody([7, 8, 9, 10, 11]);
    const stat = runTimed(opts, body, fakeClock(1000));

    expect(stat.iters).toBe(3);
    // median of [7,8,9] is 8.
    expect(stat.medianMs).toBe(8);
    expect(stat.minMs).toBe(7);
  });

  it('reports correct medianMs, minMs, and iters together', () => {
    const opts: BenchOpts = { warmup: 1, minIters: 5, maxIters: 5, budgetMs: 1e9 };
    // first value discarded as warmup; measured = [9, 3, 7, 1, 5].
    const body = sequenceBody([99, 9, 3, 7, 1, 5]);
    const stat = runTimed(opts, body, fakeClock(0));

    expect(stat.iters).toBe(5);
    // sorted measured: [1,3,5,7,9], len>>1==2 -> 5
    expect(stat.medianMs).toBe(5);
    expect(stat.minMs).toBe(1);
  });
});

// The async twin of sequenceBody: resolves the same fixed sequence.
function asyncSequenceBody(values: ReadonlyArray<number>): () => Promise<number> {
  let i = 0;
  return () => {
    const v = values[i] ?? 0;
    i += 1;
    return Promise.resolve(v);
  };
}

describe('runTimedAsync', () => {
  it('discards exactly `warmup` iterations before measuring', async () => {
    const opts: BenchOpts = { warmup: 2, minIters: 3, maxIters: 3, budgetMs: 1e9 };
    const body = asyncSequenceBody([100, 200, 3, 4, 5]);
    const stat = await runTimedAsync(opts, body, fakeClock(0));

    expect(stat.iters).toBe(3);
    expect(stat.medianMs).toBe(4);
    expect(stat.minMs).toBe(3);
  });

  it('stops at maxIters even when the budget allows more', async () => {
    const opts: BenchOpts = { warmup: 0, minIters: 1, maxIters: 4, budgetMs: 1e9 };
    const body = asyncSequenceBody([1, 2, 3, 4, 5, 6]);
    const stat = await runTimedAsync(opts, body, fakeClock(0));

    expect(stat.iters).toBe(4);
    expect(stat.medianMs).toBe(3);
    expect(stat.minMs).toBe(1);
  });

  it('stops early once the budget is exceeded after minIters', async () => {
    // Same deterministic trace as the sync twin: `start`=0, the first two
    // pushes short-circuit (no clock read), then guards read 20, 40, 60 and
    // stop at 60 >= 50. Exactly 4 iterations -- pinned to catch an off-by-one
    // in the (byte-identical) async budget guard.
    const opts: BenchOpts = { warmup: 0, minIters: 2, maxIters: 100, budgetMs: 50 };
    const body = asyncSequenceBody([10, 11, 12, 13, 14, 15, 16, 17]);
    const stat = await runTimedAsync(opts, body, fakeClock(20));

    expect(stat.iters).toBe(4);
    // measured = [10, 11, 12, 13]; sorted, len>>1==2 -> 12; min is 10.
    expect(stat.medianMs).toBe(12);
    expect(stat.minMs).toBe(10);
  });

  it('honors minIters even when the budget is already exceeded', async () => {
    const opts: BenchOpts = { warmup: 0, minIters: 3, maxIters: 100, budgetMs: 0 };
    const body = asyncSequenceBody([7, 8, 9, 10, 11]);
    const stat = await runTimedAsync(opts, body, fakeClock(1000));

    expect(stat.iters).toBe(3);
    expect(stat.medianMs).toBe(8);
    expect(stat.minMs).toBe(7);
  });

  it('reports correct medianMs, minMs, and iters together', async () => {
    const opts: BenchOpts = { warmup: 1, minIters: 5, maxIters: 5, budgetMs: 1e9 };
    const body = asyncSequenceBody([99, 9, 3, 7, 1, 5]);
    const stat = await runTimedAsync(opts, body, fakeClock(0));

    expect(stat.iters).toBe(5);
    expect(stat.medianMs).toBe(5);
    expect(stat.minMs).toBe(1);
  });
});

describe('seriesClose', () => {
  it('reports two identical series as matching', () => {
    const a = new Float64Array([1, 2, 3]);
    const b = new Float64Array([1, 2, 3]);
    expect(seriesClose(a, b).match).toBe(true);
  });

  it('treats differences within the absolute tolerance as matching', () => {
    // 1e-4 < 2e-3 absolute tolerance.
    const a = new Float64Array([1.0, 100.0]);
    const b = new Float64Array([1.0001, 100.0001]);
    expect(seriesClose(a, b).match).toBe(true);
  });

  it('treats near-zero noise as matching', () => {
    // both within the near-zero guard (expected <= 3e-6, actual <= 1e-6).
    const a = new Float64Array([0.0, 1e-7]);
    const b = new Float64Array([5e-7, 0.0]);
    expect(seriesClose(a, b).match).toBe(true);
  });

  it('matches the real C-LEARN VM-vs-wasm reassociation noise', () => {
    // The exact divergence observed: |diff| ~ 2.47e-9 on a value ~0.153 is
    // floating-point reassociation noise, six orders of magnitude inside the
    // engine's 2e-3 absolute VM-vs-wasm parity tolerance -- a match, not a bug.
    const a = new Float64Array([0.15306828340588152]);
    const b = new Float64Array([0.15306828094062933]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(true);
  });

  it('reports a genuine divergence beyond tolerance, with the offending index and values', () => {
    const a = new Float64Array([1.0, 2.0, 3.0]);
    const b = new Float64Array([1.0, 2.5, 3.0]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(false);
    if (!result.match) {
      expect(result.index).toBe(1);
      expect(result.expected).toBe(2.0);
      expect(result.actual).toBe(2.5);
    }
  });

  it('reports a length mismatch (index -1)', () => {
    const a = new Float64Array([1, 2, 3]);
    const b = new Float64Array([1, 2]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(false);
    if (!result.match) {
      expect(result.index).toBe(-1);
    }
  });

  it('reports NaN-vs-finite as a mismatch at the offending index', () => {
    // The Rust oracle (`ensure_results`) PANICS on this: a NaN is never
    // around-zero and approx_eq!(NaN, finite) is false. A naive
    // `Math.abs(NaN - finite) > tol` is NaN > tol === false, which would
    // wrongly wave it through -- this pins the faithful rejection.
    const a = new Float64Array([1.0, NaN, 3.0]);
    const b = new Float64Array([1.0, 2.0, 3.0]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(false);
    if (!result.match) {
      expect(result.index).toBe(1);
      expect(Number.isNaN(result.expected)).toBe(true);
      expect(result.actual).toBe(2.0);
    }
  });

  it('reports finite-vs-NaN as a mismatch at the offending index', () => {
    // Symmetric to the above: NaN on the `actual` side is equally a mismatch.
    const a = new Float64Array([1.0, 2.0, 3.0]);
    const b = new Float64Array([1.0, NaN, 3.0]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(false);
    if (!result.match) {
      expect(result.index).toBe(1);
      expect(result.expected).toBe(2.0);
      expect(Number.isNaN(result.actual)).toBe(true);
    }
  });

  it('reports NaN-vs-NaN as a mismatch (faithful to approx_eq! on NaN)', () => {
    // approx_eq!(NaN, NaN) is false, so the Rust oracle would PANIC even
    // here. A parity comparison must never silently accept NaN.
    const a = new Float64Array([1.0, NaN]);
    const b = new Float64Array([1.0, NaN]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(false);
    if (!result.match) {
      expect(result.index).toBe(1);
    }
  });

  it('reports +Infinity-vs-finite as a mismatch', () => {
    // A non-finite value on either side (Infinity as well as NaN) is a
    // broken run the cross-check exists to reject.
    const a = new Float64Array([1.0, Infinity]);
    const b = new Float64Array([1.0, 2.0]);
    const result = seriesClose(a, b);
    expect(result.match).toBe(false);
    if (!result.match) {
      expect(result.index).toBe(1);
    }
  });
});
