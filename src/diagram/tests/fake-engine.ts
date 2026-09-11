// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A reusable in-memory fake of the engine `Project`/`Model`/`Run` surface the
// ProjectController depends on (the `EngineApi` interface). It records applied
// patches, dispose calls and the method-call sequence, and lets tests script
// serialized outputs, errors, simulatability, sim-run results, patch failures,
// and patch latency (a gate each applyPatch awaits) -- so the controller's
// async coordination can be exercised without spinning up WASM or jsdom.

import type { JsonProjectPatch, ErrorDetail } from '@simlin/engine';
import type { EngineApi, EngineModelApi, EngineRunApi, ProjectControllerConfig } from '../project-controller';

// The engine prefixes injected stdlib model names (e.g. 'stdlib\u{205A}delay1').
// We mirror that convention so isStdlibModel() recognizes the fake's stdlib
// model and tests can assert on its presence/absence.
export const FAKE_STDLIB_MODEL_NAME = 'stdlib\u{205A}delay1';

/**
 * A minimal but valid native-format project JSON that projectFromJson accepts.
 *
 * When `includeStdlib` is true, a recognizable stdlib model is appended,
 * mirroring the real engine's `serializeJson(format, includeStdlib=true)`
 * behavior: the display path injects referenced stdlib definitions, the save
 * path (includeStdlib=false) omits them so they are never persisted.
 */
export function validProjectJson(
  overrides: {
    name?: string;
    extraModels?: ReadonlyArray<Record<string, unknown>>;
    mainViewElements?: ReadonlyArray<Record<string, unknown>>;
    auxiliaries?: ReadonlyArray<Record<string, unknown>>;
    includeStdlib?: boolean;
  } = {},
): string {
  const name = overrides.name ?? 'test';
  const models: Array<Record<string, unknown>> = [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: overrides.auxiliaries ?? [],
      views: [{ elements: overrides.mainViewElements ?? [] }],
    },
    ...(overrides.extraModels ?? []),
  ];
  if (overrides.includeStdlib) {
    models.push({
      name: FAKE_STDLIB_MODEL_NAME,
      stocks: [],
      flows: [],
      auxiliaries: [{ name: 'output', equation: '1' }],
      views: [{ elements: [] }],
    });
  }
  return JSON.stringify({
    name,
    simSpecs: { startTime: 0, endTime: 10, dt: '1' },
    models,
  });
}

export interface FakeEngineOptions {
  // The JSON returned by serializeJson(includeStdlib). May change between calls
  // by passing a function, which receives the includeStdlib flag so it can
  // mirror the engine's display-vs-save distinction. Defaults to
  // validProjectJson({ includeStdlib }), so the default fake already injects a
  // stdlib model on the display path and omits it on the save path.
  json?: string | ((includeStdlib: boolean) => string);
  // The protobuf returned by serializeProtobuf(). Defaults to a 1-byte marker
  // that increments on each call so every committed rebuild sees a new snapshot.
  protobuf?: Uint8Array | (() => Uint8Array);
  errors?: ErrorDetail[] | (() => ErrorDetail[]);
  simulatable?: boolean | (() => boolean);
  // Scripts the sim run. When it throws, the LTM fallback retries; supply a
  // function that throws on the first call to exercise that path.
  run?: (overrides: Record<string, number>, options: { analyzeLtm?: boolean }) => EngineRunApi;
  // Forces applyPatch to reject: always (true / an Error), or per patch (a
  // function of the patch and its 0-based call index returning the error to
  // throw, or undefined to accept).
  applyPatchThrows?: boolean | Error | ((patch: JsonProjectPatch, index: number) => Error | undefined);
  // Awaited by every applyPatch before it applies or throws: the latency a
  // worker round trip adds, controllable per call.
  applyPatchGate?: (patch: JsonProjectPatch, index: number) => Promise<void>;
  // Called when a patch is accepted, after the gate: lets a test move the
  // scripted serialization to the patched state.
  onApplyPatch?: (patch: JsonProjectPatch) => void;
  // Scripts getModel().getIncomingLinks(varName) for the connector-sync check:
  // a per-ident dependency map, or a function. Unlisted idents yield []. When a
  // function throws, that variable is dropped from the connector check.
  incomingLinks?: Record<string, readonly string[]> | ((varName: string) => readonly string[]);
  // Forces getModel() to reject (the connector-sync skip path).
  getModelThrows?: boolean | Error;
}

