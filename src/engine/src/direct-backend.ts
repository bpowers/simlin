// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * DirectBackend: calls WASM functions directly (no Worker).
 *
 * Used by Node.js and as the internal implementation for WorkerServer.
 * Maps opaque integer handles to WASM pointers.
 */

import { EngineBackend, ProjectHandle, ModelHandle, SimHandle, SimEngine } from './backend';
import {
  simlin_project_open_protobuf,
  simlin_project_open_json,
  simlin_project_unref,
  simlin_project_get_model_count,
  simlin_project_get_model_names,
  simlin_project_get_model,
  simlin_project_serialize_protobuf,
  simlin_project_serialize_json,
  simlin_project_is_simulatable,
  simlin_project_get_errors,
  simlin_project_apply_patch,
} from './internal/project';
import {
  simlin_project_open_xmile,
  simlin_project_open_vensim,
  simlin_project_serialize_xmile,
  simlin_project_render_svg,
  simlin_project_render_png,
} from './internal/import-export';
import {
  simlin_model_unref,
  simlin_model_get_name,
  simlin_model_get_incoming_links,
  simlin_model_get_links as simlin_model_get_links_fn,
  simlin_model_get_latex_equation,
  simlin_model_get_var_names,
  simlin_model_get_var_json,
  simlin_model_get_sim_specs_json,
} from './internal/model';
import {
  simlin_sim_new,
  simlin_sim_unref,
  simlin_sim_run_to,
  simlin_sim_run_to_end,
  simlin_sim_reset,
  simlin_sim_get_stepcount,
  simlin_sim_get_value,
  simlin_sim_set_value,
  simlin_sim_get_series,
  simlin_sim_get_var_names as simlin_sim_get_var_names_fn,
} from './internal/sim';
import {
  simlin_analyze_get_loops,
  simlin_analyze_get_links,
  readLoops,
  readLinks,
  simlin_free_loops,
  simlin_free_links,
} from './internal/analysis';
import { readAllErrorDetails, simlin_error_free } from './internal/error';
import { simlin_model_compile_to_wasm, parseWasmLayout, WasmLayout, WasmBlobExports } from './internal/wasmgen';
import {
  SimlinProjectPtr,
  SimlinModelPtr,
  SimlinSimPtr,
  SimlinJsonFormat,
  SimlinLinkPolarity,
  ErrorDetail,
  Link as LowLevelLink,
} from './internal/types';
import { Loop, Link, LoopPolarity, LinkPolarity } from './types';
import { JsonProjectPatch } from './json-types';
import {
  configureWasm as wasmConfigureWasm,
  ensureInitialized,
  isInitialized,
  reset as wasmReset,
  WasmConfig,
  WasmSourceProvider,
} from '@simlin/engine/internal/wasm';

function convertLinkPolarity(raw: SimlinLinkPolarity): LinkPolarity {
  switch (raw) {
    case SimlinLinkPolarity.Positive:
      return LinkPolarity.Positive;
    case SimlinLinkPolarity.Negative:
      return LinkPolarity.Negative;
    case SimlinLinkPolarity.Unknown:
      return LinkPolarity.Unknown;
    default:
      throw new Error(`Invalid link polarity value: ${raw}`);
  }
}

function convertLinks(linksPtr: number): Link[] {
  if (linksPtr === 0) {
    return [];
  }
  let links: Link[] = [];
  try {
    const rawLinks = readLinks(linksPtr);
    links = rawLinks.map((link: LowLevelLink) => ({
      from: link.from,
      to: link.to,
      polarity: convertLinkPolarity(link.polarity),
      score: link.score || undefined,
    }));
  } finally {
    simlin_free_links(linksPtr);
  }
  return links;
}

type HandleKind = 'project' | 'model' | 'sim';

interface HandleEntry {
  kind: HandleKind;
  ptr: number;
  disposed: boolean;
  // For model/sim handles, track which project they belong to
  projectHandle?: number;
  // For sim handles: which execution backend this sim runs on. A 'wasm' entry
  // has no native sim pointer (ptr is 0); it owns a WebAssembly.Instance and
  // drives the blob's exports directly. Absent/'vm' means the bytecode VM.
  engine?: SimEngine;
  // Wasm-engine state (set only when engine === 'wasm'). The instance is owned
  // here so it is created exactly once and GC'd when the entry is dropped.
  wasmInstance?: WebAssembly.Instance;
  wasmLayout?: WasmLayout;
  wasmExports?: WasmBlobExports;
  // The model's stop time, captured at creation so simRunToEnd can drive the
  // blob's resumable run_to(stop) (mirroring the VM's run_to(specs.stop)).
  wasmStopTime?: number;
}

