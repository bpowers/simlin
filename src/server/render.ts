// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';
import { Worker } from 'worker_threads';

import { File } from './schemas/file_pb';
import type { RenderWorkerData, RenderWorkerResult } from './render-worker';

// Re-exported so existing consumers (and their tests) keep a single import
// surface; the implementations live in preview-geometry.ts so the render
// worker can use them without loading this module.
export { previewDimensions, parseSvgDimensions } from './preview-geometry';

/**
 * TOTAL wall-clock budget for one preview request, measured from the
 * renderToPNG call -- it covers time queued for a render slot plus the render
 * itself, enforced by terminating the worker. Generous -- a routine preview,
 * including the worker compiling its own WASM instance, finishes well under a
 * second -- but bounded, so a pathological or adversarial model costs one
 * failed request instead of pinning a render slot (and, before issue #694,
 * the whole event loop).
 */
export const RENDER_TIMEOUT_MS = 10_000;

/**
 * At most this many render workers at once. The cap bounds worker fan-out
 * and CPU contention with the Express event loop while still letting renders
 * overlap. It does NOT make worst-case memory survivable: each worker's WASM
 * memory can grow to the 1 GiB module cap, and two maxed-out workers exceed
 * an F4's RAM -- that exposure predates the worker split; it's now contained
 * to an instance restart (with app.yaml max_instances capping the cost)
 * instead of undefined in-process behavior. Excess renders queue FIFO; queue
 * depth is implicitly bounded because every waiting render is an in-flight
 * HTTP request and GAE caps those at max_concurrent_requests (100).
 */
const MAX_CONCURRENT_RENDERS = 2;

export interface RenderLimiter {
  run<T>(task: () => Promise<T>): Promise<T>;
}

/**
 * Minimal FIFO concurrency limiter. A slot is released when the task settles
 * (resolve or reject), so a failed or timed-out render can never leak a slot.
 * Exported for direct unit testing.
 */
export function createRenderLimiter(maxConcurrent: number): RenderLimiter {
  if (!Number.isInteger(maxConcurrent) || maxConcurrent <= 0) {
    throw new Error(`maxConcurrent must be a positive integer, got ${maxConcurrent}`);
  }

  let active = 0;
  const waiters: Array<() => void> = [];

  const acquire = (): Promise<void> => {
    if (active < maxConcurrent) {
      active++;
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      waiters.push(() => {
        active++;
        resolve();
      });
    });
  };

  const release = (): void => {
    active--;
    const next = waiters.shift();
    if (next) {
      next();
    }
  };

  return {
    async run<T>(task: () => Promise<T>): Promise<T> {
      await acquire();
      try {
        return await task();
      } finally {
        release();
      }
    },
  };
}

const renderLimiter = createRenderLimiter(MAX_CONCURRENT_RENDERS);

/** Test-only overrides; production callers pass no options. */
export interface RenderOptions {
  /** Override RENDER_TIMEOUT_MS (e.g. to exercise the timeout path fast). */
  timeoutMs?: number;
  /** Override the worker entry (e.g. a hanging or failing fixture). */
  workerScript?: string;
}

/**
 * Locate the compiled worker entry. In production this module runs from the
 * compiled lib/, so render-worker.js is a sibling. Under ts-jest __dirname is
 * the source directory, where only render-worker.ts exists; fall back to the
 * compiled copy under lib/ (produced by `pnpm build` -- tests that need it
 * skip when it's absent).
 */
function resolveWorkerScript(): string {
  const candidates = [path.join(__dirname, 'render-worker.js'), path.join(__dirname, 'lib', 'render-worker.js')];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  throw new Error(
    `render-worker.js not found (looked at ${candidates.join(', ')}); ` +
      'run `pnpm --filter @simlin/server run build`',
  );
}

/**
 * Spawn a worker for one render and settle exactly once: on the worker's
 * result message, its 'error'/'messageerror'/'exit' events, or the deadline.
 * The worker is terminated on every settle path -- terminate() is idempotent
 * and a no-op on an already-exited thread, so unconditional termination is
 * the simplest way to guarantee no thread outlives its request.
 *
 * `deadline` is an epoch-ms timestamp captured before the render queued for a
 * slot; the worker only gets whatever budget remains. `timeoutMs` is the
 * original total budget, used for the error message.
 */
function runRenderWorker(
  projectContents: Uint8Array,
  deadline: number,
  timeoutMs: number,
  workerScript: string,
): Promise<Uint8Array> {
  return new Promise<Uint8Array>((resolve, reject) => {
    const data: RenderWorkerData = { projectContents };
    const worker = new Worker(workerScript, { workerData: data });
    let settled = false;

    const settle = (outcome: () => void): void => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      void worker.terminate();
      outcome();
    };

    const timer = setTimeout(
      () => {
        settle(() => reject(new Error(`preview render timed out after ${timeoutMs}ms`)));
      },
      Math.max(0, deadline - Date.now()),
    );

    worker.on('message', (result: RenderWorkerResult) => {
      if (result.ok) {
        settle(() => resolve(result.png));
      } else {
        settle(() => reject(new Error(result.error)));
      }
    });
    worker.on('error', (err) => {
      settle(() => reject(err));
    });
    worker.on('messageerror', (err) => {
      // A result that fails to deserialize would otherwise leave us waiting
      // out the deadline and reporting a misleading "timed out".
      settle(() => reject(new Error(`render worker result could not be deserialized: ${err.message}`)));
    });
    worker.on('exit', (code) => {
      // Reached only if the worker exits before posting a result (pending
      // messages are delivered ahead of 'exit'; settle() dedupes regardless).
      settle(() => reject(new Error(`render worker exited with code ${code} before producing a result`)));
    });
  });
}

/**
 * Render a project file's `main` model to a preview PNG in an isolated,
 * per-request worker thread. The timeout is a TOTAL wall-clock budget from
 * this call, covering both queueing for a render slot and the render itself.
 * Failures (bad model, worker crash, timeout) reject; callers translate that
 * into a 500 for the one affected request.
 */
export async function renderToPNG(fileDoc: File, options?: RenderOptions): Promise<Uint8Array> {
  const projectContents = fileDoc.getProjectContents_asU8();
  const timeoutMs = options?.timeoutMs ?? RENDER_TIMEOUT_MS;
  const workerScript = options?.workerScript ?? resolveWorkerScript();
  // Capture the deadline before enqueueing so queue wait counts against the
  // budget: the client has been waiting the whole time, and a request whose
  // deadline lapsed in the queue must not burn a slot on a doomed render.
  const deadline = Date.now() + timeoutMs;
  return renderLimiter.run(() => {
    if (Date.now() >= deadline) {
      return Promise.reject(new Error(`preview render timed out after ${timeoutMs}ms waiting for a render slot`));
    }
    return runRenderWorker(projectContents, deadline, timeoutMs, workerScript);
  });
}
