// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// ProjectController is the headless coordination layer extracted from
// Editor.tsx. It owns the WASM engine lifecycle, the apply-patch ->
// serialize -> rebuild pipeline, the save queue, undo/redo history, sim
// runs, the cached-error derivation, version/generation bookkeeping, and
// the module-navigation stack. It has ZERO React and ZERO DOM dependencies
// (no document/window; setTimeout is allowed for deferred dispatch) so the
// async coordination can be unit-tested against a fake engine without jsdom.
//
// The Editor is a thin view binding: it subscribes to the controller's
// snapshot, mirrors it into one state field, and builds JSON ops that it
// hands to controller.applyPatch()/updateView()/queueViewUpdate().
//
// The controller never owns presentation state. Toast-style transient errors
// (the Editor's `modelErrors`) are surfaced via the `onError` config callback;
// the Editor decides how to present them.

import {
  Project,
  Model,
  Variable,
  EquationError,
  UnitError,
  SimError,
  ModelError,
  ErrorCode,
  StockFlowView,
  UID,
  Rect,
  projectFromJson,
  projectAttachData,
  findNonFiniteViewCoord,
  stockFlowViewFromJson,
  stockFlowViewToJson,
} from '@simlin/core/datamodel';
import { defined, mapSet, setsEqual, toInt, uint8ArraysEqual, type Series } from '@simlin/core/common';
import { first, getOrThrow } from '@simlin/core/collections';
import type { JsonProjectPatch, JsonModelOperation, ErrorDetail, JsonProject } from '@simlin/engine';
import { SimlinErrorKind, SimlinUnitErrorKind } from '@simlin/engine';

import { preserveLiveView } from './merge-live-view';
import { advanceProjectHistory } from './project-history';
import {
  type ModuleStackEntry,
  currentModelName,
  pushModule,
  popModule,
  navigateToLevel,
  isStdlibModel,
  isMacroModel,
} from './module-navigation';
import { computeConnectorErrors } from './connector-sync';

/**
 * The maximum number of undo snapshots kept. A small buffer is intentional:
 * undo is a convenience for the last few edits, not a full revision history,
 * and each snapshot is a complete serialized protobuf.
 */
export const MaxUndoSize = 5;

/**
 * Cached, model-scoped error derivation. Recomputed from `engine.getErrors()`
 * whenever the project content or the active model changes. The Editor reads
 * this from the snapshot to render the error panel and warning dots.
 */
export interface CachedErrorDetails {
  readonly varErrors: ReadonlyMap<string, readonly EquationError[]>;
  readonly unitErrors: ReadonlyMap<string, readonly UnitError[]>;
  readonly simError: SimError | undefined;
  readonly modelErrors: readonly ModelError[];
}

/**
 * The immutable view of controller state the Editor renders from. A fresh
 * object is produced on every change so identity comparison (===) detects
 * updates; prior snapshots are never mutated.
 */
export interface ProjectSnapshot {
  readonly project: Project | undefined;
  // The fractional cache-key scheme (+0.01 for content edits, +0.001 for
  // view-only updates) is preserved: Canvas keys render caches off this.
  readonly projectVersion: number;
  // Increments exactly when project *content* changes (real edits and
  // undo/redo) -- not on view-only updates or save-version bookkeeping.
  // The Editor keys the details panels on this so a pan frame or autosave
  // does not remount an open panel and discard in-progress edits.
  readonly projectGeneration: number;
  readonly status: 'ok' | 'error' | 'disabled';
  readonly cachedErrors: CachedErrorDetails;
  readonly data: ReadonlyMap<string, Series>;
  readonly modelName: string;
  readonly modelStack: readonly ModuleStackEntry[];
  readonly canUndo: boolean;
  readonly canRedo: boolean;
  // Monotonic counter bumped only when undo/redo resets navigation to 'main'
  // because the restored project no longer contains the viewed model. The
  // Editor watches this to clear its own selection/details/tool UI state for
  // that specific case (ordinary undo preserves them). Drill-in / back / level
  // are driven by the Editor's own handlers (via the NavigationOutcome return),
  // so they do NOT bump this.
  readonly navResetSeq: number;
}

/** The subset of the engine `Project` API the controller depends on. */
export interface EngineApi {
  applyPatch(patch: JsonProjectPatch, options?: { dryRun?: boolean; allowErrors?: boolean }): Promise<ErrorDetail[]>;
  serializeProtobuf(): Promise<Uint8Array>;
  serializeJson(format?: unknown, includeStdlib?: boolean): Promise<string>;
  getErrors(): Promise<ErrorDetail[]>;
  isSimulatable(modelName?: string | null): Promise<boolean>;
  mainModel(): Promise<EngineModelApi>;
  getModel(modelName: string | null): Promise<EngineModelApi>;
  dispose(): Promise<void>;
}

/** The subset of the engine `Model` API the controller depends on (sim runs
 * plus the per-variable equation-dependency query used for connector-sync). */
export interface EngineModelApi {
  run(overrides?: Record<string, number>, options?: { analyzeLtm?: boolean }): Promise<EngineRunApi>;
  getIncomingLinks(varName: string): Promise<readonly string[]>;
}

/** The subset of an engine `Run` the controller depends on. */
export interface EngineRunApi {
  readonly varNames: readonly string[];
  getSeries(name: string): Float64Array;
}

/**
 * Configuration injected by the host (the Editor). The two `open*` factories
 * isolate the controller from the concrete `EngineProject` static methods so
 * it can be unit-tested against a fake engine. `onError` surfaces transient
 * errors to the host's toast UI; `onChange` notifies subscribers (the Editor
 * subscribes through `subscribe()`, which wraps this).
 */