/** Optional fields carried onto a freshly-allocated handle entry. */
interface HandleExtra {
  projectHandle?: number;
  engine?: SimEngine;
  wasmInstance?: WebAssembly.Instance;
  wasmLayout?: WasmLayout;
  wasmExports?: WasmBlobExports;
  wasmStopTime?: number;
}

export class DirectBackend implements EngineBackend {
  private _nextHandle = 1;
  private _handles = new Map<number, HandleEntry>();
  private _projectChildren = new Map<number, Set<number>>();

  private allocHandle(kind: HandleKind, ptr: number, extra?: HandleExtra): number {
    const handle = this._nextHandle++;
    this._handles.set(handle, {
      kind,
      ptr,
      disposed: false,
      projectHandle: extra?.projectHandle,
      engine: extra?.engine,
      wasmInstance: extra?.wasmInstance,
      wasmLayout: extra?.wasmLayout,
      wasmExports: extra?.wasmExports,
      wasmStopTime: extra?.wasmStopTime,
    });
    if (kind === 'project') {
      this._projectChildren.set(handle, new Set());
    } else if (extra?.projectHandle !== undefined) {
      this._projectChildren.get(extra.projectHandle)?.add(handle);
    }
    return handle;
  }

  private getEntry(handle: number, expectedKind: HandleKind): HandleEntry {
    const entry = this._handles.get(handle);
    if (!entry) {
      throw new Error(`Handle ${handle} does not exist`);
    }
    if (entry.disposed) {
      throw new Error(`Handle ${handle} has been disposed`);
    }
    if (entry.kind !== expectedKind) {
      throw new Error(`Handle ${handle} is a ${entry.kind}, expected ${expectedKind}`);
    }
    return entry;
  }

  private getProjectPtr(handle: ProjectHandle): SimlinProjectPtr {
    return this.getEntry(handle as number, 'project').ptr;
  }

  private getModelPtr(handle: ModelHandle): SimlinModelPtr {
    return this.getEntry(handle as number, 'model').ptr;
  }

  private getSimPtr(handle: SimHandle): SimlinSimPtr {
    return this.getEntry(handle as number, 'sim').ptr;
  }

  // Lifecycle

  async init(wasmSource?: WasmSourceProvider): Promise<void> {
    await ensureInitialized(wasmSource);
  }

  isInitialized(): boolean {
    return isInitialized();
  }

  reset(): void {
    // Dispose all active handles -- don't unref because wasmReset() invalidates all pointers
    for (const [, entry] of this._handles) {
      entry.disposed = true;
    }
    this._handles.clear();
    this._projectChildren.clear();
    this._nextHandle = 1;
    wasmReset();
  }

  configureWasm(config: WasmConfig): void {
    wasmConfigureWasm(config);
  }

  // Project open operations

  projectOpenXmile(data: Uint8Array): ProjectHandle {
    const ptr = simlin_project_open_xmile(data);
    return this.allocHandle('project', ptr) as ProjectHandle;
  }

  projectOpenProtobuf(data: Uint8Array): ProjectHandle {
    const ptr = simlin_project_open_protobuf(data);
    return this.allocHandle('project', ptr) as ProjectHandle;
  }

  projectOpenJson(data: Uint8Array, format: SimlinJsonFormat): ProjectHandle {
    const ptr = simlin_project_open_json(data, format);
    return this.allocHandle('project', ptr) as ProjectHandle;
  }

  projectOpenVensim(data: Uint8Array): ProjectHandle {
    const ptr = simlin_project_open_vensim(data);
    return this.allocHandle('project', ptr) as ProjectHandle;
  }

  // Project operations

  projectDispose(handle: ProjectHandle): void {
    const entry = this._handles.get(handle as number);
    if (!entry || entry.disposed) {
      return; // idempotent
    }
    // Dispose all child handles (models and sims) belonging to this project
    const children = this._projectChildren.get(handle as number);
    if (children) {
      for (const childHandle of children) {
        const childEntry = this._handles.get(childHandle);
        if (childEntry && !childEntry.disposed) {
          childEntry.disposed = true;
          if (childEntry.kind === 'sim') {
            // Skip the native unref for a wasm sim (no native sim pointer).
            if (childEntry.engine !== 'wasm') {
              simlin_sim_unref(childEntry.ptr);
            }
          } else if (childEntry.kind === 'model') {
            simlin_model_unref(childEntry.ptr);
          }
        }
      }
      this._projectChildren.delete(handle as number);
    }
    entry.disposed = true;
    simlin_project_unref(entry.ptr);
  }

  projectGetModelCount(handle: ProjectHandle): number {
    return simlin_project_get_model_count(this.getProjectPtr(handle));
  }

  projectGetModelNames(handle: ProjectHandle): string[] {
    return simlin_project_get_model_names(this.getProjectPtr(handle));
  }

