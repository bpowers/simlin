// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

import { Project as EngineProject } from '@simlin/engine';

import { createRenderLimiter, renderToPNG } from '../render';
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
    // that lands before its `await` would otherwise be flagged by jest as an
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

  it('rejects a render whose total deadline lapsed while queued, without spawning a worker', async () => {
    // Occupy both slots with hung workers for ~300ms. The third render's
    // 50ms budget expires while it waits in the queue, so when a slot frees
    // it must fail fast with the distinct waiting-for-slot message -- even
    // though its worker script would succeed instantly if spawned.
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