export interface ProjectControllerConfig {
  readonly initialProjectVersion: number;
  readonly input:
    | { readonly format: 'protobuf'; readonly data: Readonly<Uint8Array> }
    | { readonly format: 'json'; readonly data: string };
  readonly openProtobuf: (data: Uint8Array) => Promise<EngineApi>;
  readonly openJson: (data: string) => Promise<EngineApi>;
  readonly save: (
    project: { format: 'protobuf'; data: Uint8Array } | { format: 'json'; data: string },
    currVersion: number,
  ) => Promise<number | undefined>;
  readonly onError: (err: Error) => void;
}

interface ErrorDetailsLike {
  code?: unknown;
  message?: string;
  details?: unknown;
}

function getErrorDetails(error: unknown): ErrorDetailsLike {
  if (typeof error === 'object' && error !== null) {
    const maybeError = error as Record<string, unknown>;
    return {
      code: maybeError.code,
      message: typeof maybeError.message === 'string' ? maybeError.message : undefined,
      details: maybeError.details,
    };
  }
  if (typeof error === 'string') {
    return { message: error };
  }
  return {};
}

/**
 * Convert the engine's flat error list into the model-scoped equation/unit
 * error maps the Editor renders. Errors for other models are filtered out.
 */
export function convertErrorDetails(
  errors: readonly ErrorDetail[],
  modelName: string,
): {
  varErrors: ReadonlyMap<string, readonly EquationError[]>;
  unitErrors: ReadonlyMap<string, readonly UnitError[]>;
} {
  const varErrors = new Map<string, EquationError[]>();
  const unitErrors = new Map<string, UnitError[]>();

  for (const err of errors) {
    if (err.modelName !== modelName) {
      continue;
    }

    const ident = err.variableName;
    if (!ident) {
      continue;
    }

    const isUnitError = err.kind === SimlinErrorKind.Units;

    if (isUnitError) {
      const unitError: UnitError = {
        start: err.startOffset ?? 0,
        end: err.endOffset ?? 0,
        code: err.code as unknown as ErrorCode,
        isConsistencyError: err.unitErrorKind === SimlinUnitErrorKind.Consistency,
        details: err.message ?? undefined,
      };
      let existing = unitErrors.get(ident);
      if (!existing) {
        existing = [];
        unitErrors.set(ident, existing);
      }
      existing.push(unitError);
    } else {
      const eqError: EquationError = {
        start: err.startOffset ?? 0,
        end: err.endOffset ?? 0,
        code: err.code as unknown as ErrorCode,
      };
      let existing = varErrors.get(ident);
      if (!existing) {
        existing = [];
        varErrors.set(ident, existing);
      }
      existing.push(eqError);
    }
  }

  return { varErrors, unitErrors };
}

const EMPTY_CACHED_ERRORS: CachedErrorDetails = {
  varErrors: new Map<string, readonly EquationError[]>(),
  unitErrors: new Map<string, readonly UnitError[]>(),
  simError: undefined,
  modelErrors: [],
};

/** The result of a navigation method, describing the UI consequences the
 * Editor must apply (selection restoration, panel/tool resets). Viewport
 * restoration is handled internally by the controller via queueViewUpdate. */
export interface NavigationOutcome {
  // The selection to restore (drill-in clears it; back/level restore the
  // parent's). Undefined means "navigation did not happen" (e.g. drill-in
  // into a model not present in the project).
  readonly restoredSelection: ReadonlySet<UID> | undefined;
}

/**
 * Headless coordination for a single open project. Create one per mounted
 * Editor; call `dispose()` exactly once when the Editor unmounts.
 *
 * StrictMode safety: the Editor creates the controller in componentDidMount
 * and disposes it in componentWillUnmount. A mount -> unmount -> mount cycle
 * on the same Editor instance (React 18 StrictMode) therefore creates a
 * *fresh* controller on the second mount -- the first one was disposed. The
 * controller itself need not be re-armable after dispose; `disposed` latches
 * true and every async continuation short-circuits on it.
 */
export class ProjectController {
  private readonly config: ProjectControllerConfig;

  // The live engine handle. Undefined before openInitialProject() resolves
  // and after dispose().
  private engine: EngineApi | undefined = undefined;

  // --- snapshot-backing state ---
  private project: Project | undefined = undefined;
  private projectHistory: readonly Readonly<Uint8Array>[];
  private projectOffset = 0;
  private projectVersion: number;
  private projectGeneration = 0;
  private status: 'ok' | 'error' | 'disabled' = 'disabled';
  private cachedErrors: CachedErrorDetails = EMPTY_CACHED_ERRORS;
  private data: ReadonlyMap<string, Series> = new Map<string, Series>();
  private modelName = 'main';
  private modelStack: readonly ModuleStackEntry[] = [];
  private navResetSeq = 0;

  // The currently-published immutable snapshot. Replaced wholesale whenever
  // any backing field changes and a notify is flushed.
  private snapshot: ProjectSnapshot;

  // --- save queue ---
  private inSave = false;
  private saveQueued = false;

  // --- new-engine view race ---
  // There exists a race where we need to center/update the viewBox when
  // displaying a newly imported model, but the async wasm round-trip hasn't
  // completed before we want to save the viewBox change. We stash the queued
  // view and replay it once the new engine is installed.
  private newEngineShouldPullView = false;
  private newEngineQueuedView: StockFlowView | undefined = undefined;

  // --- lifecycle ---
  // Latches true on dispose(). Every async continuation checks it before
  // touching state, opening an engine, or notifying subscribers, so work that
  // was already in flight at dispose time cannot resurrect a dead controller.
  private disposed = false;

  // --- notification coalescing ---
  private readonly listeners = new Set<() => void>();
  // Depth counter so a synchronous multi-step mutation (the old code's single
  // setState batch) flushes exactly one notify. notify() increments published
  // state but defers the listener fan-out until the outermost batch closes.
  private batchDepth = 0;
  private snapshotDirty = false;

  constructor(config: ProjectControllerConfig) {
    this.config = config;
    this.projectVersion = config.initialProjectVersion;
    this.projectHistory = config.input.format === 'protobuf' ? [config.input.data] : [];
    this.snapshot = this.buildSnapshot();
  }

  // --- subscription API ---