  projectGetModel(handle: ProjectHandle, name: string | null): ModelHandle {
    const ptr = simlin_project_get_model(this.getProjectPtr(handle), name);
    return this.allocHandle('model', ptr, { projectHandle: handle as number }) as ModelHandle;
  }

  projectIsSimulatable(handle: ProjectHandle, modelName: string | null): boolean {
    return simlin_project_is_simulatable(this.getProjectPtr(handle), modelName);
  }

  projectSerializeProtobuf(handle: ProjectHandle): Uint8Array {
    return simlin_project_serialize_protobuf(this.getProjectPtr(handle));
  }

  projectSerializeJson(handle: ProjectHandle, format: SimlinJsonFormat, includeStdlib: boolean = false): Uint8Array {
    return simlin_project_serialize_json(this.getProjectPtr(handle), format, includeStdlib);
  }

  projectSerializeXmile(handle: ProjectHandle): Uint8Array {
    return simlin_project_serialize_xmile(this.getProjectPtr(handle));
  }

  projectRenderSvg(handle: ProjectHandle, modelName: string): Uint8Array {
    return simlin_project_render_svg(this.getProjectPtr(handle), modelName);
  }

  projectRenderPng(handle: ProjectHandle, modelName: string, width: number, height: number): Uint8Array {
    return simlin_project_render_png(this.getProjectPtr(handle), modelName, width, height);
  }

  projectGetErrors(handle: ProjectHandle): ErrorDetail[] {
    const errPtr = simlin_project_get_errors(this.getProjectPtr(handle));
    if (errPtr === 0) {
      return [];
    }
    const details = readAllErrorDetails(errPtr);
    simlin_error_free(errPtr);
    return details;
  }

  projectApplyPatch(
    handle: ProjectHandle,
    patch: JsonProjectPatch,
    dryRun: boolean,
    allowErrors: boolean,
  ): ErrorDetail[] {
    const patchJson = JSON.stringify(patch);
    const patchBytes = new TextEncoder().encode(patchJson);

    const collectedPtr = simlin_project_apply_patch(this.getProjectPtr(handle), patchBytes, dryRun, allowErrors);

    if (collectedPtr === 0) {
      return [];
    }

    const details = readAllErrorDetails(collectedPtr);
    simlin_error_free(collectedPtr);
    return details;
  }

  // Model operations

  modelGetName(handle: ModelHandle): string {
    return simlin_model_get_name(this.getModelPtr(handle));
  }

  modelDispose(handle: ModelHandle): void {
    const entry = this._handles.get(handle as number);
    if (!entry || entry.disposed) {
      return; // idempotent
    }
    entry.disposed = true;
    if (entry.projectHandle !== undefined) {
      this._projectChildren.get(entry.projectHandle)?.delete(handle as number);
    }
    simlin_model_unref(entry.ptr);
  }

  modelGetIncomingLinks(handle: ModelHandle, varName: string): string[] {
    return simlin_model_get_incoming_links(this.getModelPtr(handle), varName);
  }

  modelGetLinks(handle: ModelHandle): Link[] {
    const linksPtr = simlin_model_get_links_fn(this.getModelPtr(handle));
    return convertLinks(linksPtr);
  }

  modelGetLoops(handle: ModelHandle): Loop[] {
    const loopsPtr = simlin_analyze_get_loops(this.getModelPtr(handle));
    if (loopsPtr === 0) {
      return [];
    }
    let loops: Loop[] = [];
    try {
      const rawLoops = readLoops(loopsPtr);
      loops = rawLoops.map((loop) => ({
        id: loop.id,
        variables: loop.variables,
        polarity: loop.polarity as unknown as LoopPolarity,
      }));
    } finally {
      simlin_free_loops(loopsPtr);
    }
    return loops;
  }

  modelGetLatexEquation(handle: ModelHandle, ident: string): string | null {
    return simlin_model_get_latex_equation(this.getModelPtr(handle), ident);
  }

  modelGetVarJson(handle: ModelHandle, varName: string): Uint8Array {
    return simlin_model_get_var_json(this.getModelPtr(handle), varName);
  }

  modelGetVarNames(handle: ModelHandle, typeMask: number = 0, filter: string | null = null): string[] {
    return simlin_model_get_var_names(this.getModelPtr(handle), typeMask, filter);
  }

  modelGetSimSpecsJson(handle: ModelHandle): Uint8Array {
    return simlin_model_get_sim_specs_json(this.getModelPtr(handle));
  }

  // Sim operations

