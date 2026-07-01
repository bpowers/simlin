// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Unit tests for the cross-origin worker trampoline (functional core of the
 * embed fix for issue #688).
 *
 * The browser backend factory spawns the engine's Web Worker from a chunk URL
 * that, in the embeddable web component, is an absolute app.simlin.com URL.
 * On a third-party page `new Worker(<cross-origin url>)` throws a synchronous
 * SecurityError, so the factory routes cross-origin URLs through a
 * same-origin blob: trampoline instead. These tests cover the pure
 * decision/construction functions and the injectable spawn shell; no real
 * Worker is required.
 */

import {
  ENGINE_PUBLIC_PATH_GLOBAL,
  isCrossOrigin,
  planWorkerCreation,
  resolvePublicPathOverride,
  spawnWithTrampoline,
  workerTrampolineSource,
} from '../src/worker-trampoline';

const CHUNK_URL = 'https://app.simlin.com/static/js/async/325.js';
const APP_ORIGIN = 'https://app.simlin.com';
const EMBED_ORIGIN = 'https://example.com';

describe('isCrossOrigin', () => {
  it('is false when the worker URL matches the page origin', () => {
    expect(isCrossOrigin(CHUNK_URL, APP_ORIGIN)).toBe(false);
  });

  it('is true for a different host', () => {
    expect(isCrossOrigin(CHUNK_URL, EMBED_ORIGIN)).toBe(true);
  });

  it('is true for a different scheme on the same host', () => {
    expect(isCrossOrigin(CHUNK_URL, 'http://app.simlin.com')).toBe(true);
  });

  it('is true for a different port on the same host', () => {
    expect(isCrossOrigin(CHUNK_URL, 'https://app.simlin.com:8443')).toBe(true);
  });

  it('is false when the page origin cannot be determined', () => {
    expect(isCrossOrigin(CHUNK_URL, undefined)).toBe(false);
    expect(isCrossOrigin(CHUNK_URL, null)).toBe(false);
    expect(isCrossOrigin(CHUNK_URL, '')).toBe(false);
  });

  it('is false for an opaque ("null") page origin', () => {
    // A blob: trampoline would inherit the same opaque origin, so it cannot
    // help; keep the direct path.
    expect(isCrossOrigin(CHUNK_URL, 'null')).toBe(false);
  });

  it('is false for a relative worker URL (resolves against the page)', () => {
    expect(isCrossOrigin('/static/js/async/325.js', EMBED_ORIGIN)).toBe(false);
    expect(isCrossOrigin('./engine-worker.js', EMBED_ORIGIN)).toBe(false);
  });

  it('is false for non-http(s) worker URLs', () => {
    expect(isCrossOrigin('blob:https://example.com/uuid', EMBED_ORIGIN)).toBe(false);
    expect(isCrossOrigin('data:text/javascript,1', EMBED_ORIGIN)).toBe(false);
  });
});

describe('resolvePublicPathOverride', () => {
  it('passes through undefined and empty publicPath', () => {
    expect(resolvePublicPathOverride(undefined, CHUNK_URL)).toBeUndefined();
    expect(resolvePublicPathOverride('', CHUNK_URL)).toBeUndefined();
  });

  it('normalizes the dot-dot form rspack computes for assetPrefix auto', () => {
    // rspack's runtime computes publicPath by string concatenation, e.g.
    // dirname(script src) + "../../"; normalize so the worker gets a clean
    // absolute base.
    expect(resolvePublicPathOverride('https://app.simlin.com/static/js/../../', CHUNK_URL)).toBe(
      'https://app.simlin.com/',
    );
  });

  it('resolves a relative publicPath against the worker URL origin', () => {
    expect(resolvePublicPathOverride('/', CHUNK_URL)).toBe('https://app.simlin.com/');
  });
});

describe('workerTrampolineSource', () => {
  it('emits a single static import for module workers', () => {
    expect(workerTrampolineSource(CHUNK_URL, 'module')).toBe(`import ${JSON.stringify(CHUNK_URL)};\n`);
  });

  it('emits importScripts for classic workers', () => {
    expect(workerTrampolineSource(CHUNK_URL, 'classic')).toBe(`importScripts(${JSON.stringify(CHUNK_URL)});\n`);
  });

  it('sets the publicPath global before importScripts for classic workers', () => {
    const source = workerTrampolineSource(CHUNK_URL, 'classic', 'https://app.simlin.com/');
    const lines = source.trimEnd().split('\n');
    expect(lines).toHaveLength(2);
    expect(lines[0]).toBe(`self[${JSON.stringify(ENGINE_PUBLIC_PATH_GLOBAL)}] = "https://app.simlin.com/";`);
    expect(lines[1]).toBe(`importScripts(${JSON.stringify(CHUNK_URL)});`);
  });

  it('escapes URLs so they cannot break out of the string literal', () => {
    const hostile = 'https://app.simlin.com/x");importScripts("https://evil.example/y';
    const source = workerTrampolineSource(hostile, 'classic');
    expect(source).toBe(`importScripts(${JSON.stringify(hostile)});\n`);
    expect(source).toContain('\\"');
  });
});