  /** Subscribe to snapshot changes. Returns an unsubscribe function. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** The current immutable snapshot. Stable identity until the next change. */
  getSnapshot(): ProjectSnapshot {
    return this.snapshot;
  }

  private buildSnapshot(): ProjectSnapshot {
    return {
      project: this.project,
      projectVersion: this.projectVersion,
      projectGeneration: this.projectGeneration,
      status: this.status,
      cachedErrors: this.cachedErrors,
      data: this.data,
      modelName: this.modelName,
      modelStack: this.modelStack,
      canUndo: this.canUndo(),
      canRedo: this.canRedo(),
      navResetSeq: this.navResetSeq,
    };
  }

  /**
   * Mark the snapshot stale and (when not inside a batch) flush a single
   * notification. Subscribers run after the new snapshot is published, so a
   * listener calling getSnapshot() sees the latest state. Disposed controllers
   * never notify.
   */
  private notify(): void {
    this.snapshotDirty = true;
    if (this.batchDepth > 0) {
      return;
    }
    this.flush();
  }

  private flush(): void {
    if (!this.snapshotDirty) {
      return;
    }
    this.snapshotDirty = false;
    this.snapshot = this.buildSnapshot();
    if (this.disposed) {
      return;
    }
    for (const listener of this.listeners) {
      listener();
    }
  }

  /**
   * Coalesce all snapshot changes made inside `fn` into a single notify. This
   * mirrors the old code's batching of multiple synchronous setState-equivalent
   * changes into one render. Re-entrant: only the outermost batch flushes.
   */
  private batch<T>(fn: () => T): T {
    this.batchDepth++;
    try {
      return fn();
    } finally {
      this.batchDepth--;
      if (this.batchDepth === 0) {
        this.flush();
      }
    }
  }

  // --- undo/redo predicates ---

  canUndo(): boolean {
    return this.projectHistory.length > 1 && this.projectOffset < this.projectHistory.length - 1;
  }

  canRedo(): boolean {
    return this.projectOffset > 0;
  }

  // --- engine lifecycle ---

  /**
   * Open the initial project in the engine and rebuild `project`. Idempotent
   * against dispose: if dispose() races in before/after the open completes,
   * the freshly-opened engine is released here rather than stranded.
   *
   * The try/catch deliberately extends past the engine open: the post-open
   * steps (serializeProtobuf, serializeJson, projectFromJson) can still throw
   * (a WASM panic, or projectFromJson rejecting an unknown view element type).
   * Catching here surfaces a contextual message and disposes the orphaned
   * engine, which is strictly better than leaving the user on a blank canvas.
   */
  async openInitialProject(): Promise<void> {
    let engine: EngineApi;
    try {
      engine =
        this.config.input.format === 'json'
          ? await this.config.openJson(this.config.input.data)
          : await this.config.openProtobuf(this.config.input.data as Uint8Array);
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      this.reportError(`opening the project in the engine failed: ${err.message ?? 'Unknown error'}`);
      return;
    }

    if (this.disposed) {
      // dispose() ran during the engine open. Release the orphan: dispose()
      // could not reach an engine that didn't exist yet.
      await this.disposeOrphanedEngine(engine);
      return;
    }

    try {
      this.engine = engine;

      const serializedProject = await engine.serializeProtobuf();
      const json = JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject;
      // No live view exists on first open, so the engine-serialized view IS the
      // rendered view -- compute connector annotations directly against it.
      const project = await this.attachConnectorErrors(await this.updateVariableErrors(projectFromJson(json)));

      if (this.disposed) {
        this.engine = undefined;
        await this.disposeOrphanedEngine(engine);
        return;
      }

      this.batch(() => {
        this.projectHistory = [serializedProject];
        this.project = project;
        this.notify();
      });
    } catch (e: unknown) {
      this.engine = undefined;
      await this.disposeOrphanedEngine(engine);
      const err = getErrorDetails(e);
      this.reportError(`opening the project failed: ${err.message ?? 'Unknown error'}`);
    }
  }

  /**
   * Reopen the engine from a serialized snapshot (the undo/redo path). Disposes
   * the previous engine first. Returns the new engine on success, undefined on
   * failure. See openInitialProject for why the post-open steps are guarded.
   */
  private async openEngineProject(serializedProject: Readonly<Uint8Array>): Promise<EngineApi | undefined> {
    await this.engine?.dispose();
    this.engine = undefined;

    let engine: EngineApi;
    try {
      engine = await this.config.openProtobuf(serializedProject as Uint8Array);
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      this.reportError(`opening the project in the engine failed: ${err.message ?? 'Unknown error'}`);
      return undefined;
    }

    if (this.disposed) {
      await this.disposeOrphanedEngine(engine);
      return undefined;
    }

    try {
      this.engine = engine;

      const json = JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject;
      let project = projectFromJson(json);

      if (this.newEngineShouldPullView) {
        const queuedView = defined(this.newEngineQueuedView);
        this.newEngineShouldPullView = false;
        this.newEngineQueuedView = undefined;
        const model = defined(project.models.get(this.modelName));
        const views = [...model.views];
        views[0] = queuedView;
        const updatedModel = { ...model, views };
        project = { ...project, models: mapSet(project.models, this.modelName, updatedModel) };
        // queueViewUpdate is async; it will round-trip the queued view to the
        // freshly-installed engine. We intentionally do not await it here.
        void this.queueViewUpdate(queuedView);
      }

      // The rendered view here is `project`'s view -- either the engine snapshot
      // or the queued view spliced in above -- so annotate directly against it.
      const withErrors = await this.attachConnectorErrors(await this.updateVariableErrors(project));

      if (this.disposed) {
        this.engine = undefined;
        await this.disposeOrphanedEngine(engine);
        return undefined;
      }

      this.batch(() => {
        this.project = withErrors;
        // A reopen restores different project content (the undo/redo path). Bump
        // the version (Canvas render-cache key) and the generation (detail-panel
        // remount key) in the SAME notification as the content swap, so the
        // version-keyed element cache rebuilds from the restored view and the
        // panels re-seed from restored content. undoRedo deliberately does NOT
        // bump these synchronously -- see the comment there (#817).
        this.projectVersion = this.projectVersion + 0.01;
        this.projectGeneration += 1;
        this.notify();
      });

      return engine;
    } catch (e: unknown) {
      this.engine = undefined;
      await this.disposeOrphanedEngine(engine);
      const err = getErrorDetails(e);
      this.reportError(`opening the project failed: ${err.message ?? 'Unknown error'}`);
      return undefined;
    }
  }

