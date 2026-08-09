// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';
import { Worker } from 'worker_threads';

import { File } from './schemas/file_pb';
import type { RenderWorkerData, RenderWorkerResult } from './render-worker';

/**
 * TOTAL wall-clock budget for one preview request, measured from the
 * renderToPNG call -- it covers time queued for a render slot plus the render
 * itself. While queued it is enforced by the limiter's per-waiter deadline
 * timer (issue #929); once running, by terminating the worker. Generous -- a
 * routine preview, including the worker compiling its own WASM instance,
 * finishes well under a second -- but bounded, so a pathological or
 * adversarial model costs one failed request instead of pinning a render slot
 * (and, before issue #694, the whole event loop).
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
  /**
   * Run `task` once a concurrency slot is free. `deadline` (epoch ms) bounds
   * the wait for that slot: if it passes first, the returned promise rejects
   * with RenderSlotTimeoutError, the task never runs, and no slot is
   * consumed. Without a deadline the caller waits indefinitely.
   */
  run<T>(task: () => Promise<T>, deadline?: number): Promise<T>;
}

/**
 * Rejection for a run() whose deadline expired before it acquired a slot. A
 * distinct class so renderToPNG can translate queue-wait expiry into its
 * user-facing timeout message without string matching.
 */
export class RenderSlotTimeoutError extends Error {
  constructor() {
    super('deadline expired while waiting for a render slot');
    this.name = 'RenderSlotTimeoutError';
  }
}

/**
 * Minimal FIFO concurrency limiter. A slot is released when the task settles
 * (resolve or reject), so a failed or timed-out render can never leak a slot.
 * A queued waiter's deadline is enforced by its own timer (issue #929): the
 * timer removes the waiter from the queue, so a slot freed later is handed to
 * the next live waiter instead of being burned on a dead one. Timer expiry
 * and grant are mutually exclusive settles -- grant() clears the timer, and
 * a fired timer takes the waiter out of release()'s reach -- so each run()
 * settles exactly once and `active` stays exact. Exported for direct unit
 * testing.
 */
export function createRenderLimiter(maxConcurrent: number): RenderLimiter {
  if (!Number.isInteger(maxConcurrent) || maxConcurrent <= 0) {
    throw new Error(`maxConcurrent must be a positive integer, got ${maxConcurrent}`);
  }

  let active = 0;
  interface Waiter {
    grant: () => void;
  }
  const waiters: Waiter[] = [];

  const acquire = (deadline?: number): Promise<void> => {
    if (deadline !== undefined && Date.now() >= deadline) {
      // Budget already spent; refuse even when a slot is free so a doomed
      // request never consumes capacity.
      return Promise.reject(new RenderSlotTimeoutError());
    }
    if (active < maxConcurrent) {
      active++;
      return Promise.resolve();
    }
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const waiter: Waiter = {
        grant: () => {
          clearTimeout(timer);
          active++;
          resolve();
        },
      };
      if (deadline !== undefined) {
        // Assumes a finite, near-future deadline (the sole caller derives it
        // from Date.now() + RENDER_TIMEOUT_MS). A NaN or >2^31-1ms delay is
        // clamped by Node to ~1ms, i.e. an instant spurious rejection; we
        // document rather than guard because a correct guard needs timer
        // re-arming (a bare clamp reproduces the same early rejection), and
        // for an HTTP-facing queue failing fast on a nonsense deadline beats
        // waiting forever.
        timer = setTimeout(() => {
          const idx = waiters.indexOf(waiter);
          if (idx === -1) {
            // Defensive: grant() clears this timer before dequeueing the
            // waiter, so a fired callback should always find it. Bailing out
            // here matters because splice(-1, 1) below would otherwise evict
            // some other waiter and strand this settled one in the queue.
            return;
          }
          waiters.splice(idx, 1);
          reject(new RenderSlotTimeoutError());
        }, deadline - Date.now());
      }
      waiters.push(waiter);
    });
  };

  const release = (): void => {
    active--;
    const next = waiters.shift();
    if (next) {
      next.grant();
    }
  };

  return {
    async run<T>(task: () => Promise<T>, deadline?: number): Promise<T> {
      await acquire(deadline);
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
  // deadline lapses in the queue must reject then (issue #929), not linger
  // pending until a slot frees.
  const deadline = Date.now() + timeoutMs;
  try {
    return await renderLimiter.run(() => {
      if (Date.now() >= deadline) {
        // The limiter's own deadline timer normally fires first; this guards
        // the sliver between being granted a slot and the task starting, so
        // we never spawn a worker with no remaining budget.
        return Promise.reject(new RenderSlotTimeoutError());
      }
      return runRenderWorker(projectContents, deadline, timeoutMs, workerScript);
    }, deadline);
  } catch (err) {
    if (err instanceof RenderSlotTimeoutError) {
      throw new Error(`preview render timed out after ${timeoutMs}ms waiting for a render slot`);
    }
    throw err;
  }
}