export interface FakeEngine extends EngineApi {
  readonly appliedPatches: ReadonlyArray<JsonProjectPatch>;
  readonly serializeProtobufCalls: number;
  readonly runCalls: ReadonlyArray<{ overrides: Record<string, number>; analyzeLtm: boolean | undefined }>;
  // Every engine method entry, in order ('applyPatch', 'serializeJson:stdlib',
  // 'serializeJson:save', 'serializeProtobuf', 'getErrors', ...).
  readonly calls: ReadonlyArray<string>;
  // The most engine calls observed in flight at once.
  readonly maxConcurrentCalls: number;
  disposeCount: number;
}

function makeRun(varNames: readonly string[], series: ReadonlyMap<string, Float64Array>): EngineRunApi {
  return {
    varNames,
    getSeries(name: string): Float64Array {
      return series.get(name) ?? new Float64Array(0);
    },
  };
}

/** A simple Run with a `time` series and any named variables. */
export function fakeRun(seriesByName: Record<string, number[]>): EngineRunApi {
  const series = new Map<string, Float64Array>();
  for (const [name, values] of Object.entries(seriesByName)) {
    series.set(name, new Float64Array(values));
  }
  if (!series.has('time')) {
    series.set('time', new Float64Array([0, 1, 2]));
  }
  return makeRun(Object.keys(seriesByName), series);
}

export function makeFakeEngine(options: FakeEngineOptions = {}): FakeEngine {
  const appliedPatches: JsonProjectPatch[] = [];
  const runCalls: Array<{ overrides: Record<string, number>; analyzeLtm: boolean | undefined }> = [];
  const calls: string[] = [];
  let serializeProtobufCalls = 0;
  let protobufCounter = 100;
  let patchIndex = 0;
  let inFlight = 0;
  let maxConcurrentCalls = 0;

  // Every method counts itself in flight across its own awaits, so a test can
  // assert the controller never overlaps two engine calls.
  const tracked = async <T>(name: string, body: () => Promise<T>): Promise<T> => {
    calls.push(name);
    inFlight++;
    maxConcurrentCalls = Math.max(maxConcurrentCalls, inFlight);
    try {
      // One macrotask of latency, so overlapping callers would actually overlap.
      await new Promise((resolve) => setTimeout(resolve, 0));
      return await body();
    } finally {
      inFlight--;
    }
  };

  const resolveJson = (includeStdlib: boolean): string => {
    if (typeof options.json === 'function') {
      return options.json(includeStdlib);
    }
    return options.json ?? validProjectJson({ includeStdlib });
  };
  const resolveProtobuf = (): Uint8Array => {
    if (typeof options.protobuf === 'function') {
      return options.protobuf();
    }
    if (options.protobuf) {
      return options.protobuf;
    }
    return new Uint8Array([protobufCounter++]);
  };
  const resolveErrors = (): ErrorDetail[] =>
    typeof options.errors === 'function' ? options.errors() : (options.errors ?? []);
  const resolveSimulatable = (): boolean =>
    typeof options.simulatable === 'function' ? options.simulatable() : (options.simulatable ?? true);

  const resolveIncomingLinks = (varName: string): readonly string[] => {
    if (typeof options.incomingLinks === 'function') {
      return options.incomingLinks(varName);
    }
    return options.incomingLinks?.[varName] ?? [];
  };

  const model: EngineModelApi = {
    run(overrides: Record<string, number> = {}, runOptions: { analyzeLtm?: boolean } = {}): Promise<EngineRunApi> {
      return tracked('run', async () => {
        runCalls.push({ overrides, analyzeLtm: runOptions.analyzeLtm });
        if (options.run) {
          return options.run(overrides, runOptions);
        }
        return fakeRun({ time: [0, 1, 2], output: [1, 2, 3] });
      });
    },
    getIncomingLinks(varName: string): Promise<readonly string[]> {
      return tracked('getIncomingLinks', async () => resolveIncomingLinks(varName));
    },
  };

  const engine: FakeEngine = {
    appliedPatches,
    runCalls,
    calls,
    disposeCount: 0,
    get serializeProtobufCalls() {
      return serializeProtobufCalls;
    },
    get maxConcurrentCalls() {
      return maxConcurrentCalls;
    },
    applyPatch(patch: JsonProjectPatch): Promise<ErrorDetail[]> {
      return tracked('applyPatch', async () => {
        const index = patchIndex++;
        await options.applyPatchGate?.(patch, index);
        const throws = options.applyPatchThrows;
        const error =
          typeof throws === 'function'
            ? throws(patch, index)
            : throws
              ? throws instanceof Error
                ? throws
                : Object.assign(new Error('patch rejected'), { code: 1, details: [] })
              : undefined;
        if (error !== undefined) {
          throw error;
        }
        appliedPatches.push(patch);
        options.onApplyPatch?.(patch);
        return [];
      });
    },
    serializeProtobuf(): Promise<Uint8Array> {
      return tracked('serializeProtobuf', async () => {
        serializeProtobufCalls++;
        return resolveProtobuf();
      });
    },
    serializeJson(_format?: unknown, includeStdlib?: boolean): Promise<string> {
      return tracked(includeStdlib ? 'serializeJson:stdlib' : 'serializeJson:save', async () =>
        resolveJson(!!includeStdlib),
      );
    },
    getErrors(): Promise<ErrorDetail[]> {
      return tracked('getErrors', async () => resolveErrors());
    },
    isSimulatable(): Promise<boolean> {
      return tracked('isSimulatable', async () => resolveSimulatable());
    },
    mainModel(): Promise<EngineModelApi> {
      return tracked('mainModel', async () => model);
    },
    getModel(_modelName: string | null): Promise<EngineModelApi> {
      return tracked('getModel', async () => {
        if (options.getModelThrows) {
          throw options.getModelThrows instanceof Error ? options.getModelThrows : new Error('getModel failed');
        }
        return model;
      });
    },
    async dispose(): Promise<void> {
      calls.push('dispose');
      engine.disposeCount++;
    },
  };

  return engine;
}

