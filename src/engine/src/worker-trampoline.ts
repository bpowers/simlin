// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Cross-origin worker trampoline (issue #688).
 *
 * The embeddable web component is hotlinked from third-party pages as
 * `<script src="https://app.simlin.com/static/js/sd-component.js">`, and its
 * engine worker chunk resolves to an absolute app.simlin.com URL. The Worker
 * constructor enforces the same-origin policy regardless of CORS, so on an
 * embedding page `new Worker(<app.simlin.com url>)` throws a synchronous
 * SecurityError and the engine can never initialize.
 *
 * The workaround is the standard blob trampoline: create the worker from a
 * same-origin `blob:` URL whose only job is to load the real cross-origin
 * chunk. Two wrinkles, both verified against the emitted rspack 1.7 output:
 *
 * 1. rspack downgrades `{ type: 'module' }` to a classic worker for
 *    non-module (UMD) builds, so the trampoline body must be
 *    `importScripts(...)` there, while module-worker bundlers (vite) need a
 *    static `import` instead. `importScripts` of a cross-origin classic
 *    script is permitted without CORS; the module `import` is a CORS fetch
 *    and relies on the ACAO header app.yaml serves for /static.
 *
 * 2. Inside a classic worker chunk, rspack's `publicPath: 'auto'` runtime
 *    derives the asset base from `self.location`, which for a trampolined
 *    worker is the blob URL -- i.e. the *embedding* page's origin -- so the
 *    engine's WASM fetch would target the wrong host. The trampoline
 *    therefore smuggles the correct asset root through a well-known global
 *    (ENGINE_PUBLIC_PATH_GLOBAL) that engine-worker.ts applies to
 *    `__webpack_public_path__` before anything triggers the WASM load.
 *    Module-format chunks resolve assets via `import.meta.url` (which a
 *    static import preserves as the real chunk URL) and need no override.
 *
 * This module is the functional core: pure decision/construction functions
 * plus an injectable spawn shell, all unit-testable without a real Worker.
 */

export type WorkerType = 'module' | 'classic';

export interface DirectPlan {
  readonly kind: 'direct';
}

export interface TrampolinePlan {
  readonly kind: 'trampoline';
  readonly source: string;
}

export type WorkerCreationPlan = DirectPlan | TrampolinePlan;

/**
 * Global property name the classic-worker trampoline uses to hand the
 * bundler's asset root to engine-worker.ts. Shared as a constant so both
 * sides of the handoff cannot drift apart.
 */
export const ENGINE_PUBLIC_PATH_GLOBAL = '__simlin_engine_public_path__';

/**
 * Whether creating a Worker for `workerUrl` from a page at `pageOrigin`
 * would violate the Worker constructor's same-origin requirement.
 *
 * Conservative by design: whenever the answer cannot be determined the
 * result is false, preserving the direct (status quo) construction path.
 */
export function isCrossOrigin(workerUrl: string, pageOrigin: string | null | undefined): boolean {
  // 'null' is the serialization of an opaque origin (sandboxed iframe,
  // file:, ...). A blob: trampoline would inherit the same opaque origin
  // and cannot help, so keep the direct path there too.
  if (!pageOrigin || pageOrigin === 'null') {
    return false;
  }
  let parsed: URL;
  try {
    parsed = new URL(workerUrl);
  } catch {
    // Relative URLs resolve against the page itself, i.e. same-origin.
    return false;
  }
  // blob:/data: worker URLs are not subject to the cross-origin restriction
  // the trampoline works around; only http(s) chunk URLs are candidates.
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    return false;
  }
  return parsed.origin !== pageOrigin;
}

/**
 * Normalize the bundler-reported publicPath into an absolute URL string the
 * worker can prepend to asset paths. rspack's 'auto' runtime builds the
 * value by string concatenation (e.g. "https://host/static/js/" + "../../"),
 * so normalization through the URL parser keeps the handoff clean; a
 * relative publicPath resolves against the worker chunk's own origin, which
 * is where the assets actually live.
 */
export function resolvePublicPathOverride(publicPath: string | undefined, workerUrl: string): string | undefined {
  if (publicPath === undefined || publicPath === '') {
    return undefined;
  }
  try {
    return new URL(publicPath, workerUrl).href;
  } catch {
    return undefined;
  }
}

/**
 * Build the source of the same-origin trampoline script.
 *
 * For module workers a single static `import` suffices: the imported chunk
 * keeps its own absolute URL as `import.meta.url`, so every relative
 * resolution inside it (further chunks, WASM) still targets the origin that
 * served it. For classic workers the equivalent is `importScripts`, plus the
 * publicPath handoff described in the module docs above.
 */
export function workerTrampolineSource(absUrl: string, workerType: WorkerType, publicPathOverride?: string): string {
  // JSON.stringify guarantees the URL cannot break out of the string
  // literal, whatever characters it contains.
  const urlLiteral = JSON.stringify(absUrl);
  if (workerType === 'module') {
    return `import ${urlLiteral};\n`;
  }
  const lines: string[] = [];
  if (publicPathOverride !== undefined) {
    lines.push(`self[${JSON.stringify(ENGINE_PUBLIC_PATH_GLOBAL)}] = ${JSON.stringify(publicPathOverride)};`);
  }
  lines.push(`importScripts(${urlLiteral});`);
  return `${lines.join('\n')}\n`;
}

