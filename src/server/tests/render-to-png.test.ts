// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import { Project as EngineProject } from '@simlin/engine';

import { createRenderLimiter, renderToPNG, RenderSlotTimeoutError } from '../render';
import { File } from '../schemas/file_pb';

const FIXTURES_DIR = path.join(__dirname, 'fixtures');

function fixture(name: string): string {
  return path.join(FIXTURES_DIR, name);
}

function makeFile(contents: Uint8Array): File {
  const file = new File();
  file.setProjectContents(contents);
  return file;
}

// Assert a settled promise rejected and return its stringified reason
// (e.g. "Error: preview render timed out after 300ms").
function rejectionMessage(result: PromiseSettledResult<unknown>): string {
  expect(result.status).toBe('rejected');
  return String((result as PromiseRejectedResult).reason);
}

// Resolve one macrotask so already-runnable limiter tasks get a chance to
// start before we assert on which of them ran.
function flush(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

interface Deferred {
  promise: Promise<void>;
  resolve: () => void;
  reject: (err: Error) => void;
}

function deferred(): Deferred {
  let resolve!: () => void;
  let reject!: (err: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// Observe a promise's settlement state without awaiting it. Attaching the
// rejection handler up front also keeps an early rejection from tripping the
// runner's unhandled-rejection detection before the test awaits the promise.
function track<T>(promise: Promise<T>): { promise: Promise<T>; state: () => 'pending' | 'fulfilled' | 'rejected' } {
  let state: 'pending' | 'fulfilled' | 'rejected' = 'pending';
  promise.then(
    () => {
      state = 'fulfilled';
    },
    () => {
      state = 'rejected';
    },
  );
  return { promise, state: () => state };
}

describe('createRenderLimiter', () => {
  it('rejects a non-positive concurrency cap', () => {
    expect(() => createRenderLimiter(0)).toThrow(/positive/);
    expect(() => createRenderLimiter(-1)).toThrow(/positive/);
  });

  it('runs tasks up to the cap and queues the rest in FIFO order', async () => {
    const limiter = createRenderLimiter(2);
    const started: string[] = [];
    const gates = { a: deferred(), b: deferred(), c: deferred(), d: deferred() };

    const task = (name: keyof typeof gates) => () => {
      started.push(name);
      return gates[name].promise;
    };

    const runs = [limiter.run(task('a')), limiter.run(task('b')), limiter.run(task('c')), limiter.run(task('d'))];
    await flush();
    expect(started).toEqual(['a', 'b']);

    gates.b.resolve();
    await runs[1];
    await flush();
    expect(started).toEqual(['a', 'b', 'c']);

    gates.a.resolve();
    await runs[0];
    await flush();
    expect(started).toEqual(['a', 'b', 'c', 'd']);

    gates.c.resolve();
    gates.d.resolve();
    await Promise.all(runs);
  });

  it('releases the slot when a task rejects', async () => {
    const limiter = createRenderLimiter(1);
    await expect(limiter.run(() => Promise.reject(new Error('first fails')))).rejects.toThrow('first fails');
    await expect(limiter.run(() => Promise.resolve('second runs'))).resolves.toBe('second runs');
  });

  it('propagates task results', async () => {
    const limiter = createRenderLimiter(1);
    await expect(limiter.run(() => Promise.resolve(42))).resolves.toBe(42);
  });
});

describe('createRenderLimiter deadlines', () => {
  it('rejects an already-expired deadline without running the task or taking a slot', async () => {
    const limiter = createRenderLimiter(1);
    let ran = false;
    await expect(
      limiter.run(() => {
        ran = true;
        return Promise.resolve();
      }, Date.now() - 1),
    ).rejects.toThrow(RenderSlotTimeoutError);
    expect(ran).toBe(false);

    // The refusal must not have consumed the (free) slot.
    await expect(limiter.run(() => Promise.resolve('probe'))).resolves.toBe('probe');
  });

  it('rejects a queued waiter at its deadline while every slot is still busy', async () => {
    const limiter = createRenderLimiter(2);
    const gate = deferred();
    const holders = [track(limiter.run(() => gate.promise)), track(limiter.run(() => gate.promise))];
    let ran = false;

    // Before issue #929 this promise stayed pending until a holder released
    // its slot; with no timer of its own the await below would hang.
    await expect(
      limiter.run(() => {
        ran = true;
        return Promise.resolve();
      }, Date.now() + 30),
    ).rejects.toThrow(RenderSlotTimeoutError);

    // The rejection came from the deadline, not from a freed slot: both
    // holders are still running and the queued task never started.
    expect(ran).toBe(false);
    expect(holders[0].state()).toBe('pending');
    expect(holders[1].state()).toBe('pending');

    gate.resolve();
    await Promise.all(holders.map((h) => h.promise));
  }, 2_000);

  it('a timed-out waiter never runs and hands its queue position to the next live waiter', async () => {
    const limiter = createRenderLimiter(1);
    const started: string[] = [];
    const gateA = deferred();
    const gateC = deferred();

    const runA = limiter.run(() => {
      started.push('a');
      return gateA.promise;
    });
    const runB = track(
      limiter.run(() => {
        started.push('b');
        return Promise.resolve();
      }, Date.now() + 20),
    );
    const runC = limiter.run(() => {
      started.push('c');
      return gateC.promise;
    });

    await expect(runB.promise).rejects.toThrow(RenderSlotTimeoutError);
    expect(started).toEqual(['a']);

    // When A's slot frees it must go to C, the next live waiter -- not be
    // stranded on (or burned by) the already-rejected B.
    gateA.resolve();
    await runA;
    await flush();
    expect(started).toEqual(['a', 'c']);

    gateC.resolve();
    await runC;

    // Slot accounting is intact: on this cap-1 limiter a fresh task starts
    // without waiting, so B's timeout neither leaked nor double-freed a slot.
    let probeStarted = false;
    const probe = limiter.run(() => {
      probeStarted = true;
      return Promise.resolve();
    });
    await flush();
    expect(probeStarted).toBe(true);
    await probe;
  }, 2_000);

  it('mixed-deadline queue: expired waiters reject at their own deadlines, live ones run FIFO', async () => {
    const limiter = createRenderLimiter(2);
    const started: string[] = [];
    const rejectedOrder: string[] = [];
    let running = 0;
    let maxRunning = 0;

    const gates = { h1: deferred(), h2: deferred(), w2: deferred(), w4: deferred() };
    const gatedTask = (name: keyof typeof gates) => async () => {
      started.push(name);
      running++;
      maxRunning = Math.max(maxRunning, running);
      try {
        await gates[name].promise;
      } finally {
        running--;
      }
    };
    const instantTask = (name: string) => () => {
      started.push(name);
      return Promise.resolve();
    };

    const h1 = limiter.run(gatedTask('h1'));
    const h2 = limiter.run(gatedTask('h2'));
    // Queue [expired, live, expired, live]: w1/w3's deadlines lapse while
    // they wait; w2/w4 stay live until a slot frees.
    const w1 = track(limiter.run(instantTask('w1'), Date.now() + 15));
    const w2 = limiter.run(gatedTask('w2'), Date.now() + 5_000);
    const w3 = track(limiter.run(instantTask('w3'), Date.now() + 30));
    const w4 = limiter.run(gatedTask('w4'));

    void w1.promise.catch(() => rejectedOrder.push('w1'));
    void w3.promise.catch(() => rejectedOrder.push('w3'));

    await expect(w1.promise).rejects.toThrow(RenderSlotTimeoutError);
    await expect(w3.promise).rejects.toThrow(RenderSlotTimeoutError);
    // Node fires timers in expiry order, so the 15ms deadline rejects first,
    // and both rejected while the holders still owned every slot.
    expect(rejectedOrder).toEqual(['w1', 'w3']);
    expect(started).toEqual(['h1', 'h2']);

    gates.h1.resolve();
    await h1;
    await flush();
    expect(started).toEqual(['h1', 'h2', 'w2']);

    gates.h2.resolve();
    await h2;
    await flush();
    expect(started).toEqual(['h1', 'h2', 'w2', 'w4']);

    gates.w2.resolve();
    gates.w4.resolve();
    await Promise.all([w2, w4]);
    expect(maxRunning).toBe(2);
  }, 2_000);

  // The deadline timer and the slot release land in the same timer turn. Node
  // fires same-expiry timers in creation order, but rather than pin who wins,
  // assert the invariants that must hold either way: the waiter settles
  // exactly one way (ran-and-fulfilled XOR rejected-without-running) and the
  // slot is neither leaked nor double-granted. The two cases flip the
  // creation order to steer the race toward each winner.
  const raceCase = (timerFirst: boolean) => async () => {
    const limiter = createRenderLimiter(1);
    const gate = deferred();
    const holder = limiter.run(() => gate.promise);

    const delayMs = 20;
    let ran = false;
    if (!timerFirst) {
      setTimeout(() => gate.resolve(), delayMs);
    }
    const waiter = limiter.run(() => {
      ran = true;
      return Promise.resolve();
    }, Date.now() + delayMs);
    if (timerFirst) {
      setTimeout(() => gate.resolve(), delayMs);
    }

    const [holderResult, waiterResult] = await Promise.allSettled([holder, waiter]);
    expect(holderResult.status).toBe('fulfilled');
    if (waiterResult.status === 'fulfilled') {
      expect(ran).toBe(true);
    } else {
      expect(ran).toBe(false);
      expect((waiterResult as PromiseRejectedResult).reason).toBeInstanceOf(RenderSlotTimeoutError);
    }

    // Whichever side won, the slot must be free again: a new task on this
    // cap-1 limiter starts without waiting.
    let probeStarted = false;
    const probe = limiter.run(() => {
      probeStarted = true;
      return Promise.resolve();
    });
    await flush();
    expect(probeStarted).toBe(true);
    await probe;
  };

  it('deadline firing as a slot frees settles once and leaks nothing (timer scheduled first)', raceCase(true), 2_000);
  it(
    'deadline firing as a slot frees settles once and leaks nothing (release scheduled first)',
    raceCase(false),
    2_000,
  );
});

describe('renderToPNG worker orchestration', () => {
  it('resolves with the bytes the worker posts back', async () => {
    const contents = new Uint8Array([1, 2, 3, 4, 5]);
    const png = await renderToPNG(makeFile(contents), { workerScript: fixture('worker-success.js') });
    expect(Array.from(png)).toEqual([1, 2, 3, 4, 5]);
  });

  it('rejects when the worker reports a render failure', async () => {
    await expect(
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-error-result.js') }),
    ).rejects.toThrow('boom: intentional render failure');
  });

  it('rejects when the worker dies with an uncaught exception', async () => {
    await expect(
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-throw.js') }),
    ).rejects.toThrow('worker exploded');
  });

  it('rejects when the worker exits without producing a result', async () => {
    await expect(
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-exit.js') }),
    ).rejects.toThrow(/exited with code 7/);
  });

  it('rejects when the worker script does not exist', async () => {
    await expect(
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('does-not-exist.js') }),
    ).rejects.toThrow();
  });

  it('times out a hung worker', async () => {
    await expect(
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-hang.js'), timeoutMs: 200 }),
    ).rejects.toThrow(/timed out after 200ms/);
  });

  it('releases both render slots after timed-out renders (no slot leak)', async () => {
    // Saturate the concurrency cap (2) with hung workers; if a timeout leaked
    // its slot, the follow-up success render below would never start.
    // allSettled (not sequential awaits) so every rejection has a handler
    // attached from the start -- both timers fire together, and a rejection
    // that lands before its `await` would otherwise be flagged by the runner as an
    // unhandled rejection.
    const hung = await Promise.allSettled([
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-hang.js'), timeoutMs: 150 }),
      renderToPNG(makeFile(new Uint8Array([2])), { workerScript: fixture('worker-hang.js'), timeoutMs: 150 }),
    ]);
    expect(rejectionMessage(hung[0])).toMatch(/timed out/);
    expect(rejectionMessage(hung[1])).toMatch(/timed out/);

    const contents = new Uint8Array([9, 9, 9]);
    const png = await renderToPNG(makeFile(contents), { workerScript: fixture('worker-success.js') });
    expect(Array.from(png)).toEqual([9, 9, 9]);
  });

  it('rejects a queued render at its own deadline while both slots are still busy', async () => {
    // Two hung renders own both slots for ~500ms; the third request's entire
    // 50ms budget elapses in the queue. Before issue #929 its promise simply
    // stayed pending until a slot freed; now the limiter's deadline timer
    // must reject it while the hung renders are still in flight.
    const hangs = [
      track(renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-hang.js'), timeoutMs: 500 })),
      track(renderToPNG(makeFile(new Uint8Array([2])), { workerScript: fixture('worker-hang.js'), timeoutMs: 500 })),
    ];

    await expect(
      renderToPNG(makeFile(new Uint8Array([3])), { workerScript: fixture('worker-success.js'), timeoutMs: 50 }),
    ).rejects.toThrow(/timed out after 50ms waiting for a render slot/);

    // Rejected at its own deadline, not because a slot freed.
    expect(hangs[0].state()).toBe('pending');
    expect(hangs[1].state()).toBe('pending');

    const settled = await Promise.allSettled(hangs.map((h) => h.promise));
    expect(rejectionMessage(settled[0])).toMatch(/timed out after 500ms/);
    expect(rejectionMessage(settled[1])).toMatch(/timed out after 500ms/);
  });

  it('rejects a render whose total deadline lapsed while queued, without spawning a worker', async () => {
    // Occupy both slots with hung workers for ~300ms. The third render's
    // 50ms budget expires while it waits in the queue, so the limiter's
    // deadline timer fails it with the distinct waiting-for-slot message --
    // even though its worker script would succeed instantly if spawned.
    // allSettled for the same unhandled-rejection reason as above.
    const results = await Promise.allSettled([
      renderToPNG(makeFile(new Uint8Array([1])), { workerScript: fixture('worker-hang.js'), timeoutMs: 300 }),
      renderToPNG(makeFile(new Uint8Array([2])), { workerScript: fixture('worker-hang.js'), timeoutMs: 300 }),
      renderToPNG(makeFile(new Uint8Array([3])), { workerScript: fixture('worker-success.js'), timeoutMs: 50 }),
    ]);
    expect(rejectionMessage(results[0])).toMatch(/timed out after 300ms/);
    expect(rejectionMessage(results[1])).toMatch(/timed out after 300ms/);
    expect(rejectionMessage(results[2])).toMatch(/timed out after 50ms waiting for a render slot/);
  });
});

// End-to-end: spawn the REAL compiled worker (lib/render-worker.js), which
// instantiates its own engine WASM inside the thread -- proving the WASM path
// resolution works from a worker the same way it does on the main thread.
// `pnpm build` produces lib/; skip (not fail) when it's absent so the suite
// stays runnable on a source-only checkout. CI and pre-commit build first, so
// this always runs there. Note the compiled worker can be stale relative to
// render-worker.ts; the pipeline logic itself is tested from source in
// render-model-preview.test.ts.
const builtWorker = path.join(__dirname, '..', 'lib', 'render-worker.js');
const describeIfBuilt = fs.existsSync(builtWorker) ? describe : describe.skip;
if (!fs.existsSync(builtWorker)) {
  console.warn(
    `[render-to-png] skipping real-worker e2e: ${builtWorker} not found; run \`pnpm --filter @simlin/server run build\`.`,
  );
}

describeIfBuilt('renderToPNG end to end (real worker, real WASM)', () => {
  it('renders the population default project to a bounded PNG', async () => {
    const modelPath = path.join(__dirname, '..', '..', '..', 'default_projects', 'population', 'model.xmile');
    const xmile = fs.readFileSync(modelPath, 'utf8');
    const importProject = await EngineProject.open(xmile);
    const protobuf = await importProject.serializeProtobuf();
    await importProject.dispose();

    // No options: exercises the default worker-script resolution and timeout.
    const png = await renderToPNG(makeFile(protobuf));

    // PNG signature
    expect(png[0]).toBe(137);
    expect(png[1]).toBe(80); // P
    expect(png[2]).toBe(78); // N
    expect(png[3]).toBe(71); // G

    // IHDR width/height are big-endian at offsets 16/20
    const buffer = Buffer.from(png);
    expect(buffer.readUInt32BE(16)).toBeLessThanOrEqual(800);
    expect(buffer.readUInt32BE(20)).toBeLessThanOrEqual(800);
  }, 20_000); // the worker compiles its own WASM instance; allow headroom on slow machines
});