/** A gate a test opens by hand: `gate.wait` is what applyPatchGate awaits. */
export function makeGate(): { wait: () => Promise<void>; open: () => void } {
  let resolve: () => void = () => {};
  const promise = new Promise<void>((r) => {
    resolve = r;
  });
  return { wait: () => promise, open: () => resolve() };
}

/**
 * Build a ProjectControllerConfig wired to fake engines. `openProtobuf` /
 * `openJson` resolve to the engines yielded by `nextEngine` (a queue of
 * engines, or a single engine reused for every open). `onError` records into
 * the returned `errors` array; `save` records into `saves` and returns the
 * scripted version.
 */
export function makeControllerConfig(opts: {
  format?: 'protobuf' | 'json';
  initialVersion?: number;
  initialData?: Uint8Array | string;
  engine?: FakeEngine;
  engines?: FakeEngine[];
  openThrows?: boolean | Error;
  now?: () => number;
  save?: (
    project: { format: 'protobuf'; data: Uint8Array } | { format: 'json'; data: string },
    currVersion: number,
  ) => Promise<number | undefined>;
}): {
  config: ProjectControllerConfig;
  errors: Error[];
  saves: Array<{ project: { format: string; data: unknown }; currVersion: number }>;
  openedEngines: FakeEngine[];
  openedWith: Uint8Array[];
} {
  const format = opts.format ?? 'protobuf';
  const errors: Error[] = [];
  const saves: Array<{ project: { format: string; data: unknown }; currVersion: number }> = [];
  const openedEngines: FakeEngine[] = [];
  const openedWith: Uint8Array[] = [];

  const queue: FakeEngine[] = opts.engines ? [...opts.engines] : [];
  const singleEngine = opts.engine;

  const nextEngine = async (): Promise<EngineApi> => {
    if (opts.openThrows) {
      throw opts.openThrows instanceof Error ? opts.openThrows : new Error('open failed');
    }
    const engine = queue.length > 0 ? defined(queue.shift()) : defined(singleEngine ?? makeFakeEngine());
    openedEngines.push(engine);
    return engine;
  };

  const config: ProjectControllerConfig = {
    initialProjectVersion: opts.initialVersion ?? 1,
    input:
      format === 'protobuf'
        ? { format: 'protobuf', data: (opts.initialData as Uint8Array | undefined) ?? new Uint8Array([1]) }
        : { format: 'json', data: (opts.initialData as string | undefined) ?? validProjectJson() },
    openProtobuf: (data) => {
      openedWith.push(data);
      return nextEngine();
    },
    openJson: () => nextEngine(),
    save:
      opts.save ??
      (async (project, currVersion) => {
        saves.push({ project, currVersion });
        return currVersion + 1;
      }),
    onError: (err) => {
      errors.push(err);
    },
    now: opts.now,
  };

  return { config, errors, saves, openedEngines, openedWith };
}

function defined<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error('expected a value but got undefined (fake-engine queue exhausted?)');
  }
  return value;
}