  simNew(modelHandle: ModelHandle, enableLtm: boolean, engine: SimEngine = 'vm'): SimHandle {
    const modelEntry = this.getEntry(modelHandle as number, 'model');
    if (engine === 'wasm') {
      return this.simNewWasm(modelHandle, modelEntry, enableLtm);
    }
    const ptr = simlin_sim_new(modelEntry.ptr, enableLtm);
    return this.allocHandle('sim', ptr, {
      projectHandle: modelEntry.projectHandle,
      engine: 'vm',
    }) as SimHandle;
  }

  /**
   * Create a wasm-engine sim: compile the model to a self-contained wasm blob,
   * instantiate it import-free, and store the instance + decoded layout + stop
   * time on the handle entry. There is intentionally no VM fallback -- an
   * unsupported model surfaces the compile error to the caller.
   */
  private simNewWasm(modelHandle: ModelHandle, modelEntry: HandleEntry, enableLtm: boolean): SimHandle {
    // Reject LTM up front, before any compile work: the wasm backend does not
    // emit LTM instrumentation, so a wasm sim can never satisfy enableLtm.
    if (enableLtm) {
      throw new Error("LTM is not supported on the wasm engine; use engine:'vm'");
    }

    // Throws SimlinError on an unsupported model (e.g. a runtime view range);
    // we deliberately do not catch-and-fall-back to the VM.
    const { wasm, layout } = simlin_model_compile_to_wasm(modelEntry.ptr);
    const parsed = parseWasmLayout(layout);

    // Capture the model's stop time so simRunToEnd can drive run_to(stop),
    // mirroring Model.timeSpec()'s defensive endTime parse (model.ts:297).
    const specs = JSON.parse(new TextDecoder().decode(this.modelGetSimSpecsJson(modelHandle))) as {
      endTime?: number;
    };
    const wasmStopTime = specs.endTime ?? 10;

    // The blob is import-free and DirectBackend never runs on the browser main
    // thread, so synchronous compile + instantiate is allowed here. The blob has
    // its own (non-growing) linear memory, independent of the libsimlin singleton.
    // `copyFromWasm` returns a fresh, non-shared Uint8Array (byteOffset 0), so its
    // backing buffer is a plain ArrayBuffer -- the cast only drops the lib's
    // ArrayBufferLike widening (which admits SharedArrayBuffer) that does not apply here.
    const wasmBytes = wasm.buffer as ArrayBuffer;
    const instance = new WebAssembly.Instance(new WebAssembly.Module(wasmBytes), {});
    const wasmExports = instance.exports as unknown as WasmBlobExports;

    return this.allocHandle('sim', 0, {
      projectHandle: modelEntry.projectHandle,
      engine: 'wasm',
      wasmInstance: instance,
      wasmLayout: parsed,
      wasmExports,
      wasmStopTime,
    }) as SimHandle;
  }

  simDispose(handle: SimHandle): void {
    const entry = this._handles.get(handle as number);
    if (!entry || entry.disposed) {
      return; // idempotent
    }
    entry.disposed = true;
    if (entry.projectHandle !== undefined) {
      this._projectChildren.get(entry.projectHandle)?.delete(handle as number);
    }
    // A wasm sim has no native sim pointer; dropping the entry lets the
    // WebAssembly.Instance be GC'd. Only the VM path holds a native sim to unref.
    if (entry.engine !== 'wasm') {
      simlin_sim_unref(entry.ptr);
    }
  }

  simRunTo(handle: SimHandle, time: number): void {
    simlin_sim_run_to(this.getSimPtr(handle), time);
  }

  simRunToEnd(handle: SimHandle): void {
    simlin_sim_run_to_end(this.getSimPtr(handle));
  }

  simReset(handle: SimHandle): void {
    simlin_sim_reset(this.getSimPtr(handle));
  }

  simGetTime(handle: SimHandle): number {
    return simlin_sim_get_value(this.getSimPtr(handle), 'time');
  }

  simGetStepCount(handle: SimHandle): number {
    return simlin_sim_get_stepcount(this.getSimPtr(handle));
  }

  simGetValue(handle: SimHandle, name: string): number {
    return simlin_sim_get_value(this.getSimPtr(handle), name);
  }

  simSetValue(handle: SimHandle, name: string, value: number): void {
    simlin_sim_set_value(this.getSimPtr(handle), name, value);
  }

  simGetSeries(handle: SimHandle, name: string): Float64Array {
    const stepCount = this.simGetStepCount(handle);
    return simlin_sim_get_series(this.getSimPtr(handle), name, stepCount);
  }

  simGetVarNames(handle: SimHandle): string[] {
    return simlin_sim_get_var_names_fn(this.getSimPtr(handle));
  }

  simGetLinks(handle: SimHandle): Link[] {
    const linksPtr = simlin_analyze_get_links(this.getSimPtr(handle));
    return convertLinks(linksPtr);
  }
}
