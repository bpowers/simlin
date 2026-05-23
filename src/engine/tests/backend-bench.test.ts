// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// Gated VM-vs-wasm eval benchmark over fishbanks / WORLD3 / C-LEARN, driven
// through the public Model.simulate({engine}) API. The heavy run is opt-in via
// RUN_BENCH=1 so it stays out of the default `pnpm test` (C-LEARN's compile
// alone is seconds, well past the few-seconds-per-test budget). The pure
// harness it relies on is always-on-tested in bench-stats.test.ts; nothing
// heavy runs by default here. The cross-check inside runBenchmark guards
// correctness; this asserts every model produced a positive finite median on
// both engines.

import { runBenchmark } from './backend-bench';

const RUN = process.env.RUN_BENCH === '1';

(RUN ? it : it.skip)(
  'benchmarks VM vs wasm eval (fishbanks/WORLD3/C-LEARN)',
  async () => {
    const rows = await runBenchmark({ warmup: 3, minIters: 3, maxIters: 100, budgetMs: 2500 });
    for (const r of rows) {
      expect(Number.isFinite(r.vm.medianMs)).toBe(true);
      expect(r.vm.medianMs).toBeGreaterThan(0);
      expect(Number.isFinite(r.wasm.medianMs)).toBe(true);
      expect(r.wasm.medianMs).toBeGreaterThan(0);
    }
  },
  300_000,
);