  /**
   * Release the WASM engine handle and latch the controller disposed. Safe to
   * call before openInitialProject() resolves: a still-in-flight open detects
   * the disposed flag and releases its own engine. Best-effort: a throwing
   * dispose must not crash the host.
   */
  async dispose(): Promise<void> {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.listeners.clear();
    const engine = this.engine;
    this.engine = undefined;
    if (engine) {
      await this.disposeOrphanedEngine(engine);
    }
  }

  /**
   * Release an engine handle we opened but never wired into a live snapshot,
   * so the WASM allocation doesn't leak. dispose() is best-effort: a throwing
   * dispose must not mask the original error we're surfacing.
   */
  private async disposeOrphanedEngine(engine: EngineApi): Promise<void> {
    try {
      await engine.dispose();
    } catch {
      // ignored: the engine is being abandoned regardless
    }
  }

  // --- the update pipeline ---

  /**
   * Apply a content patch and, on success, rebuild `project` from the engine
   * and schedule a re-simulation. Returns false (without rebuilding or
   * scheduling) when the patch throws.
   *
   * `label` identifies the operation in the user-facing fallback message when
   * the engine reports no message.
   */
  async applyPatch(patch: JsonProjectPatch, label: string): Promise<boolean> {
    if (!(await this.applyPatchOrReportError(patch, label))) {
      return false;
    }
    await this.refreshFromEngine();
    return true;
  }