export interface PlanWorkerCreationArgs {
  readonly workerUrl: string;
  readonly pageOrigin: string | null | undefined;
  readonly workerType: WorkerType;
  readonly publicPath?: string;
}

/**
 * Decide how to construct the engine worker: directly (same-origin or
 * undeterminable, the status quo path) or via a blob trampoline
 * (cross-origin embed).
 */
export function planWorkerCreation(args: PlanWorkerCreationArgs): WorkerCreationPlan {
  const { workerUrl, pageOrigin, workerType, publicPath } = args;
  if (!isCrossOrigin(workerUrl, pageOrigin)) {
    return { kind: 'direct' };
  }
  const override = workerType === 'classic' ? resolvePublicPathOverride(publicPath, workerUrl) : undefined;
  return { kind: 'trampoline', source: workerTrampolineSource(workerUrl, workerType, override) };
}

type WorkerCtor = new (url: string | URL, options?: WorkerOptions) => Worker;

export interface WorkerUrlFactory {
  createObjectURL(blob: Blob): string;
  revokeObjectURL(url: string): void;
}

export interface SpawnedWorker {
  readonly worker: Worker;
  /**
   * The blob: URL backing a trampolined worker, or null for the direct
   * path. The caller owns revocation: revoke once the worker has provably
   * loaded (first message) or failed (error event), not before -- the
   * browser fetches the blob asynchronously after construction.
   */
  readonly blobUrl: string | null;
}

export interface SpawnEnvironment {
  readonly pageOrigin: string | null | undefined;
  readonly publicPath?: string;
}

/**
 * Run `spawn` -- the bundler-emitted `new Worker(new URL(...), ...)`
 * expression -- with the global Worker constructor transiently swapped for
 * an interceptor that applies planWorkerCreation to the resolved chunk URL.
 *
 * Why interception instead of computing the URL up front: bundlers (rspack,
 * webpack, vite) only detect and bundle a worker when the `new URL(...)`
 * sits literally inside the `new Worker(...)` call, and they rewrite that
 * whole expression to runtime-internal publicPath/chunk-filename lookups.
 * The resolved URL is therefore only observable at the constructor call
 * itself. The swap is synchronous and restored in `finally`; JS is
 * single-threaded, so nothing else can observe it.
 *
 * On the direct path the interceptor forwards the exact URL object and
 * options it received, so same-origin behavior is unchanged.
 */
export function spawnWithTrampoline(
  globalScope: { Worker?: unknown },
  spawn: () => Worker,
  env: SpawnEnvironment,
  urlFactory: WorkerUrlFactory = URL,
): SpawnedWorker {
  const NativeWorker = globalScope.Worker as WorkerCtor | undefined;
  if (typeof NativeWorker !== 'function') {
    // No Worker constructor here (SSR, exotic runtimes): let the bundled
    // expression behave exactly as it would have without this shim.
    return { worker: spawn(), blobUrl: null };
  }

  // Holder object rather than a bare local: assignments happen inside the
  // interceptor closure, and TypeScript's control-flow analysis would
  // otherwise narrow a bare local to its initial null at the read below.
  const captured: { spawned: SpawnedWorker | null } = { spawned: null };
  // A function expression (not an arrow) so the emitted `new Worker(...)`
  // call can construct it; returning an object from a constructor makes
  // that object the result of `new`.
  const InterceptingWorker = function (url: string | URL, options?: WorkerOptions): Worker {
    const plan = planWorkerCreation({
      workerUrl: String(url),
      pageOrigin: env.pageOrigin,
      workerType: options?.type === 'module' ? 'module' : 'classic',
      publicPath: env.publicPath,
    });
    if (plan.kind === 'direct') {
      const worker = new NativeWorker(url, options);
      captured.spawned = { worker, blobUrl: null };
      return worker;
    }
    const blobUrl = urlFactory.createObjectURL(new Blob([plan.source], { type: 'text/javascript' }));
    try {
      const worker = new NativeWorker(blobUrl, options);
      captured.spawned = { worker, blobUrl };
      return worker;
    } catch (err) {
      urlFactory.revokeObjectURL(blobUrl);
      throw err;
    }
  } as unknown as WorkerCtor;

  globalScope.Worker = InterceptingWorker;
  let worker: Worker;
  try {
    worker = spawn();
  } finally {
    globalScope.Worker = NativeWorker;
  }
  // If the bundled code somehow constructed the worker without going through
  // the global constructor (or wrapped the one it got back), fall back to
  // reporting the caller-visible handle -- but still surface any blob URL we
  // created, so the caller's revoke-on-first-message/error logic reclaims it
  // instead of leaking a blob nobody can revoke. Revoking eagerly here would
  // be wrong: the underlying worker may still be fetching the blob.
  const intercepted = captured.spawned;
  if (intercepted !== null && intercepted.worker === worker) {
    return intercepted;
  }
  return { worker, blobUrl: intercepted !== null ? intercepted.blobUrl : null };
}