describe('planWorkerCreation', () => {
  it('keeps the direct path for same-origin URLs', () => {
    expect(
      planWorkerCreation({
        workerUrl: CHUNK_URL,
        pageOrigin: APP_ORIGIN,
        workerType: 'classic',
        publicPath: 'https://app.simlin.com/static/js/../../',
      }),
    ).toEqual({ kind: 'direct' });
  });

  it('keeps the direct path when the origin is unknown', () => {
    expect(planWorkerCreation({ workerUrl: CHUNK_URL, pageOrigin: undefined, workerType: 'module' })).toEqual({
      kind: 'direct',
    });
  });

  it('builds an import trampoline for cross-origin module workers', () => {
    const plan = planWorkerCreation({
      workerUrl: CHUNK_URL,
      pageOrigin: EMBED_ORIGIN,
      workerType: 'module',
      publicPath: 'https://app.simlin.com/static/js/../../',
    });
    expect(plan).toEqual({ kind: 'trampoline', source: `import ${JSON.stringify(CHUNK_URL)};\n` });
  });

  it('builds an importScripts trampoline with publicPath override for cross-origin classic workers', () => {
    const plan = planWorkerCreation({
      workerUrl: CHUNK_URL,
      pageOrigin: EMBED_ORIGIN,
      workerType: 'classic',
      publicPath: 'https://app.simlin.com/static/js/../../',
    });
    expect(plan.kind).toBe('trampoline');
    if (plan.kind !== 'trampoline') {
      throw new Error('unreachable');
    }
    expect(plan.source).toContain(`self[${JSON.stringify(ENGINE_PUBLIC_PATH_GLOBAL)}] = "https://app.simlin.com/";`);
    expect(plan.source).toContain(`importScripts(${JSON.stringify(CHUNK_URL)});`);
  });

  it('omits the publicPath override when none is available', () => {
    const plan = planWorkerCreation({
      workerUrl: CHUNK_URL,
      pageOrigin: EMBED_ORIGIN,
      workerType: 'classic',
    });
    expect(plan).toEqual({
      kind: 'trampoline',
      source: `importScripts(${JSON.stringify(CHUNK_URL)});\n`,
    });
  });
});

