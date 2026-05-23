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
import {
  simlin_model_compile_to_wasm,
  parseWasmLayout,
  readStridedSeries,
  WasmLayout,
  WasmBlobExports,
} from './internal/wasmgen';
import { canonicalizeIdent } from './internal/canonicalize';
import {
  SimlinProjectPtr,
  SimlinModelPtr,
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

/**
 * Compare two strings by Unicode code point, matching Rust's `str` `sort()`
 * (which orders by UTF-8 bytes -- equivalent to code-point order for valid
 * Unicode). The default JS `Array.prototype.sort` compares by UTF-16 code unit,
 * which mis-orders characters outside the BMP (a surrogate-pair lead unit
 * 0xD800-0xDBFF sorts below a BMP char like U+E000 even though its code point is
 * higher), so the wasm `getVarNames` must use this comparator to stay byte-for-
 * byte identical to the VM's sorted output.
 */
function compareByCodePoint(a: string, b: string): number {
  const ai = a[Symbol.iterator]();
  const bi = b[Symbol.iterator]();
  for (;;) {
    const an = ai.next();
    const bn = bi.next();
    if (an.done || bn.done) {
      // The shorter string sorts first; if both ended, they are equal.
      return an.done ? (bn.done ? 0 : -1) : 1;
    }
    const ac = an.value.codePointAt(0)!;
    const bc = bn.value.codePointAt(0)!;
    if (ac !== bc) {
      return ac - bc;
    }
  }
}

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
            // A wasm sim has no native sim pointer; release its heavy wasm state
            // (instance + layout) instead of unref'ing, so disposing the project
            // does not leave child WebAssembly.Instances pinned via the map.
            if (childEntry.engine === 'wasm') {
              this.releaseWasmSimState(childEntry);
            } else {
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

  /**
   * Release a wasm sim entry's heavy state (the WebAssembly.Instance, its
   * exports, and the decoded layout). The disposed entry is intentionally kept
   * in `_handles` as a tombstone so a use-after-dispose still throws the clear
   * "has been disposed" diagnostic; but those heavy refs must be cleared, or the
   * map would pin a whole WebAssembly.Instance + layout per disposed sim and
   * memory would grow unbounded across create/dispose cycles. `wasmStopTime` is
   * a plain number, so it costs nothing to leave -- only the heavy refs matter.
   */
  private releaseWasmSimState(entry: HandleEntry): void {
    entry.wasmInstance = undefined;
    entry.wasmExports = undefined;
    entry.wasmLayout = undefined;
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
    // A wasm sim has no native sim pointer; instead it owns a WebAssembly.Instance,
    // so explicitly release that heavy state (the tombstone stays for diagnostics).
    // Only the VM path holds a native sim to unref.
    if (entry.engine === 'wasm') {
      this.releaseWasmSimState(entry);
    } else {
      simlin_sim_unref(entry.ptr);
    }
  }

  /**
   * Resolve a caller variable name to its f64 slot in the wasm layout.
   * Canonicalizes the name (Rust-faithful) and looks it up in `varOffsets`;
   * throws an "unknown variable" error when absent (parity with the VM's
   * not-found error on by-name reads/writes).
   */
  private wasmSlot(layout: WasmLayout, name: string): number {
    const slot = layout.varOffsets.get(canonicalizeIdent(name));
    if (slot === undefined) {
      throw new Error(`unknown variable: ${name}`);
    }
    return slot;
  }

  simRunTo(handle: SimHandle, time: number): void {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // The blob's run_to is resumable (calls run_initials internally and
      // resumes from the prior cursor); a time past the stop is clamped by the blob.
      entry.wasmExports!.run_to(time);
      return;
    }
    simlin_sim_run_to(entry.ptr, time);
  }

  simRunToEnd(handle: SimHandle): void {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // Drive run_to(stop), mirroring the VM's run_to(specs.stop).
      entry.wasmExports!.run_to(entry.wasmStopTime!);
      return;
    }
    simlin_sim_run_to_end(entry.ptr);
  }

  simReset(handle: SimHandle): void {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // Phase-1 reset: clears the run cursor, preserves constant overrides.
      entry.wasmExports!.reset();
      return;
    }
    simlin_sim_reset(entry.ptr);
  }

  simGetTime(handle: SimHandle): number {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // `time` is slot 0 of the live curr chunk at linear-memory base 0.
      return new DataView(entry.wasmExports!.memory.buffer).getFloat64(0, true);
    }
    return simlin_sim_get_value(entry.ptr, 'time');
  }

  simGetStepCount(handle: SimHandle): number {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // Return COMPLETED steps, not the slab capacity. The blob's live G_SAVED
      // counter (the `saved_steps` global) equals nChunks after a full run but
      // is 0 before any run and after reset -- so reading nChunks here would
      // falsely report a complete run on a fresh/just-reset sim. The exported
      // i32 global's `.value` is typed `any`, so coerce through Number().
      return Number(entry.wasmExports!.saved_steps.value);
    }
    return simlin_sim_get_stepcount(entry.ptr);
  }

  simGetValue(handle: SimHandle, name: string): number {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // Read the variable's current value from the blob's live curr chunk at
      // linear-memory base 0, mirroring the VM's get_value_now (vm.rs:880-887).
      const slot = this.wasmSlot(entry.wasmLayout!, name);
      return new DataView(entry.wasmExports!.memory.buffer).getFloat64(slot * 8, true);
    }
    return simlin_sim_get_value(entry.ptr, name);
  }

  simSetValue(handle: SimHandle, name: string, value: number): void {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      const slot = this.wasmSlot(entry.wasmLayout!, name);
      const rc = entry.wasmExports!.set_value(slot, value);
      if (rc !== 0) {
        // The blob returns nonzero when the slot is not a settable constant,
        // mirroring the VM's BadOverride rejection (constants only).
        throw new Error(`cannot set value of '${name}': not a simple constant`);
      }
      return;
    }
    simlin_sim_set_value(entry.ptr, name, value);
  }

  simGetSeries(handle: SimHandle, name: string): Float64Array {
    const entry = this.getEntry(handle as number, 'sim');
    // Both engines truncate the series to the completed-step count so a partially
    // run -- or just-reset -- sim never exposes uncommitted/stale tail rows. On
    // wasm the results slab keeps its full nChunks capacity even when saved_steps
    // is smaller (reset clears the run cursor but not the slab), so slicing the
    // strided read to the saved count is what keeps it at VM parity (the VM
    // returns only saved rows mid-run and bounds the read by the passed count).
    const stepCount = this.simGetStepCount(handle);
    if (entry.engine === 'wasm') {
      // Read memory.buffer fresh per call (uniform with the singleton helpers).
      const slot = this.wasmSlot(entry.wasmLayout!, name);
      return readStridedSeries(entry.wasmExports!.memory.buffer, entry.wasmLayout!, slot, stepCount);
    }
    return simlin_sim_get_series(entry.ptr, name, stepCount);
  }

  simGetVarNames(handle: SimHandle): string[] {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // Mirror the VM's simlin_sim_get_var_names: filter ONLY `$`-prefixed
      // internal vars (is_internal_var) -- the reserved time/dt/initial_time/
      // final_time names are kept -- and sort by Rust byte order. Rust's
      // str `sort()` compares by UTF-8 bytes, which for valid Unicode orders
      // identically to code-point order; the default JS UTF-16 Array.sort
      // would mis-order non-ASCII (surrogate-pair) names, so compare by code point.
      const names = Array.from(entry.wasmLayout!.varOffsets.keys()).filter((n) => !n.startsWith('$'));
      names.sort(compareByCodePoint);
      return names;
    }
    return simlin_sim_get_var_names_fn(entry.ptr);
  }

  simGetLinks(handle: SimHandle): Link[] {
    const entry = this.getEntry(handle as number, 'sim');
    if (entry.engine === 'wasm') {
      // LTM link scores are a VM-only analysis; a wasm sim never enables LTM.
      throw new Error("getLinks is not supported on the wasm engine; use engine:'vm'");
    }
    const linksPtr = simlin_analyze_get_links(entry.ptr);
    return convertLinks(linksPtr);
  }
}
