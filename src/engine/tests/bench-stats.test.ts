// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Unit tests for the pure benchmark harness: the median statistic and the
// adaptive warmup/measure policy. Both `body` and `now` are injected, so these
// tests are deterministic and never touch the wall clock or any model/WASM.

import { median, runTimed, runTimedAsync, type BenchOpts } from './bench-stats';

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
    // budget 50ms, clock advances 20ms per read. The loop checks the clock
    // before each push beyond minIters. minIters=2 is honored; then it keeps
    // going while elapsed < 50: clock reads at the guard are 0, 20, 40, 60...
    const opts: BenchOpts = { warmup: 0, minIters: 2, maxIters: 100, budgetMs: 50 };
    const body = sequenceBody([10, 11, 12, 13, 14, 15, 16, 17]);
    const stat = runTimed(opts, body, fakeClock(20));

    // The clock starts after the first read (used as `start`). minIters=2 push
    // unconditionally; subsequent iterations gate on elapsed < 50ms.
    expect(stat.iters).toBeGreaterThanOrEqual(2);
    expect(stat.iters).toBeLessThan(100);
    expect(Number.isFinite(stat.medianMs)).toBe(true);
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
    const opts: BenchOpts = { warmup: 0, minIters: 2, maxIters: 100, budgetMs: 50 };
    const body = asyncSequenceBody([10, 11, 12, 13, 14, 15, 16, 17]);
    const stat = await runTimedAsync(opts, body, fakeClock(20));

    expect(stat.iters).toBeGreaterThanOrEqual(2);
    expect(stat.iters).toBeLessThan(100);
    expect(Number.isFinite(stat.medianMs)).toBe(true);
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