describe('spawnWithTrampoline', () => {
  // Mimics the DOM Worker constructor closely enough for the shell: records
  // its arguments so tests can assert exactly what was constructed.
  class FakeWorker {
    readonly url: string | URL;
    readonly options: WorkerOptions | undefined;
    constructor(url: string | URL, options?: WorkerOptions) {
      this.url = url;
      this.options = options;
    }
  }

  interface FakeUrlFactory {
    createObjectURL: jest.Mock<string, [Blob]>;
    revokeObjectURL: jest.Mock<void, [string]>;
    lastBlob: () => Blob;
  }

  function makeUrlFactory(): FakeUrlFactory {
    let blob: Blob | null = null;
    const createObjectURL = jest.fn((b: Blob) => {
      blob = b;
      return 'blob:https://example.com/fake-uuid';
    });
    const revokeObjectURL = jest.fn();
    return {
      createObjectURL,
      revokeObjectURL,
      lastBlob: () => {
        if (blob === null) {
          throw new Error('no blob was created');
        }
        return blob;
      },
    };
  }

  function makeScope(): { Worker?: unknown } {
    return { Worker: FakeWorker };
  }

  // Emulates the bundler-emitted expression: it constructs whatever the
  // (possibly swapped) global `Worker` currently refers to.
  function bundlerSpawn(scope: { Worker?: unknown }, url: string, type: string | undefined): () => Worker {
    return () => {
      const ctor = scope.Worker as new (url: URL, options?: WorkerOptions) => Worker;
      return new ctor(new URL(url), { type: type as WorkerType | undefined });
    };
  }
  type WorkerType = 'classic' | 'module';

  it('passes same-origin workers through to the native constructor untouched', () => {
    const scope = makeScope();
    const urlFactory = makeUrlFactory();
    const spawned = spawnWithTrampoline(
      scope,
      bundlerSpawn(scope, CHUNK_URL, undefined),
      { pageOrigin: APP_ORIGIN, publicPath: 'https://app.simlin.com/static/js/../../' },
      urlFactory,
    );
    expect(spawned.blobUrl).toBeNull();
    const worker = spawned.worker as unknown as FakeWorker;
    expect(worker).toBeInstanceOf(FakeWorker);
    expect(String(worker.url)).toBe(CHUNK_URL);
    expect(urlFactory.createObjectURL).not.toHaveBeenCalled();
    expect(scope.Worker).toBe(FakeWorker);
  });

  it('routes cross-origin classic workers through a blob trampoline', async () => {
    const scope = makeScope();
    const urlFactory = makeUrlFactory();
    const spawned = spawnWithTrampoline(
      scope,
      bundlerSpawn(scope, CHUNK_URL, undefined),
      { pageOrigin: EMBED_ORIGIN, publicPath: 'https://app.simlin.com/static/js/../../' },
      urlFactory,
    );
    expect(spawned.blobUrl).toBe('blob:https://example.com/fake-uuid');
    const worker = spawned.worker as unknown as FakeWorker;
    expect(worker).toBeInstanceOf(FakeWorker);
    expect(String(worker.url)).toBe('blob:https://example.com/fake-uuid');
    const source = await urlFactory.lastBlob().text();
    expect(source).toContain(`self[${JSON.stringify(ENGINE_PUBLIC_PATH_GLOBAL)}] = "https://app.simlin.com/";`);
    expect(source).toContain(`importScripts(${JSON.stringify(CHUNK_URL)});`);
    expect(scope.Worker).toBe(FakeWorker);
  });

  it('routes cross-origin module workers through an import trampoline', async () => {
    const scope = makeScope();
    const urlFactory = makeUrlFactory();
    const spawned = spawnWithTrampoline(
      scope,
      bundlerSpawn(scope, CHUNK_URL, 'module'),
      { pageOrigin: EMBED_ORIGIN, publicPath: undefined },
      urlFactory,
    );
    expect(spawned.blobUrl).toBe('blob:https://example.com/fake-uuid');
    const source = await urlFactory.lastBlob().text();
    expect(source).toBe(`import ${JSON.stringify(CHUNK_URL)};\n`);
    expect(scope.Worker).toBe(FakeWorker);
  });

  it('restores the global and revokes the blob URL when construction fails', () => {
    class ThrowingWorker {
      constructor() {
        throw new Error('worker construction failed');
      }
    }
    const scope: { Worker?: unknown } = { Worker: ThrowingWorker };
    const urlFactory = makeUrlFactory();
    expect(() =>
      spawnWithTrampoline(
        scope,
        bundlerSpawn(scope, CHUNK_URL, undefined),
        { pageOrigin: EMBED_ORIGIN, publicPath: undefined },
        urlFactory,
      ),
    ).toThrow('worker construction failed');
    expect(urlFactory.revokeObjectURL).toHaveBeenCalledWith('blob:https://example.com/fake-uuid');
    expect(scope.Worker).toBe(ThrowingWorker);
  });

  it('restores the global when spawn itself throws', () => {
    const scope = makeScope();
    const urlFactory = makeUrlFactory();
    expect(() =>
      spawnWithTrampoline(
        scope,
        () => {
          throw new Error('bundler pattern not transformed');
        },
        { pageOrigin: APP_ORIGIN, publicPath: undefined },
        urlFactory,
      ),
    ).toThrow('bundler pattern not transformed');
    expect(scope.Worker).toBe(FakeWorker);
  });

  it('falls back to plain spawn when the environment has no Worker', () => {
    const scope: { Worker?: unknown } = {};
    const sentinel = {} as Worker;
    const spawned = spawnWithTrampoline(scope, () => sentinel, { pageOrigin: EMBED_ORIGIN }, makeUrlFactory());
    expect(spawned).toEqual({ worker: sentinel, blobUrl: null });
  });

  it('returns the spawned worker as-is when spawn bypasses the global constructor', () => {
    const scope = makeScope();
    const sentinel = {} as Worker;
    const spawned = spawnWithTrampoline(scope, () => sentinel, { pageOrigin: EMBED_ORIGIN }, makeUrlFactory());
    expect(spawned).toEqual({ worker: sentinel, blobUrl: null });
    expect(scope.Worker).toBe(FakeWorker);
  });
});