  /**
   * Apply a patch (allowing errors so partially-invalid models can be edited),
   * reporting any failure. Returns false on failure so callers can bail. This
   * is split from refreshFromEngine() so a caller can interleave its own state
   * updates between the patch and the (async, serialize-heavy) round-trip.
   */
  async applyPatchOrReportError(patch: JsonProjectPatch, label: string): Promise<boolean> {
    const engine = this.engine;
    if (!engine) {
      return false;
    }
    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error(`applyPatch error (${label}):`, err.code, err.message, err.details);
      this.reportError(err.message ?? `Unknown error during ${label}`);
      return false;
    }
    this.adoptPatchedViews(patch);
    return true;
  }

  /**
   * Mirror any primary-view upsert carried by a just-applied patch into the
   * live project, exactly as updateView's optimistic step does.
   *
   * Why: preserveLiveView (see updateProject) always keeps the ACTIVE model's
   * live view on refresh, protecting newer optimistic pans/moves from older
   * engine snapshots. But a view arriving in an explicit upsertView op (e.g.
   * a rename patching the variable and its view together) is newer user intent
   * than the live view by construction -- without this mirror, the stale live
   * view clobbers the patched one on refresh, the edit looks like a silent
   * no-op, and the next geometry edit round-trips the stale view back into the
   * engine, persisting a model/view divergence.
   *
   * Elements are re-linked against the CURRENT (pre-patch) variables; refs to
   * variables the patch introduced resolve as undefined until the follow-up
   * refreshFromEngine re-links them (same transient the optimistic paths
   * already tolerate).
   */
  private adoptPatchedViews(patch: JsonProjectPatch): void {
    const variables = this.project?.models.get(this.modelName)?.variables;
    if (!variables) {
      return;
    }
    for (const model of patch.models ?? []) {
      if (model.name !== this.modelName) {
        continue;
      }
      for (const op of model.ops ?? []) {
        if (op.type === 'upsertView' && op.payload.index === 0) {
          this.applyOptimisticView(stockFlowViewFromJson(op.payload.view, variables));
        }
      }
    }
  }

  /** Round-trip the engine's serialized state back into `project` and schedule
   * a re-simulation. Called after a successful patch. */
  async refreshFromEngine(): Promise<void> {
    const engine = this.engine;
    if (!engine) {
      return;
    }
    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  }

  /**
   * Rebuild `project` from a serialized protobuf snapshot. Records undo history
   * and schedules a save unless told otherwise.
   *
   * Preserving the live view: this call may have raced with a newer optimistic
   * setView (the user kept panning while the round-trip was in flight), so the
   * engine snapshot is potentially behind. preserveLiveView keeps the active
   * model's view from the live `project` to avoid the diagram snapping back.
   *
   * View-only updates (recordHistory: false) refresh the rendered project and
   * bump projectVersion but must not touch projectHistory/projectOffset:
   * viewBox/zoom are serialized into the protobuf, so recording them would let
   * a single momentum flick evict every real edit from the small undo buffer.
   */
  async updateProject(
    serializedProject: Readonly<Uint8Array>,
    opts: { scheduleSave?: boolean; recordHistory?: boolean } = {},
  ): Promise<void> {
    const { scheduleSave = true, recordHistory = true } = opts;
    if (this.projectHistory.length > 0) {
      const current = this.projectHistory[this.projectOffset];
      if (uint8ArraysEqual(serializedProject, current)) {
        return;
      }
    }

    const engine = this.engine;
    if (!engine) {
      return;
    }
    // Include stdlib model definitions so the editor can display and navigate
    // into stdlib modules. The save path does NOT pass includeStdlib, so
    // stdlib models are never persisted.
    const json = JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject;
    let activeProject = await this.updateVariableErrors(projectFromJson(json));
    if (this.data) {
      activeProject = projectAttachData(activeProject, this.data, 'main');
    }
    activeProject = preserveLiveView(activeProject, this.project, this.modelName);
    // Connector annotations must reflect the RENDERED view, so compute them only
    // after preserveLiveView has swapped in the (possibly newer) live view --
    // see attachConnectorErrors. Runs after projectAttachData too, so it
    // preserves the attached series when it rewrites the active model's vars.
    activeProject = await this.attachConnectorErrors(activeProject);

    if (this.disposed) {
      return;
    }

    // Fractionally increase the version -- the server only sends back integer
    // versions, but this lets the Canvas use a simple version check to
    // invalidate caches.
    const projectVersion = this.projectVersion + 0.01;

    this.batch(() => {
      if (recordHistory) {
        const nextHistory = advanceProjectHistory(
          { projectHistory: this.projectHistory, projectOffset: this.projectOffset },
          serializedProject,
          MaxUndoSize,
        );
        this.projectHistory = nextHistory.projectHistory;
        this.projectOffset = nextHistory.projectOffset;
        this.projectGeneration += 1;
      }
      this.project = activeProject;
      this.projectVersion = projectVersion;
      this.notify();
    });

    if (scheduleSave) {
      this.scheduleSave();
    }
  }

  /**
   * Optimistic view update for a DISCRETE element/structure edit (create,
   * delete, element/group move, label move, flow/link attach): reflect the new
   * view in the snapshot immediately (so the UI never flashes stale positions),
   * then round-trip through the engine.
   *
   * `recordHistory` controls whether this edit advances the undo buffer; it
   * defaults to false so the bare call stays a non-recording view refresh. Each
   * discrete user edit passes `recordHistory: true` so it becomes individually
   * undoable. For the handlers that apply a content patch (via
   * applyPatchOrReportError) BEFORE calling this, the snapshot serialized here
   * captures the engine state AFTER both the content patch and the view update,
   * so a single recorded entry covers the whole edit -- no double-recording.
   *
   * The per-frame viewport stream (pan/zoom/momentum/resize) does NOT come
   * through here -- it uses queueViewUpdate, which never records, so a momentum
   * flick cannot evict real edits from the small undo buffer.
   */
  /**
   * Guard against a view carrying a non-finite (NaN/Infinity) coordinate. Such a
   * coordinate serializes to JSON `null`, which the engine's patch parser rejects
   * with "invalid type: null, expected f64" -- historically bricking the model
   * (every later edit failed and the element rendered displaced). A non-finite
   * coordinate always means an upstream geometry bug, so we refuse the update
   * entirely (no optimistic apply, no patch) and surface a descriptive error
   * rather than corrupting the model. Returns true when the view is safe to
   * apply. (issue #818)
   */
  private viewCoordsAreFinite(view: StockFlowView): boolean {
    const bad = findNonFiniteViewCoord(view);
    if (bad === undefined) {
      return true;
    }
    this.reportError(`internal error: refusing a view update with a non-finite coordinate (${bad})`);
    return false;
  }

  async updateView(view: StockFlowView, opts: { recordHistory?: boolean } = {}): Promise<void> {
    if (!this.viewCoordsAreFinite(view)) {
      return;
    }
    const { recordHistory = false } = opts;
    this.applyOptimisticView(view);

    const engine = this.engine;
    if (!engine) {
      return;
    }
    const patch = this.viewPatch(view);
    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (view update):', err.code, err.message, err.details);
      this.reportError(err.message ?? 'Unknown error during view update');
      return;
    }
    await this.updateProject(await engine.serializeProtobuf(), { scheduleSave: true, recordHistory });
  }

  /**
   * Like updateView but for viewBox/zoom-only changes (pan/zoom/momentum,
   * panel resizes): optimistic immediate snapshot, async engine round-trip
   * that neither records history nor schedules a save. When no engine is yet
   * installed (a newly imported model still loading), stash the view to replay
   * once the engine arrives.
   */
  async queueViewUpdate(view: StockFlowView): Promise<void> {
    if (!this.viewCoordsAreFinite(view)) {
      return;
    }
    this.applyOptimisticView(view);

    const engine = this.engine;
    if (!engine) {
      this.newEngineShouldPullView = true;
      this.newEngineQueuedView = view;
      return;
    }
    const patch = this.viewPatch(view);
    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (queue view update):', err.code, err.message, err.details);
      this.reportError(err.message ?? 'Unknown error during view update');
      return;
    }
    await this.updateProject(await engine.serializeProtobuf(), { scheduleSave: false, recordHistory: false });
  }

  /**
   * Synchronously replace the active model's primary view in `project` and bump
   * the render version by a small fraction (cache-key only; no history, no
   * generation bump). This is the optimistic step shared by updateView and
   * queueViewUpdate. No-op (other than version bump skipped) when no project is
   * loaded yet.
   */
  private applyOptimisticView(view: StockFlowView): void {
    const project = this.project;
    if (!project) {
      return;
    }
    const model = defined(project.models.get(this.modelName));
    const views = [...model.views];
    views[0] = view;
    const updatedModel = { ...model, views };
    const activeProject = { ...project, models: mapSet(project.models, this.modelName, updatedModel) };

    this.batch(() => {
      this.project = activeProject;
      this.projectVersion = this.projectVersion + 0.001;
      this.notify();
    });
  }

  private viewPatch(view: StockFlowView): JsonProjectPatch {
    const ops: JsonModelOperation[] = [
      {
        type: 'upsertView',
        payload: { index: 0, view: stockFlowViewToJson(view) },
      },
    ];
    return { models: [{ name: this.modelName, ops }] };
  }

  // --- save queue ---

  /**
   * Schedule a save on the current version. Deferred via setTimeout so a burst
   * of edits coalesces. The continuation short-circuits if the controller was
   * disposed before it fired.
   */
  scheduleSave(): void {
    const projectVersion = this.projectVersion;
    setTimeout(() => {
      if (this.disposed) {
        return;
      }
      void this.save(toInt(projectVersion));
    });
  }

  /**
   * Serialize and hand off to the host's save callback. A save already in
   * flight queues exactly one flush. inSave is released in a finally block: a
   * thrown save (e.g. host-side network failure) must not leave inSave stuck
   * true, otherwise every subsequent edit silently queues forever. The queued
   * retry uses `version ?? currVersion` so a save that errored before the
   * server returned a new version still attempts to flush the next edit.
   */
  async save(currVersion: number): Promise<void> {
    if (this.inSave) {
      this.saveQueued = true;
      return;
    }

    this.inSave = true;

    let version: number | undefined;
    try {
      const engine = defined(this.engine);
      if (this.config.input.format === 'json') {
        version = await this.config.save({ format: 'json', data: await engine.serializeJson() }, currVersion);
      } else {
        version = await this.config.save({ format: 'protobuf', data: await engine.serializeProtobuf() }, currVersion);
      }
      if (version) {
        this.projectVersion = version;
        this.notify();
      }
    } catch (err) {
      this.reportError(err instanceof Error ? err : new Error(String(err)));
    } finally {
      this.inSave = false;
      if (this.saveQueued) {
        this.saveQueued = false;
        await this.save(version ?? currVersion);
      }
    }
  }

  // --- undo/redo ---

  /**
   * Move the undo cursor and reopen the engine from the restored snapshot.
   * Only the cursor (`projectOffset`) moves synchronously; the version and
   * generation bump (and the notify that drives re-render / details-panel
   * remount) are deferred to the engine reopen so they land together with the
   * restored content -- see the inline comment below and openEngineProject
   * (#817). One consequence: the UndoRedoBar's enabled state, read from the
   * snapshot, updates a macrotask later (after the reopen notifies) rather than
   * on click; canUndo()/canRedo() the live methods still reflect the cursor
   * immediately. After the reopen, if the restored project no longer contains
   * the viewed model (e.g. undo after creating and drilling into a new
   * submodel), navigation resets to 'main' and `navResetSeq` bumps so the
   * Editor clears its selection/details/tool state.
   */
  undoRedo(kind: 'undo' | 'redo'): void {
    const delta = kind === 'undo' ? 1 : -1;
    let projectOffset = this.projectOffset + delta;
    projectOffset = Math.min(projectOffset, this.projectHistory.length - 1);
    projectOffset = Math.max(projectOffset, 0);
    const serializedProject = defined(this.projectHistory[projectOffset]);

    // Move the undo cursor synchronously so canUndo/canRedo (live methods) and a
    // rapid second click compute the right next offset. But do NOT bump
    // projectVersion/projectGeneration or notify yet: this.project is still the
    // pre-undo content and the rebuild is async. Bumping the version now would
    // make the Canvas cache its uid lookup from the stale (pre-undo) view, then
    // the async reopen would swap in the restored view WITHOUT re-bumping the
    // version -- leaving the version-keyed element cache stale relative to
    // props.view, the transient inconsistency behind the dangling-ref undo crash
    // (#817). The bump + notify happens once, inside openEngineProject, in the
    // same batch as the content swap.
    this.projectOffset = projectOffset;

    setTimeout(() => {
      if (this.disposed) {
        return;
      }
      void this.reopenForUndoRedo(serializedProject);
    });
  }

  private async reopenForUndoRedo(serializedProject: Readonly<Uint8Array>): Promise<void> {
    const engine = await this.openEngineProject(serializedProject);
    if (this.disposed) {
      // The reopen finished against a disposed controller -- release the
      // engine it installed so the WASM allocation isn't stranded.
      this.engine = undefined;
      if (engine) {
        await this.disposeOrphanedEngine(engine);
      }
      return;
    }
    // After undo/redo, the restored project may not contain the model we were
    // viewing. Reset navigation if the current model is gone.
    const project = this.project;
    if (project && this.modelStack.length > 0 && !project.models.has(this.modelName)) {
      this.batch(() => {
        this.modelStack = [];
        this.modelName = 'main';
        this.navResetSeq += 1;
        this.notify();
      });
    }
    this.scheduleSimRun();
    this.scheduleSave();
  }

  // --- sim runs ---

  /** Schedule a deferred simulation run. The continuation short-circuits on
   * dispose or a missing engine. */
  scheduleSimRun(): void {
    setTimeout(() => {
      if (this.disposed) {
        return;
      }
      if (!this.engine) {
        return;
      }
      void this.loadSim();
    });
  }

  /**
   * Recalculate status, then run the main model and attach the resulting series
   * to the root model. Sparklines don't need Loops-That-Matter analysis, and
   * LTM compilation can blow up WASM memory on dense causal graphs (World3:
   * ~1.8M elementary circuits -> RuntimeError: unreachable). We request a plain
   * simulation first; on any failure we retry with LTM explicitly disabled so a
   * future default flip cannot starve the UI of sparkline data. The first
   * failure is surfaced as a warning-style error entry.
   */
  async loadSim(): Promise<void> {
    await this.recalculateStatus();

    const engine = this.engine;
    if (!engine) {
      return;
    }

    if (!(await engine.isSimulatable())) {
      return;
    }
    const model = await engine.mainModel();
    let run: EngineRunApi;
    try {
      run = await model.run();
    } catch (e) {
      this.reportError(e instanceof Error ? e : new Error(String(e)));
      try {
        run = await model.run({}, { analyzeLtm: false });
      } catch (e2) {
        this.reportError(e2 instanceof Error ? e2 : new Error(String(e2)));
        await this.refreshCachedErrors();
        return;
      }
    }

    if (this.disposed) {
      return;
    }

    const idents = run.varNames;
    const time = run.getSeries('time') ?? new Float64Array(0);
    const data = new Map<string, Series>(
      idents.map((ident) => {
        const values = run.getSeries(ident) ?? new Float64Array(0);
        return [ident, { name: ident, time, values }];
      }),
    );
    const project = defined(this.project);
    // Simulation data comes from mainModel(), so variable idents are
    // root-model-scoped. Always attach data to 'main' so root sparklines stay
    // populated even when a sim runs while viewing a child model.
    this.batch(() => {
      this.project = projectAttachData(project, data, 'main');
      this.data = data;
      this.notify();
    });
    // Refresh cached errors after simulation so the error panel reflects any
    // new simulation errors (e.g. runtime divide-by-zero).
    await this.refreshCachedErrors();
  }

  /** Derive simulatability status from the engine and project. */
  async recalculateStatus(): Promise<void> {
    const project = this.project;
    const engine = this.engine;

    let status: 'ok' | 'error' | 'disabled';
    if (!engine || !project || project.hasNoEquations) {
      status = 'disabled';
    } else if (!(await engine.isSimulatable())) {
      status = 'error';
    } else {
      status = 'ok';
    }

    if (this.disposed) {
      return;
    }
    if (status !== this.status) {
      this.status = status;
      this.notify();
    }
  }

  // --- error cache ---

  /**
   * Re-derive the model-scoped cached errors from the engine. Returns the new
   * cache (or undefined when no engine is installed).
   */
  async refreshCachedErrors(): Promise<CachedErrorDetails | undefined> {
    const engine = this.engine;
    if (!engine) {
      return undefined;
    }

    const modelName = this.modelName;
    const errors = await engine.getErrors();
    const { varErrors, unitErrors } = convertErrorDetails(errors, modelName);

    let simError: SimError | undefined;
    const modelErrors: ModelError[] = [];
    for (const err of errors) {
      if (err.modelName && err.modelName !== modelName) {
        continue;
      }
      if (err.kind === SimlinErrorKind.Simulation) {
        simError = {
          code: err.code as unknown as ErrorCode,
          details: err.message ?? undefined,
        };
      } else if (!err.variableName) {
        modelErrors.push({
          code: err.code as unknown as ErrorCode,
          details: err.message ?? undefined,
        });
      }
    }
    const cachedErrors: CachedErrorDetails = { varErrors, unitErrors, simError, modelErrors };
    if (this.disposed) {
      return cachedErrors;
    }
    this.cachedErrors = cachedErrors;
    this.notify();
    return cachedErrors;
  }

  /**
   * Annotate the project's active-model variables with their equation/unit
   * errors. Refreshes the cached errors as a side effect. Returns a new Project;
   * does not mutate `this.project`.
   */
  async updateVariableErrors(project: Project): Promise<Project> {
    const cached = await this.refreshCachedErrors();
    if (!cached) {
      return project;
    }

    const modelName = this.modelName;
    const { varErrors, unitErrors } = cached;

    if (varErrors.size > 0) {
      const model = getOrThrow(project.models, modelName);

      // If all the errors are 'just' that we have no equations, don't scream
      // "error" at the user -- they are starting from scratch on a new model
      // and don't expect it to be running yet.
      if (
        varErrors.size === model.variables.size &&
        setsEqual(new Set(varErrors.keys()), new Set(model.variables.keys()))
      ) {
        let foundOtherError = false;
        for (const [, errs] of varErrors) {
          if (errs.length !== 1 || first(errs).code !== ErrorCode.EmptyEquation) {
            foundOtherError = true;
            break;
          }
        }
        if (!foundOtherError) {
          return { ...project, hasNoEquations: true };
        }
      }

      const mutableVars = new Map(model.variables);
      for (const [ident, errs] of varErrors) {
        const variable = mutableVars.get(ident);
        if (variable) {
          mutableVars.set(ident, { ...variable, errors: errs });
        }
      }
      const updatedModel = { ...model, variables: mutableVars as ReadonlyMap<string, Variable> };
      project = { ...project, models: mapSet(project.models, modelName, updatedModel) };
    }

    if (unitErrors.size > 0) {
      const model = getOrThrow(project.models, modelName);
      const mutableVars = new Map(model.variables);
      for (const [ident, errs] of unitErrors) {
        const variable = mutableVars.get(ident);
        if (variable) {
          mutableVars.set(ident, { ...variable, unitErrors: errs });
        }
      }
      const updatedModel = { ...model, variables: mutableVars as ReadonlyMap<string, Variable> };
      project = { ...project, models: mapSet(project.models, modelName, updatedModel) };
    }

    return project;
  }

  /**
   * Annotate the active model's aux/flow/stock variables with sketch-connector
   * drift (connectors out of sync with equations; see diagram/connector-sync.ts).
   * Equation dependencies come from the engine's per-variable `getIncomingLinks`
   * (authoritative: excludes builtins/TIME, structural flow<->stock edges, and
   * dotted module-output refs), so the check resolves arrayed/apply-to-all
   * dependencies correctly. Returns a new Project; does not mutate `this.project`.
   *
   * ORDERING: this MUST run on the view that will actually be rendered, so it is
   * called by each rebuild path AFTER that path has settled on its final view --
   * NOT tail-called from updateVariableErrors. In updateProject the rendered view
   * is the live optimistic view that `preserveLiveView` swaps in, which can be
   * NEWER than the engine-serialized snapshot (e.g. a connector the user just
   * drew but that has not round-tripped yet); computing against the stale engine
   * view would flag a "missing" connector that is actually present on screen (or
   * miss a stale one). The open/undo-reopen paths have no newer live view -- the
   * engine (or queued) view IS the rendered view -- so they call this directly on
   * that project.
   *
   * Best-effort and non-fatal: any engine failure (a raced rename, a missing
   * model) leaves the project without connector annotations rather than aborting
   * the rebuild. Skipped for stdlib/macro models (not user-edited sketches).
   */
  async attachConnectorErrors(project: Project): Promise<Project> {
    const engine = this.engine;
    const modelName = this.modelName;
    if (!engine || isStdlibModel(modelName)) {
      return project;
    }
    const model = project.models.get(modelName);
    const view = model?.views[0];
    if (!model || !view || isMacroModel(model)) {
      return project;
    }

    // Only aux/flow/stock with a primary node on this view are checkable targets.
    const targetIdents: string[] = [];
    const seen = new Set<string>();
    for (const el of view.elements) {
      if (el.type !== 'aux' && el.type !== 'stock' && el.type !== 'flow') {
        continue;
      }
      const variable = model.variables.get(el.ident);
      if (!variable || variable.type === 'module' || seen.has(el.ident)) {
        continue;
      }
      seen.add(el.ident);
      targetIdents.push(el.ident);
    }
    if (targetIdents.length === 0) {
      return project;
    }

    let engineModel: EngineModelApi;
    try {
      engineModel = await engine.getModel(modelName);
    } catch {
      return project;
    }
    if (this.disposed) {
      return project;
    }

    // Fetch each target's equation dependencies. A per-variable failure (e.g. a
    // transient rename mismatch) drops only that variable from the check.
    const dependencies = new Map<string, readonly string[]>();
    const fetched = await Promise.all(
      targetIdents.map(async (ident): Promise<readonly string[] | undefined> => {
        try {
          return await engineModel.getIncomingLinks(ident);
        } catch {
          return undefined;
        }
      }),
    );
    if (this.disposed) {
      return project;
    }
    for (let i = 0; i < targetIdents.length; i++) {
      const deps = fetched[i];
      if (deps) {
        dependencies.set(targetIdents[i], deps);
      }
    }
    if (dependencies.size === 0) {
      return project;
    }

    const issuesByIdent = computeConnectorErrors({
      elements: view.elements,
      variables: model.variables,
      dependencies,
    });
    if (issuesByIdent.size === 0) {
      return project;
    }

    const mutableVars = new Map(model.variables);
    for (const [ident, issues] of issuesByIdent) {
      const variable = mutableVars.get(ident);
      if (variable) {
        mutableVars.set(ident, { ...variable, connectorErrors: issues });
      }
    }
    const updatedModel = { ...model, variables: mutableVars as ReadonlyMap<string, Variable> };
    return { ...project, models: mapSet(project.models, modelName, updatedModel) };
  }

  // --- active-model navigation ---

  /**
   * Drill into a module's child model. Pushes a stack entry capturing the
   * current (parent) selection/viewport, switches the active model, and clears
   * the rendered model's optimistic view to the child's. Returns the selection
   * the Editor should adopt (empty) or undefined when the target model is not
   * present (a guard against pushing a nonexistent model). Viewport restoration
   * is not needed on drill-in (the child keeps its own stored view).
   *
   * @param currentSelection the Editor's live selection to capture for restore
   * @param currentViewBox/currentZoom the active view's viewport to capture
   */
  drillIntoModule(
    moduleIdent: string,
    targetModelName: string,
    currentSelection: ReadonlySet<UID>,
    currentViewBox: Rect,
    currentZoom: number,
  ): NavigationOutcome {
    const project = this.project;
    if (!project || !project.models.has(targetModelName)) {
      return { restoredSelection: undefined };
    }
    const newStack = pushModule(
      this.modelStack,
      targetModelName,
      moduleIdent,
      currentSelection,
      currentViewBox,
      currentZoom,
    );
    const newModelName = currentModelName(newStack);
    this.batch(() => {
      this.modelStack = newStack;
      this.modelName = newModelName;
      this.notify();
    });
    // The error refresh for the newly active model is driven here (the active
    // model changed). Fire-and-forget: the snapshot updates when it resolves.
    void this.refreshCachedErrors();
    return { restoredSelection: new Set<UID>() };
  }

  /**
   * Navigate back one level. Restores the parent's selection (returned to the
   * Editor) and viewport (applied internally via queueViewUpdate, which now
   * resolves getView() to the just-restored model because modelName is updated
   * synchronously first). Returns undefined selection when the stack is empty.
   */
  navigateBack(): NavigationOutcome {
    if (this.modelStack.length === 0) {
      return { restoredSelection: undefined };
    }
    return this.applyNavigation(popModule(this.modelStack));
  }

  /**
   * Navigate to a breadcrumb level. Same restoration contract as navigateBack.
   * Returns undefined selection when targetLevel is out of range.
   */
  navigateToLevel(targetLevel: number): NavigationOutcome {
    if (targetLevel >= this.modelStack.length) {
      return { restoredSelection: undefined };
    }
    return this.applyNavigation(navigateToLevel(this.modelStack, targetLevel));
  }

  private applyNavigation(result: {
    newStack: readonly ModuleStackEntry[];
    restoredModelName: string;
    restoredSelection: ReadonlySet<UID>;
    restoredViewBox: Rect;
    restoredZoom: number;
  }): NavigationOutcome {
    this.batch(() => {
      this.modelStack = result.newStack;
      this.modelName = result.restoredModelName;
      this.notify();
    });
    // Restore the parent model's viewport. modelName was updated synchronously
    // above, so getView() (via this.project) resolves to the restored model --
    // no setState-callback deferral is needed. Fire-and-forget round-trip.
    const view = this.getView();
    if (view) {
      void this.queueViewUpdate({ ...view, viewBox: result.restoredViewBox, zoom: result.restoredZoom });
    }
    // The active model changed -- refresh its cached errors.
    void this.refreshCachedErrors();
    return { restoredSelection: result.restoredSelection };
  }

  // --- read accessors used by the Editor's op builders ---

  getEngine(): EngineApi | undefined {
    return this.engine;
  }

  getProject(): Project | undefined {
    return this.project;
  }

  getModel(): Model | undefined {
    const project = this.project;
    if (!project) {
      return undefined;
    }
    return project.models.get(this.modelName);
  }

  getView(): StockFlowView | undefined {
    const model = this.getModel();
    if (!model) {
      return undefined;
    }
    return model.views[0];
  }

  getModelName(): string {
    return this.modelName;
  }

  // --- error surfacing ---

  /** Forward a transient error to the host's toast UI (never presentation
   * state the controller owns). Accepts a message string or an Error. */
  private reportError(err: string | Error): void {
    if (this.disposed) {
      return;
    }
    this.config.onError(err instanceof Error ? err : new Error(err));
  }
}
