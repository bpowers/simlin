// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// ProjectController is the headless coordination layer between the Editor and
// the WASM engine. It has ZERO React and ZERO DOM dependencies (setTimeout-free
// as well: every engine call runs through one serialized executor), so the
// async coordination is unit-tested against a fake engine without jsdom.
//
// The model (docs/design-plans/2026-09-10-diagram-editing-core.md, "Controller"):
//
// - `committed` is the engine's last acknowledged project.
// - `queue` holds edit-class items in FIFO order: view edits and model-only
//   edits (the pending edits), viewport persists, undo/redo, engine queries,
//   and the initial open. An item stays at the head of the queue while it runs.
// - The RENDERED view of a model is the next view of the last pending edit
//   targeting it, else its committed view, with the live viewport for that
//   model overlaid. The snapshot's `project` is committed plus those views plus
//   the derived annotations (errors, sim series, connector drift).
// - `token` is bumped by every truncation, undo/redo landing, and reopen. An
//   edit with a next view planned under an older token is dropped as failed.
// - One executor runs every engine call. No engine reference is held across an
//   await outside an item.
// - Maintenance (save serialization, error refresh, connector dependencies, sim
//   runs) is coalesced to one pending run per kind and runs when no edit-class
//   item is queued, or after MaintenanceEditBound consecutive edit-class items
//   or MaintenanceTimeBoundMs of continuous edit work.
// - A failed view edit truncates every later view edit and undo/redo (each was
//   planned on its optimistic view), bumps the token, renders committed, and
//   reports one error naming how many later edits were discarded. Model-only
//   edits survive: they derive their payload from committed state at dequeue.
//   A failed model-only edit only reports: nothing was planned on it. A patch
//   that applied but could not be read back resyncs committed from the engine
//   before the item's fate is decided: a successful re-read means it landed; a
//   reopen of the last recorded snapshot means it failed; a failed reopen
//   latches engine-unavailable. The model and the diagram never disagree.
// - While a pending edit renames a variable, the rendered model names that
//   variable by its new ident, so every rendered element resolves to the
//   committed variable it will name.
//
// The controller never owns presentation state: transient errors go to the
// host through `onError`.

import {
  Project,
  Model,
  EquationError,
  UnitError,
  UnitErrorKind,
  SimError,
  ModelError,
  ErrorCode,
  StockFlowView,
  UID,
  Rect,
  Variable,
  projectFromJson,
  projectAttachData,
  findNonFiniteViewCoord,
  isNamedViewElement,
  stockFlowViewToJson,
} from '@simlin/core/datamodel';
import { canonicalize } from '@simlin/core/canonicalize';
import { mapSet, setsEqual, uint8ArraysEqual, type Series } from '@simlin/core/common';
import { first } from '@simlin/core/collections';
import type { JsonProjectPatch, ErrorDetail, JsonProject } from '@simlin/engine';
import { SimlinErrorKind, SimlinUnitErrorKind } from '@simlin/engine';

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
import { buildEditOps } from './view-model-sync';
import { allocateVariableName, nameCollisionError } from './variable-names';

/**
 * The maximum number of undo snapshots kept. A small buffer is intentional:
 * undo is a convenience for the last few edits, not a full revision history,
 * and each snapshot is a complete serialized protobuf.
 */
export const MaxUndoSize = 5;

/**
 * Maintenance waits while edit-class items are queued, but never for more than
 * this many consecutive items or this much continuous edit work, so a sustained
 * stream of slow edits cannot starve saving.
 */
export const MaintenanceEditBound = 5;
export const MaintenanceTimeBoundMs = 5000;

/**
 * Cached, model-scoped error derivation for the active model. The Editor reads
 * this from the snapshot to render the error panel.
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
  // The RENDERED project: committed content with each model's rendered view
  // (pending next views, live viewports) and the derived annotations.
  readonly project: Project | undefined;
  // PURELY the render-cache key the Canvas invalidates its element lookup off.
  // It advances whenever `project` is replaced and carries no server meaning --
  // the integer version the server holds is `serverVersion`. (Deriving the
  // save version from a render counter was issue #958: unsaved edits drifted it
  // past the next integer, corrupting the optimistic-concurrency check.)
  readonly projectVersion: number;
  // The last server-ACKNOWLEDGED integer version: seeded from the initial load
  // and advanced only by a successful save's returned version. This is the
  // sole source of the `currVersion` a save sends.
  readonly serverVersion: number;
  readonly status: 'ok' | 'error' | 'disabled';
  readonly cachedErrors: CachedErrorDetails;
  readonly data: ReadonlyMap<string, Series>;
  readonly modelName: string;
  readonly modelStack: readonly ModuleStackEntry[];
  // Undo/redo availability: history exists in that direction AND no edit or
  // undo/redo item is queued (an undo landing under a pending edit would drop
  // it as stale).
  readonly canUndo: boolean;
  readonly canRedo: boolean;
  // True while an undo/redo item is queued: the Canvas ignores presses, since a
  // gesture planned on the pre-undo view could not commit.
  readonly undoRedoQueued: boolean;
  // Bumped by every truncation, undo/redo landing and reopen (see the module
  // header). A gesture captures it at press time and aborts when it moves.
  readonly token: number;
  // Bumped by every undo/redo landing. Restored content can equal content a
  // panel was seeded from (a draft's edit landed and was undone before the host
  // rendered), so a panel keyed on content alone would keep text that is no
  // longer a draft.
  readonly restoreSeq: number;
  // True once the engine was lost and could not be reopened (see resync). It
  // latches: every later edit, undo/redo and query is refused quietly, nothing
  // more can be saved, and the host shows one persistent notice offering a
  // reload instead of a toast per refused edit.
  readonly engineUnavailable: boolean;
  // Monotonic counter bumped only when undo/redo resets navigation to 'main'
  // because the restored project no longer contains the viewed model. The
  // Editor watches this to clear its own selection/details/tool UI state.
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
 * A view's viewport: the pan offset and pixel size the canvas shows
 * (`StockFlowView.viewBox`) and its zoom factor (`StockFlowView.zoom`, 1.0 =
 * 100%). The pair a host carries across a remount (see
 * `ProjectControllerConfig.initialViewport` and the Editor's
 * `onViewportChange`).
 */
export interface Viewport {
  readonly viewBox: Rect;
  readonly zoom: number;
}

/** True when the viewport can be applied: every coordinate finite, zoom positive. */
export function isUsableViewport(viewport: Viewport): boolean {
  const { viewBox, zoom } = viewport;
  return (
    Number.isFinite(viewBox.x) &&
    Number.isFinite(viewBox.y) &&
    Number.isFinite(viewBox.width) &&
    Number.isFinite(viewBox.height) &&
    Number.isFinite(zoom) &&
    zoom > 0
  );
}

/**
 * Configuration injected by the host (the Editor). The two `open*` factories
 * isolate the controller from the concrete `EngineProject` static methods so
 * it can be unit-tested against a fake engine.
 */
export interface ProjectControllerConfig {
  readonly initialProjectVersion: number;
  readonly input:
    | { readonly format: 'protobuf'; readonly data: Readonly<Uint8Array> }
    | { readonly format: 'json'; readonly data: string };
  // When set, the root model's first view opens with THIS viewport in place of
  // the one stored in the project. It is the live viewport from the first
  // published snapshot on (the canvas never renders or fits the stored one) and
  // is persisted to the engine by a viewport item -- no undo entry, no save --
  // so it is on the same footing as a pan the user just made. A host that
  // remounts the Editor on new project bytes (the notebook widget on a kernel
  // push) uses this to keep the user's live pan/zoom, which a pan alone never
  // saves. Ignored when the view is absent or the viewport is unusable (a
  // non-finite coordinate, a non-positive zoom).
  readonly initialViewport?: Viewport;
  readonly openProtobuf: (data: Uint8Array) => Promise<EngineApi>;
  readonly openJson: (data: string) => Promise<EngineApi>;
  readonly save: (
    project: { format: 'protobuf'; data: Uint8Array } | { format: 'json'; data: string },
    currVersion: number,
  ) => Promise<number | undefined>;
  readonly onError: (err: Error) => void;
  // The clock the maintenance time bound reads. Tests supply a controllable
  // one; production uses Date.now.
  readonly now?: () => number;
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

// Every Units-kind error the engine emits carries one of the three real
// kinds; NotApplicable only appears on non-unit errors. Mapping it to
// 'definition' is a defensive default for a malformed detail.
function convertUnitErrorKind(kind: SimlinUnitErrorKind): UnitErrorKind {
  switch (kind) {
    case SimlinUnitErrorKind.Consistency:
      return 'consistency';
    case SimlinUnitErrorKind.Inference:
      return 'inference';
    default:
      return 'definition';
  }
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
        kind: convertUnitErrorKind(err.unitErrorKind),
        // The bare reason ("the equation computes to units 'x', but the
        // variable's specified units are 'y'"),
        // NOT `err.message`: the message is terminal-formatted with a source
        // snippet + `~~~` underline + summary line, which renders as garbage
        // in the details panel.
        details: err.details ?? undefined,
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

function cachedErrorsFor(errors: readonly ErrorDetail[], modelName: string): CachedErrorDetails {
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
        // Prefer the bare reason over the terminal-formatted message (the
        // unit-inference umbrella carries a plain-language sentence there);
        // most model errors have no details and keep the message.
        details: err.details ?? err.message ?? undefined,
      });
    }
  }
  return { varErrors, unitErrors, simError, modelErrors };
}

/**
 * Annotate `modelName`'s variables with their equation/unit errors, or flag the
 * project `hasNoEquations` when every variable's only error is an empty
 * equation: a brand-new sketch should not scream "error" at the user.
 */
function annotateErrors(project: Project, cached: CachedErrorDetails, modelName: string): Project {
  const model = project.models.get(modelName);
  if (!model) {
    return project;
  }
  const { varErrors, unitErrors } = cached;
  if (
    varErrors.size > 0 &&
    varErrors.size === model.variables.size &&
    setsEqual(new Set(varErrors.keys()), new Set(model.variables.keys())) &&
    [...varErrors.values()].every((errs) => errs.length === 1 && first(errs).code === ErrorCode.EmptyEquation)
  ) {
    return { ...project, hasNoEquations: true };
  }
  if (varErrors.size === 0 && unitErrors.size === 0) {
    return project;
  }
  const variables = new Map(model.variables);
  for (const [ident, errs] of varErrors) {
    const variable = variables.get(ident);
    if (variable) {
      variables.set(ident, { ...variable, errors: errs });
    }
  }
  for (const [ident, errs] of unitErrors) {
    const variable = variables.get(ident);
    if (variable) {
      variables.set(ident, { ...variable, unitErrors: errs });
    }
  }
  return { ...project, models: mapSet(project.models, modelName, { ...model, variables }) };
}

/**
 * Annotate the active model's aux/flow/stock variables with sketch-connector
 * drift on the RENDERED view (see connector-sync.ts). Dependencies are the
 * engine's per-variable `getIncomingLinks` for the variables the last
 * connector refresh fetched: authoritative (they exclude builtins/TIME,
 * structural flow<->stock edges and dotted module-output refs), where
 * `getLinks` would add structural edges and omit initial-equation deps.
 *
 * Targets with a fatal equation error are skipped: their AST did not parse, so
 * the engine reports no dependencies and every inbound connector would read as
 * stale. That relies on `annotateErrors` having run first. The all-empty
 * starter model sets `hasNoEquations` without annotating `errors`, so it
 * returns early instead; stdlib and macro models are not user sketches.
 */
function annotateConnectors(
  project: Project,
  modelName: string,
  committedDependencies: ReadonlyMap<string, readonly string[]> | undefined,
  renames: ReadonlyMap<string, PendingRename>,
): Project {
  if (committedDependencies === undefined || isStdlibModel(modelName) || project.hasNoEquations) {
    return project;
  }
  const model = project.models.get(modelName);
  const view = model?.views[0];
  if (!model || !view || isMacroModel(model)) {
    return project;
  }
  // The engine reports dependencies under committed idents; the rendered model
  // already names each pending rename's variable by its new ident.
  const renamed = (ident: string): string => renames.get(ident)?.ident ?? ident;
  const dependencies =
    renames.size === 0
      ? committedDependencies
      : new Map([...committedDependencies].map(([ident, deps]) => [renamed(ident), deps.map(renamed)]));
  const checked = new Map<string, readonly string[]>();
  for (const el of view.elements) {
    if (el.type !== 'aux' && el.type !== 'stock' && el.type !== 'flow') {
      continue;
    }
    const variable = model.variables.get(el.ident);
    const deps = dependencies.get(el.ident);
    if (!variable || variable.type === 'module' || deps === undefined) {
      continue;
    }
    if (variable.errors && variable.errors.length > 0) {
      continue;
    }
    checked.set(el.ident, deps);
  }
  if (checked.size === 0) {
    return project;
  }
  const issuesByIdent = computeConnectorErrors({
    elements: view.elements,
    variables: model.variables,
    dependencies: checked,
  });
  if (issuesByIdent.size === 0) {
    return project;
  }
  const variables = new Map(model.variables);
  for (const [ident, issues] of issuesByIdent) {
    const variable = variables.get(ident);
    if (variable) {
      variables.set(ident, { ...variable, connectorErrors: issues });
    }
  }
  return { ...project, models: mapSet(project.models, modelName, { ...model, variables }) };
}

/** A committed variable a pending view renames: its ident and display name after the rename. */
interface PendingRename {
  readonly ident: string;
  readonly name: string;
}

/**
 * The renames `pendingView` implies against the committed model, keyed by
 * committed ident: a named element whose uid is on the committed view under a
 * different canonical name, naming a committed variable. The same derivation as
 * buildEditOps' renames, so the rendered model shows exactly the renames the
 * queued edits will send.
 */
function pendingRenames(
  variables: ReadonlyMap<string, Variable>,
  committedView: StockFlowView,
  pendingView: StockFlowView,
): ReadonlyMap<string, PendingRename> {
  const committedIdents = new Map<UID, string>();
  for (const el of committedView.elements) {
    if (isNamedViewElement(el)) {
      committedIdents.set(el.uid, canonicalize(el.name));
    }
  }
  const renames = new Map<string, PendingRename>();
  for (const el of pendingView.elements) {
    if (!isNamedViewElement(el)) {
      continue;
    }
    const from = committedIdents.get(el.uid);
    const to = canonicalize(el.name);
    if (from === undefined || from === to || !variables.has(from) || renames.has(from)) {
      continue;
    }
    renames.set(from, { ident: to, name: el.name });
  }
  return renames;
}

/**
 * The committed variables with each pending rename applied: the variable moves
 * to its new ident, carrying its committed content and annotations. Every
 * element of a rendered view names the variable it will name once its edit
 * lands, so the canvas and the details panel resolve a renamed element to the
 * committed variable it renames. A rename onto a name another committed variable
 * keeps is not applied: buildEditOps refuses it at dequeue, and that variable
 * keeps rendering.
 */
function withRenamedVariables(
  variables: ReadonlyMap<string, Variable>,
  renames: ReadonlyMap<string, PendingRename>,
): ReadonlyMap<string, Variable> {
  const applicable = [...renames].filter(([, to]) => !variables.has(to.ident) || renames.has(to.ident));
  if (applicable.length === 0) {
    return variables;
  }
  const out = new Map(variables);
  for (const [from] of applicable) {
    out.delete(from);
  }
  for (const [from, to] of applicable) {
    out.set(to.ident, { ...variables.get(from)!, ident: to.ident, rawName: to.name });
  }
  return out;
}

const EMPTY_CACHED_ERRORS: CachedErrorDetails = {
  varErrors: new Map<string, readonly EquationError[]>(),
  unitErrors: new Map<string, readonly UnitError[]>(),
  simError: undefined,
  modelErrors: [],
};

function viewportOf(view: StockFlowView): Viewport {
  return { viewBox: view.viewBox, zoom: view.zoom };
}

function viewportsEqual(a: Viewport, b: Viewport): boolean {
  return (
    a.zoom === b.zoom &&
    a.viewBox.x === b.viewBox.x &&
    a.viewBox.y === b.viewBox.y &&
    a.viewBox.width === b.viewBox.width &&
    a.viewBox.height === b.viewBox.height
  );
}

function withViewport(view: StockFlowView, viewport: Viewport | undefined): StockFlowView {
  if (viewport === undefined || viewportsEqual(viewportOf(view), viewport)) {
    return view;
  }
  return { ...view, viewBox: viewport.viewBox, zoom: viewport.zoom };
}

/** The result of a navigation method, describing the UI consequences the
 * Editor must apply (selection restoration, panel/tool resets). Viewport
 * restoration is handled internally by the controller. */
export interface NavigationOutcome {
  // The selection to restore (drill-in clears it; back/level restore the
  // parent's). Undefined means "navigation did not happen" (e.g. drill-in
  // into a model not present in the project).
  readonly restoredSelection: ReadonlySet<UID> | undefined;
}

type MaintenanceKind = 'save' | 'errors' | 'connectors' | 'sim';
// The order pending maintenance runs in: user data first, then the annotations
// the diagram shows, then the (potentially slow) simulation.
const MAINTENANCE_ORDER: readonly MaintenanceKind[] = ['save', 'errors', 'connectors', 'sim'];

interface ItemBase {
  readonly settle: (landed: boolean) => void;
}

// An edit with a next view (a diagram edit, rename included) or without one (a
// model-only edit whose payload is derived from the committed project at
// dequeue, so an echoed field is never stale).
interface EditItem extends ItemBase {
  readonly kind: 'edit';
  readonly label: string;
  readonly modelName: string;
  readonly token: number;
  readonly baseView: StockFlowView | undefined;
  readonly nextView: StockFlowView | undefined;
  readonly buildPatch: ((committed: Project) => JsonProjectPatch) | undefined;
}

interface ViewportItem extends ItemBase {
  readonly kind: 'viewport';
  readonly modelName: string;
}

interface UndoRedoItem extends ItemBase {
  readonly kind: 'undoRedo';
  readonly direction: 'undo' | 'redo';
}

interface QueryItem extends ItemBase {
  readonly kind: 'query';
  readonly run: (engine: EngineApi) => Promise<void>;
}

interface OpenItem extends ItemBase {
  readonly kind: 'open';
}

type QueueItem = EditItem | ViewportItem | UndoRedoItem | QueryItem | OpenItem;

/**
 * Headless coordination for a single open project. Create one per mounted
 * Editor; call `dispose()` exactly once when the Editor unmounts.
 *
 * StrictMode safety: the Editor disposes the controller when its mount effect
 * cleans up and builds a fresh one on the next mount, so the controller itself
 * need not be re-armable: `disposed` latches true, queued items settle as not
 * landed, and the executor releases the engine once its running item returns.
 */
export class ProjectController {
  private readonly config: ProjectControllerConfig;
  private readonly now: () => number;

  // The live engine handle. Only the executor touches it.
  private engine: EngineApi | undefined = undefined;

  // --- committed state
  private committed: Project | undefined = undefined;
  private projectHistory: readonly Readonly<Uint8Array>[];
  private projectOffset = 0;
  private serverVersion: number;
  private token = 0;

  // --- derived-state inputs, refreshed by maintenance
  private errorDetails: readonly ErrorDetail[] = [];
  private simulatable: boolean | undefined = undefined;
  private data: ReadonlyMap<string, Series> = new Map<string, Series>();
  // model name -> (variable ident -> equation dependencies), from the engine.
  private incomingLinks = new Map<string, ReadonlyMap<string, readonly string[]>>();

  // --- live state
  private readonly viewport = new Map<string, Viewport>();
  private modelName = 'main';
  private modelStack: readonly ModuleStackEntry[] = [];
  private navResetSeq = 0;
  private restoreSeq = 0;
  private engineUnavailable = false;

  // --- the executor
  private queue: QueueItem[] = [];
  // The item whose engine calls are in flight, if any. It stays at the head of
  // the queue; dispose settles every other item.
  private runningItem: QueueItem | undefined = undefined;
  private readonly maintenance = new Set<MaintenanceKind>();
  private running = false;
  private loop: Promise<void> | undefined = undefined;
  private editStreak = 0;
  private editStreakStart = 0;
  private idleWaiters: Array<() => void> = [];

  // --- save flush (outside the executor: a host save is a network call, not
  // an engine call, and must not hold edits back)
  private inSave = false;
  private queuedSave: { format: 'protobuf'; data: Uint8Array } | { format: 'json'; data: string } | undefined =
    undefined;

  // --- lifecycle
  private disposed = false;

  // --- publication
  private snapshot: ProjectSnapshot;
  private projectVersion: number;
  private readonly listeners = new Set<() => void>();
  private batchDepth = 0;
  private snapshotDirty = false;
  private renderMemo:
    | {
        readonly inputs: readonly unknown[];
        readonly project: Project | undefined;
        readonly cachedErrors: CachedErrorDetails;
      }
    | undefined = undefined;

  constructor(config: ProjectControllerConfig) {
    this.config = config;
    this.now = config.now ?? Date.now;
    this.projectVersion = config.initialProjectVersion;
    this.serverVersion = config.initialProjectVersion;
    this.projectHistory = config.input.format === 'protobuf' ? [config.input.data] : [];
    this.snapshot = this.buildSnapshot();
  }

  // --- subscription API

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
    const { project, cachedErrors } = this.render();
    if (project !== this.snapshot?.project) {
      this.projectVersion += 1;
    }
    return {
      project,
      projectVersion: this.projectVersion,
      serverVersion: this.serverVersion,
      status: this.status(project),
      cachedErrors,
      data: this.data,
      modelName: this.modelName,
      modelStack: this.modelStack,
      canUndo: this.canUndo(),
      canRedo: this.canRedo(),
      undoRedoQueued: this.undoRedoQueued(),
      token: this.token,
      restoreSeq: this.restoreSeq,
      engineUnavailable: this.engineUnavailable,
      navResetSeq: this.navResetSeq,
    };
  }

  private status(project: Project | undefined): 'ok' | 'error' | 'disabled' {
    if (!this.engine || !project || project.hasNoEquations || this.simulatable === undefined) {
      return 'disabled';
    }
    return this.simulatable ? 'ok' : 'error';
  }

  /**
   * The rendered project and the active model's error cache, memoized on their
   * inputs so an unrelated republish (a save acknowledgment) keeps the project
   * identity and the Canvas keeps its render caches.
   */
  private render(): { project: Project | undefined; cachedErrors: CachedErrorDetails } {
    // Only pending NEXT VIEWS feed the rendered project: queuing a viewport,
    // query or model-only item must not replace the project identity.
    const pendingViews = this.queue.flatMap((item) =>
      item.kind === 'edit' && item.nextView !== undefined ? [item.modelName, item.nextView] : [],
    );
    const inputs: readonly unknown[] = [
      this.committed,
      this.errorDetails,
      this.data,
      this.incomingLinks,
      this.modelName,
      ...pendingViews,
      ...[...this.viewport.entries()].flat(),
    ];
    const memo = this.renderMemo;
    if (memo && memo.inputs.length === inputs.length && memo.inputs.every((input, i) => input === inputs[i])) {
      return memo;
    }
    const cachedErrors =
      memo && memo.inputs[1] === this.errorDetails && memo.inputs[4] === this.modelName
        ? memo.cachedErrors
        : this.errorDetails.length === 0
          ? EMPTY_CACHED_ERRORS
          : cachedErrorsFor(this.errorDetails, this.modelName);
    let project = this.committed;
    if (project !== undefined) {
      if (this.data.size > 0 && project.models.has('main')) {
        // Sim data comes from the root model, so series attach to 'main' even
        // while a child model is viewed.
        project = projectAttachData(project, this.data, 'main');
      }
      project = annotateErrors(project, cachedErrors, this.modelName);
      let models = project.models;
      let activeRenames: ReadonlyMap<string, PendingRename> = new Map();
      for (const [name, model] of project.models) {
        const committedView = model.views[0];
        if (committedView === undefined) {
          continue;
        }
        const pendingView = this.pendingViewOf(name);
        const renames =
          pendingView === undefined ? new Map() : pendingRenames(model.variables, committedView, pendingView);
        if (name === this.modelName) {
          activeRenames = renames;
        }
        const variables = withRenamedVariables(model.variables, renames);
        const rendered = withViewport(pendingView ?? committedView, this.viewport.get(name));
        if (rendered !== committedView || variables !== model.variables) {
          models = mapSet(models, name, { ...model, variables, views: [rendered, ...model.views.slice(1)] });
        }
      }
      if (models !== project.models) {
        project = { ...project, models };
      }
      project = annotateConnectors(project, this.modelName, this.incomingLinks.get(this.modelName), activeRenames);
    }
    this.renderMemo = { inputs, project, cachedErrors };
    return this.renderMemo;
  }

  /** The next view of the last pending edit targeting `modelName`. */
  private pendingViewOf(modelName: string): StockFlowView | undefined {
    for (let i = this.queue.length - 1; i >= 0; i--) {
      const item = this.queue[i];
      if (item.kind === 'edit' && item.modelName === modelName && item.nextView !== undefined) {
        return item.nextView;
      }
    }
    return undefined;
  }

  /**
   * Mark the snapshot stale and (when not inside a batch) flush a single
   * notification. Disposed controllers never notify.
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

  /** Coalesce every snapshot change made inside `fn` into one notification. */
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

  // --- undo/redo predicates

  private editsQueued(): boolean {
    return this.queue.some((item) => item.kind === 'edit' || item.kind === 'undoRedo');
  }

  private undoRedoQueued(): boolean {
    return this.queue.some((item) => item.kind === 'undoRedo');
  }

  private hasUndoHistory(): boolean {
    return this.projectHistory.length > 1 && this.projectOffset < this.projectHistory.length - 1;
  }

  private hasRedoHistory(): boolean {
    return this.projectOffset > 0;
  }

  canUndo(): boolean {
    return this.hasUndoHistory() && !this.editsQueued() && !this.engineUnavailable;
  }

  canRedo(): boolean {
    return this.hasRedoHistory() && !this.editsQueued() && !this.engineUnavailable;
  }

  // --- enqueueing

  /**
   * Open the initial project in the engine. Resolves when the open item has
   * run (successfully or not).
   */
  openInitialProject(): Promise<void> {
    return new Promise((resolve) => {
      this.push({ kind: 'open', settle: () => resolve() });
    });
  }

  /**
   * Enqueue a diagram edit: `nextView` becomes the rendered view of `modelName`
   * immediately, and the executor later applies the model ops implied by
   * (`baseView` -> `nextView`) plus the view, atomically. Resolves true when the
   * edit lands, false when it is refused, dropped, or rolled back.
   *
   * `baseView` defaults to the model's rendered view at enqueue time, which is
   * what a handler reading `getView()` in the same tick planned on. `token`
   * defaults to the current one; a gesture passes the token it captured at
   * press so a truncation or undo landing in between drops it.
   *
   * Refused quietly while an undo/redo is queued: the edit was planned on the
   * view the undo is about to replace, so it could only be dropped later, and
   * reporting that would name a failure the user did not cause.
   */
  enqueueViewEdit(edit: {
    readonly label: string;
    readonly nextView: StockFlowView;
    readonly modelName?: string;
    readonly baseView?: StockFlowView;
    readonly token?: number;
  }): Promise<boolean> {
    const modelName = edit.modelName ?? this.modelName;
    const baseView = edit.baseView ?? this.getRenderedView(modelName);
    if (this.disposed || this.engineUnavailable || baseView === undefined || this.undoRedoQueued()) {
      return Promise.resolve(false);
    }
    // Refused here rather than dropped at dequeue, so the stale view never
    // renders.
    if (edit.token !== undefined && edit.token !== this.token) {
      this.reportError(`${edit.label} discarded: the project changed while it was being made`);
      return Promise.resolve(false);
    }
    // A non-finite coordinate serializes to JSON null, which the engine's patch
    // parser rejects; it always means an upstream geometry bug, so the whole
    // edit is refused before anything renders (issue #818).
    const bad = findNonFiniteViewCoord(edit.nextView);
    if (bad !== undefined) {
      this.reportError(`internal error: refusing a view update with a non-finite coordinate (${bad})`);
      return Promise.resolve(false);
    }
    return new Promise((resolve) => {
      this.push({
        kind: 'edit',
        label: edit.label,
        modelName,
        token: edit.token ?? this.token,
        baseView,
        nextView: edit.nextView,
        buildPatch: undefined,
        settle: resolve,
      });
    });
  }

  /**
   * Enqueue a model-only edit (equation, table, module wiring, sim specs):
   * `buildPatch` runs at dequeue against the committed project, so the payload
   * echoes the committed variable rather than one read before earlier edits
   * landed. A builder that throws (the variable no longer exists) fails the
   * item like an engine error. Resolves as `enqueueViewEdit` does.
   */
  enqueueModelEdit(edit: {
    readonly label: string;
    readonly buildPatch: (committed: Project) => JsonProjectPatch;
  }): Promise<boolean> {
    if (this.disposed || this.engineUnavailable || this.committed === undefined) {
      return Promise.resolve(false);
    }
    return new Promise((resolve) => {
      this.push({
        kind: 'edit',
        label: edit.label,
        modelName: this.modelName,
        token: this.token,
        baseView: undefined,
        nextView: undefined,
        buildPatch: edit.buildPatch,
        settle: resolve,
      });
    });
  }

  /**
   * Set the live viewport of `modelName` (a settled pan/zoom, a resize, a
   * centering, a navigation restore). It renders immediately and a viewport
   * item persists it to the engine later: no history, no save. The item reads
   * the LATEST viewport of its model when it runs and patches nothing when that
   * equals the committed one, so a burst of settles while an edit runs persists
   * once.
   */
  setViewport(modelName: string, viewport: Viewport): void {
    if (this.disposed) {
      return;
    }
    if (!isUsableViewport(viewport)) {
      this.reportError('internal error: refusing a viewport with a non-finite coordinate or non-positive zoom');
      return;
    }
    this.viewport.set(modelName, { viewBox: { ...viewport.viewBox }, zoom: viewport.zoom });
    if (this.engineUnavailable) {
      // Panning stays live; there is no engine to persist it to.
      this.notify();
      return;
    }
    this.push({ kind: 'viewport', modelName, settle: () => {} });
  }

  /**
   * Enqueue an undo or redo. Refused (a no-op) while an edit or another
   * undo/redo is queued, and when there is no history in that direction.
   *
   * `afterQueuedEdits` lifts only the queued-edit refusal: the undo/redo is
   * queued behind the edits and applies to the history they leave. It is for a
   * caller that has just submitted the user's latest change (a details-panel
   * draft committed by the Undo press itself), so the undo applies to that
   * change. View edits enqueued after it are refused as usual.
   */
  undoRedo(direction: 'undo' | 'redo', options: { readonly afterQueuedEdits?: boolean } = {}): void {
    if (this.disposed || this.engineUnavailable || this.undoRedoQueued()) {
      return;
    }
    const available = options.afterQueuedEdits
      ? direction === 'undo'
        ? this.hasUndoHistory()
        : this.hasRedoHistory()
      : direction === 'undo'
        ? this.canUndo()
        : this.canRedo();
    if (!available) {
      return;
    }
    this.push({ kind: 'undoRedo', direction, settle: () => {} });
  }

  /**
   * Run a read-only engine query (LaTeX rendering, XMILE export) through the
   * executor, so it never runs concurrently with a patch or an engine swap.
   * Resolves undefined when the controller has no engine or the query throws.
   */
  query<T>(fn: (engine: EngineApi) => Promise<T>): Promise<T | undefined> {
    if (this.disposed || this.engineUnavailable) {
      return Promise.resolve(undefined);
    }
    return new Promise((resolve) => {
      let result: T | undefined;
      this.push({
        kind: 'query',
        run: async (engine) => {
          try {
            result = await fn(engine);
          } catch {
            result = undefined;
          }
        },
        settle: () => resolve(result),
      });
    });
  }

  /** Request a save of the committed state. */
  requestSave(): void {
    this.requestMaintenance('save');
  }

  /**
   * Resolves once nothing is queued, no maintenance is pending, the executor is
   * idle and no host save is in flight.
   */
  whenIdle(): Promise<void> {
    if (this.isIdle()) {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      this.idleWaiters.push(resolve);
    });
  }

  private isIdle(): boolean {
    return !this.running && this.queue.length === 0 && this.maintenance.size === 0 && !this.inSave;
  }

  private maybeResolveIdle(): void {
    if (this.isIdle() && this.idleWaiters.length > 0) {
      const waiters = this.idleWaiters;
      this.idleWaiters = [];
      for (const waiter of waiters) {
        waiter();
      }
    }
  }

  private push(item: QueueItem): void {
    if (this.disposed) {
      item.settle(false);
      return;
    }
    this.queue.push(item);
    this.notify();
    this.kick();
  }

  private requestMaintenance(...kinds: MaintenanceKind[]): void {
    if (this.disposed) {
      return;
    }
    for (const kind of kinds) {
      this.maintenance.add(kind);
    }
    this.kick();
  }

  // --- the executor

  private kick(): void {
    if (this.running || this.disposed) {
      return;
    }
    this.running = true;
    this.loop = this.runLoop();
  }

  private async runLoop(): Promise<void> {
    // Start on a microtask so an enqueue returns (and its optimistic render
    // publishes) before any engine call begins.
    await Promise.resolve();
    try {
      for (;;) {
        if (this.disposed) {
          break;
        }
        const step = this.nextStep();
        if (step === undefined) {
          break;
        }
        await step();
      }
    } finally {
      this.running = false;
      this.loop = undefined;
      if (this.disposed) {
        await this.releaseEngine();
      }
      this.maybeResolveIdle();
    }
  }

  private nextStep(): (() => Promise<void>) | undefined {
    if (this.queue.length > 0) {
      const streakExpired =
        this.editStreak >= MaintenanceEditBound || this.now() - this.editStreakStart >= MaintenanceTimeBoundMs;
      if (this.maintenance.size > 0 && this.editStreak > 0 && streakExpired) {
        const kinds = MAINTENANCE_ORDER.filter((kind) => this.maintenance.has(kind));
        this.editStreak = 0;
        return async () => {
          for (const kind of kinds) {
            await this.runMaintenance(kind);
          }
        };
      }
      if (this.editStreak === 0) {
        this.editStreakStart = this.now();
      }
      this.editStreak++;
      const item = this.queue[0];
      return () => this.runItem(item);
    }
    this.editStreak = 0;
    const kind = MAINTENANCE_ORDER.find((k) => this.maintenance.has(k));
    if (kind === undefined) {
      return undefined;
    }
    return () => this.runMaintenance(kind);
  }

  private async runItem(item: QueueItem): Promise<void> {
    let landed = false;
    this.runningItem = item;
    try {
      switch (item.kind) {
        case 'open':
          await this.runOpen();
          landed = true;
          break;
        case 'edit':
          landed = await this.runEdit(item);
          break;
        case 'viewport':
          landed = await this.runViewport(item);
          break;
        case 'undoRedo':
          landed = await this.runUndoRedo(item);
          break;
        case 'query':
          if (this.engine) {
            await item.run(this.engine);
          }
          landed = true;
          break;
      }
    } finally {
      this.runningItem = undefined;
      const index = this.queue.indexOf(item);
      if (index !== -1) {
        this.queue.splice(index, 1);
      }
      this.notify();
      item.settle(landed);
    }
  }

  private async runOpen(): Promise<void> {
    let engine: EngineApi;
    try {
      engine =
        this.config.input.format === 'json'
          ? await this.config.openJson(this.config.input.data)
          : await this.config.openProtobuf(this.config.input.data as Uint8Array);
    } catch (e: unknown) {
      this.reportError(`opening the project in the engine failed: ${getErrorDetails(e).message ?? 'Unknown error'}`);
      return;
    }
    if (this.disposed) {
      await disposeQuietly(engine);
      return;
    }
    let serialized: Uint8Array;
    let project: Project;
    try {
      serialized = await engine.serializeProtobuf();
      project = projectFromJson(JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject);
    } catch (e: unknown) {
      await disposeQuietly(engine);
      this.reportError(`opening the project failed: ${getErrorDetails(e).message ?? 'Unknown error'}`);
      return;
    }
    if (this.disposed) {
      await disposeQuietly(engine);
      return;
    }
    this.batch(() => {
      this.engine = engine;
      this.committed = project;
      this.projectHistory = [serialized];
      this.projectOffset = 0;
      const initial = this.config.initialViewport;
      if (initial !== undefined && isUsableViewport(initial) && project.models.get(this.modelName)?.views[0]) {
        this.setViewport(this.modelName, initial);
      }
      this.notify();
    });
    this.requestMaintenance('errors', 'connectors', 'sim');
  }

  private async runEdit(item: EditItem): Promise<boolean> {
    const engine = this.engine;
    const committed = this.committed;
    if (engine === undefined || committed === undefined) {
      this.fail(item, `${item.label} failed: the project is not open`);
      return false;
    }
    if (item.nextView !== undefined && item.token !== this.token) {
      // Whatever moved the token already bumped it; bumping again would abort
      // gestures planned under the current token for no reason.
      this.failFrom(item, `${item.label} discarded: the project changed while it was being made`, {
        bumpToken: false,
      });
      return false;
    }
    let patch: JsonProjectPatch;
    try {
      patch = this.patchFor(item, committed);
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error(`applyPatch error (${item.label}):`, err.code, err.message, err.details);
      this.fail(item, err.message ?? `Unknown error during ${item.label}`);
      return false;
    }
    if (this.disposed) {
      return false;
    }
    try {
      await this.rebuildCommitted(engine, true);
    } catch (e: unknown) {
      // The patch applied, but committed no longer matches the engine. The item
      // stays at the head of the queue (its next view still renders) while the
      // resync runs, so handlers keep planning on that view; its fate is decided
      // only once committed agrees with the engine again.
      const message = `reading the project back after ${item.label} failed: ${getErrorDetails(e).message ?? 'Unknown error'}`;
      const outcome = await this.resync(engine, true);
      if (outcome !== 'reread') {
        // Reopened: the patch is lost, so the edit failed, and so did every view
        // edit planned on its next view -- including those enqueued while the
        // reopen ran, which is why this fails AFTER the swap. Released or
        // disposed: the queue was already settled.
        if (outcome === 'reopened') {
          this.fail(item, message);
        }
        return false;
      }
    }
    this.requestMaintenance('save', 'errors', 'connectors', 'sim');
    return true;
  }

  private patchFor(item: EditItem, committed: Project): JsonProjectPatch {
    if (item.nextView === undefined || item.baseView === undefined) {
      return item.buildPatch!(committed);
    }
    const model = committed.models.get(item.modelName);
    if (model === undefined) {
      throw new Error(`model '${item.modelName}' does not exist`);
    }
    // An edit never persists a viewport ahead of the viewport items: the view it
    // upserts carries the committed viewport.
    const committedView = model.views[0];
    const nextView =
      committedView === undefined
        ? item.nextView
        : { ...item.nextView, viewBox: committedView.viewBox, zoom: committedView.zoom };
    return { models: [{ name: item.modelName, ops: buildEditOps(model, item.baseView, nextView) }] };
  }

  private async runViewport(item: ViewportItem): Promise<boolean> {
    const engine = this.engine;
    const view = this.committed?.models.get(item.modelName)?.views[0];
    const viewport = this.viewport.get(item.modelName);
    if (engine === undefined || view === undefined || viewport === undefined) {
      return false;
    }
    if (viewportsEqual(viewportOf(view), viewport)) {
      return true;
    }
    const patch: JsonProjectPatch = {
      models: [
        {
          name: item.modelName,
          ops: [
            {
              type: 'upsertView',
              payload: {
                index: 0,
                view: stockFlowViewToJson({ ...view, viewBox: viewport.viewBox, zoom: viewport.zoom }),
              },
            },
          ],
        },
      ],
    };
    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (viewport):', err.code, err.message, err.details);
      // The engine kept the committed viewport, so the rendered one goes back
      // to it -- unless a newer viewport was set meanwhile, whose own item
      // persists it.
      if (this.viewport.get(item.modelName) === viewport) {
        this.batch(() => {
          this.viewport.delete(item.modelName);
          this.notify();
        });
      }
      this.reportError(err.message ?? 'Unknown error during view update');
      return false;
    }
    if (this.disposed) {
      return false;
    }
    try {
      await this.rebuildCommitted(engine, false);
    } catch {
      // Nothing is reported: the re-read keeps the viewport, and a reopen loses
      // nothing but viewports (committed differs from the last recorded snapshot
      // only by persisted viewports), which it persists again. A release shows
      // the engine-unavailable notice.
      return (await this.resync(engine, false)) === 'reread';
    }
    return true;
  }

  private async runUndoRedo(item: UndoRedoItem): Promise<boolean> {
    const delta = item.direction === 'undo' ? 1 : -1;
    const offset = Math.max(0, Math.min(this.projectOffset + delta, this.projectHistory.length - 1));
    if (offset === this.projectOffset) {
      return false;
    }
    let engine: EngineApi;
    try {
      engine = await this.config.openProtobuf(this.projectHistory[offset] as Uint8Array);
    } catch (e: unknown) {
      this.reportError(`opening the project in the engine failed: ${getErrorDetails(e).message ?? 'Unknown error'}`);
      return false;
    }
    let project: Project;
    try {
      project = projectFromJson(JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject);
    } catch (e: unknown) {
      await disposeQuietly(engine);
      this.reportError(`opening the project failed: ${getErrorDetails(e).message ?? 'Unknown error'}`);
      return false;
    }
    if (this.disposed) {
      await disposeQuietly(engine);
      return false;
    }
    const previous = this.engine;
    this.engine = engine;
    if (previous !== undefined) {
      await disposeQuietly(previous);
    }
    this.batch(() => {
      this.committed = project;
      this.projectOffset = offset;
      this.token += 1;
      this.restoreSeq += 1;
      // The restored project carries its own viewports (they are part of each
      // snapshot), exactly as the engine will save them.
      this.viewport.clear();
      if (this.modelStack.length > 0 && !project.models.has(this.modelName)) {
        this.modelStack = [];
        this.modelName = 'main';
        this.navResetSeq += 1;
      }
      this.notify();
    });
    this.requestMaintenance('save', 'errors', 'connectors', 'sim');
    return true;
  }

  /**
   * Fail edit item `item`. A view edit fails from itself (see failFrom). A
   * model-only edit has no next view, so no queued edit was planned on it: its
   * failure is reported, later edits stay queued and the token does not move.
   * The one thing that can wait behind a model-only edit and depends on it is
   * an undo/redo queued with `afterQueuedEdits` (undo/redo is otherwise refused
   * while an edit is queued): it was meant to apply to the change this edit
   * carried, so it is discarded rather than applied to an older edit.
   */
  private fail(item: EditItem, message: string): void {
    if (item.nextView === undefined) {
      const index = this.queue.indexOf(item);
      const dependent = new Set<QueueItem>(
        index === -1 ? [] : this.queue.slice(index + 1).filter((i) => i.kind === 'undoRedo'),
      );
      if (dependent.size > 0) {
        this.batch(() => {
          this.queue = this.queue.filter((i) => !dependent.has(i));
          this.notify();
        });
        for (const d of dependent) {
          d.settle(false);
        }
      }
      this.reportError(message);
      return;
    }
    this.failFrom(item, message);
  }

  /**
   * Fail view edit `item`: every later view edit and undo/redo was planned on
   * its optimistic view (or would drop as stale), so they are discarded with it.
   * Model-only edits stay queued: they build their whole payload from the
   * committed project at dequeue, so an unrelated failure does not invalidate
   * them, and discarding one would silently lose the user's typed text. One
   * that targets a variable a discarded edit would have created fails on its own
   * at dequeue and reports its own error. Viewport and query items are not edits
   * and stay queued too. Bumps the token (unless the caller is dropping an item
   * whose token already moved) and reports one error.
   */
  private failFrom(item: EditItem, message: string, options: { readonly bumpToken?: boolean } = {}): void {
    const index = this.queue.indexOf(item);
    const later = index === -1 ? [] : this.queue.slice(index + 1);
    const discarded = new Set<QueueItem>(
      later.filter(
        (i) => (i.kind === 'edit' && i.nextView !== undefined) || i.kind === 'undoRedo' || i.kind === 'open',
      ),
    );
    this.queue = [...this.queue.slice(0, index + 1), ...later.filter((i) => !discarded.has(i))];
    this.batch(() => {
      if (options.bumpToken ?? true) {
        this.token += 1;
      }
      this.notify();
    });
    for (const d of discarded) {
      d.settle(false);
    }
    const suffix =
      discarded.size === 0 ? '' : ` (${discarded.size} later edit${discarded.size === 1 ? '' : 's'} discarded)`;
    this.reportError(`${message}${suffix}`);
  }

  /**
   * Bring `committed` back in line with the engine after a patch applied but
   * reading the project back failed, before the next item runs, and say how:
   *
   * - 'reread': reading the engine again succeeded. The patch is kept, and
   *   history records it when `recordHistory` (the caller's own setting: a
   *   viewport persist records nothing).
   * - 'reopened': the last recorded snapshot (the one at the history cursor) was
   *   opened in a new engine and installed. The patch is lost, as a rolled-back
   *   edit's is; the live viewports, which the snapshot may not carry, are
   *   persisted again.
   * - 'released': the reopen failed too. The engine is released and the
   *   controller latches engine-unavailable (see becomeUnavailable).
   * - 'disposed': the controller was disposed meanwhile; an engine the reopen
   *   produced is released, and the executor releases the installed one.
   */
  private async resync(
    engine: EngineApi,
    recordHistory: boolean,
  ): Promise<'reread' | 'reopened' | 'released' | 'disposed'> {
    try {
      await this.rebuildCommitted(engine, recordHistory);
      return this.disposed ? 'disposed' : 'reread';
    } catch {
      // fall through to the reopen
    }
    if (this.disposed) {
      return 'disposed';
    }
    let reopened: EngineApi;
    let project: Project;
    try {
      reopened = await this.config.openProtobuf(this.projectHistory[this.projectOffset] as Uint8Array);
      try {
        project = projectFromJson(JSON.parse(await reopened.serializeJson(undefined, true)) as JsonProject);
      } catch (e: unknown) {
        await disposeQuietly(reopened);
        throw e;
      }
    } catch (e: unknown) {
      if (this.disposed) {
        return 'disposed';
      }
      await this.becomeUnavailable(engine, e);
      return 'released';
    }
    if (this.disposed) {
      await disposeQuietly(reopened);
      return 'disposed';
    }
    this.engine = reopened;
    await disposeQuietly(engine);
    this.batch(() => {
      this.committed = project;
      this.notify();
    });
    for (const modelName of this.viewport.keys()) {
      this.push({ kind: 'viewport', modelName, settle: () => {} });
    }
    this.requestMaintenance('errors', 'connectors', 'sim');
    return 'reopened';
  }

  /**
   * Latch engine-unavailable after a resync could not reopen the project:
   * release the engine, settle every queued item but the running one as not
   * landed, drop pending maintenance, and refuse everything from here on
   * quietly. One persistent state replaces a toast per refused edit: the host
   * tells the user once that changes can no longer be saved and offers a reload.
   */
  private async becomeUnavailable(engine: EngineApi, cause: unknown): Promise<void> {
    console.error('the project could not be reloaded:', getErrorDetails(cause).message ?? 'Unknown error');
    const discarded = this.queue.filter((item) => item !== this.runningItem);
    this.batch(() => {
      this.engine = undefined;
      this.engineUnavailable = true;
      this.queue = this.queue.filter((item) => item === this.runningItem);
      this.maintenance.clear();
      this.notify();
    });
    for (const item of discarded) {
      item.settle(false);
    }
    await disposeQuietly(engine);
  }

  /** Replace `committed` with the engine's serialized state, recording history. */
  private async rebuildCommitted(engine: EngineApi, recordHistory: boolean): Promise<void> {
    const serialized = await engine.serializeProtobuf();
    // Include stdlib model definitions so the editor can display and navigate
    // into stdlib modules. The save path does NOT include them, so stdlib models
    // are never persisted.
    const project = projectFromJson(JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject);
    if (this.disposed) {
      return;
    }
    this.batch(() => {
      this.committed = project;
      const head = this.projectHistory[this.projectOffset];
      // viewBox/zoom are serialized into the protobuf, so a viewport persist
      // never records: one momentum flick would evict every real edit from the
      // small undo buffer.
      if (recordHistory && (head === undefined || !uint8ArraysEqual(serialized, head))) {
        const next = advanceProjectHistory(
          { projectHistory: this.projectHistory, projectOffset: this.projectOffset },
          serialized,
          MaxUndoSize,
        );
        this.projectHistory = next.projectHistory;
        this.projectOffset = next.projectOffset;
      }
      this.notify();
    });
  }

  // --- maintenance

  private async runMaintenance(kind: MaintenanceKind): Promise<void> {
    this.maintenance.delete(kind);
    const engine = this.engine;
    if (engine === undefined) {
      return;
    }
    try {
      switch (kind) {
        case 'save':
          await this.serializeForSave(engine);
          break;
        case 'errors':
          await this.refreshErrors(engine);
          break;
        case 'connectors':
          await this.refreshConnectors(engine);
          break;
        case 'sim':
          await this.runSim(engine);
          break;
      }
    } catch (e: unknown) {
      this.reportError(e instanceof Error ? e : new Error(String(e)));
    }
  }

  private async serializeForSave(engine: EngineApi): Promise<void> {
    const project =
      this.config.input.format === 'json'
        ? { format: 'json' as const, data: await engine.serializeJson() }
        : { format: 'protobuf' as const, data: await engine.serializeProtobuf() };
    void this.flushSave(project);
  }

  /**
   * Hand serialized bytes to the host, sending the last server-ACKNOWLEDGED
   * version (`serverVersion`) as the optimistic-concurrency check. A returned
   * version advances serverVersion; a failed save (rejection or
   * resolved-undefined) leaves it untouched so the next attempt re-sends the
   * same still-valid version.
   *
   * A save requested while one is in flight queues exactly one flush of the
   * LATEST bytes, which re-reads serverVersion when it runs. inSave is released
   * in a finally block: a thrown host save must not leave it stuck true, or
   * every later save would queue forever.
   */
  private async flushSave(
    project: { format: 'protobuf'; data: Uint8Array } | { format: 'json'; data: string },
  ): Promise<void> {
    if (this.inSave) {
      this.queuedSave = project;
      return;
    }
    this.inSave = true;
    try {
      const version = await this.config.save(project, this.serverVersion);
      if (version) {
        this.serverVersion = version;
        this.notify();
      }
    } catch (err) {
      this.reportError(err instanceof Error ? err : new Error(String(err)));
    } finally {
      this.inSave = false;
      const queued = this.queuedSave;
      this.queuedSave = undefined;
      if (queued !== undefined && !this.disposed) {
        await this.flushSave(queued);
      }
      this.maybeResolveIdle();
    }
  }

  private async refreshErrors(engine: EngineApi): Promise<void> {
    const errors = await engine.getErrors();
    const simulatable = await engine.isSimulatable();
    if (this.disposed) {
      return;
    }
    this.errorDetails = errors;
    this.simulatable = simulatable;
    this.notify();
  }

  /**
   * Fetch the active model's equation dependencies for the aux/flow/stock
   * variables on its committed view, under their committed idents (the engine
   * knows no pending rename; annotateConnectors maps them onto the rendered
   * model). Best-effort: a failing model lookup leaves the previous
   * dependencies in place, and a per-variable failure drops only that variable
   * from the check.
   */
  private async refreshConnectors(engine: EngineApi): Promise<void> {
    const modelName = this.modelName;
    const model = this.committed?.models.get(modelName);
    const view = model?.views[0];
    if (model === undefined || view === undefined || isStdlibModel(modelName) || isMacroModel(model)) {
      return;
    }
    const targets = new Set<string>();
    for (const el of view.elements) {
      const ident = el.type === 'aux' || el.type === 'stock' || el.type === 'flow' ? canonicalize(el.name) : undefined;
      if (ident !== undefined && model.variables.has(ident)) {
        targets.add(ident);
      }
    }
    let engineModel: EngineModelApi;
    try {
      engineModel = await engine.getModel(modelName);
    } catch {
      return;
    }
    const dependencies = new Map<string, readonly string[]>();
    for (const ident of targets) {
      try {
        dependencies.set(ident, await engineModel.getIncomingLinks(ident));
      } catch {
        // dropped from the check
      }
    }
    if (this.disposed) {
      return;
    }
    this.incomingLinks = new Map(this.incomingLinks).set(modelName, dependencies);
    this.notify();
  }

  /**
   * Run the main model and attach the series. Sparklines don't need
   * Loops-That-Matter analysis, and LTM compilation can blow up WASM memory on
   * dense causal graphs (World3: ~1.8M elementary circuits -> RuntimeError:
   * unreachable). A plain run is requested first; on any failure it retries
   * with LTM explicitly disabled so a future default flip cannot starve the UI
   * of sparkline data. The first failure is reported.
   */
  private async runSim(engine: EngineApi): Promise<void> {
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
        this.requestMaintenance('errors');
        return;
      }
    }
    if (this.disposed) {
      return;
    }
    const time = run.getSeries('time') ?? new Float64Array(0);
    this.data = new Map<string, Series>(
      run.varNames.map((ident) => [ident, { name: ident, time, values: run.getSeries(ident) ?? new Float64Array(0) }]),
    );
    this.notify();
    // A run can raise simulation errors (e.g. a runtime divide-by-zero).
    this.requestMaintenance('errors');
  }

  // --- engine lifecycle

  /**
   * Latch the controller disposed and release the engine once the running item
   * (if any) returns. Every other queued item settles as not landed now,
   * including one the executor had not started yet. Best-effort: a throwing
   * engine dispose must not crash the host.
   */
  async dispose(): Promise<void> {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.listeners.clear();
    this.maintenance.clear();
    this.settleQueued();
    if (this.loop !== undefined) {
      await this.loop;
    } else {
      await this.releaseEngine();
    }
    this.maybeResolveIdle();
  }

  /** Settle every queued item but the running one as not landed. */
  private settleQueued(): void {
    const settled = this.queue.filter((item) => item !== this.runningItem);
    this.queue = this.queue.filter((item) => item === this.runningItem);
    for (const item of settled) {
      item.settle(false);
    }
  }

  private async releaseEngine(): Promise<void> {
    const engine = this.engine;
    this.engine = undefined;
    if (engine !== undefined) {
      await disposeQuietly(engine);
    }
  }

  // --- navigation

  /**
   * Drill into a module's child model. Pushes a stack entry capturing the
   * parent's selection/viewport and switches the active model. Returns the
   * selection the Editor should adopt (empty), or undefined when the target
   * model is not present.
   */
  drillIntoModule(
    moduleIdent: string,
    targetModelName: string,
    currentSelection: ReadonlySet<UID>,
    currentViewBox: Rect,
    currentZoom: number,
  ): NavigationOutcome {
    if (!this.committed?.models.has(targetModelName)) {
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
    this.batch(() => {
      this.modelStack = newStack;
      this.modelName = currentModelName(newStack);
      this.notify();
    });
    this.requestMaintenance('connectors');
    return { restoredSelection: new Set<UID>() };
  }

  /** Navigate back one level, restoring the parent's selection and viewport. */
  navigateBack(): NavigationOutcome {
    if (this.modelStack.length === 0) {
      return { restoredSelection: undefined };
    }
    return this.applyNavigation(popModule(this.modelStack));
  }

  /** Navigate to a breadcrumb level. Same restoration contract as navigateBack. */
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
      // Navigation need not wait for the queue: the viewport restore renders now
      // and persists through a viewport item for the restored model.
      if (this.committed?.models.get(result.restoredModelName)?.views[0] !== undefined) {
        this.setViewport(result.restoredModelName, { viewBox: result.restoredViewBox, zoom: result.restoredZoom });
      }
      this.notify();
    });
    this.requestMaintenance('connectors');
    return { restoredSelection: result.restoredSelection };
  }

  // --- read accessors

  getProject(): Project | undefined {
    return this.snapshot.project;
  }

  getModel(): Model | undefined {
    return this.snapshot.project?.models.get(this.modelName);
  }

  getView(): StockFlowView | undefined {
    return this.getModel()?.views[0];
  }

  getModelName(): string {
    return this.modelName;
  }

  private getRenderedView(modelName: string): StockFlowView | undefined {
    return this.snapshot.project?.models.get(modelName)?.views[0];
  }

  /**
   * The idents a new or renamed element may not take in `modelName`: the
   * rendered model's variables (the committed variables, with each pending
   * rename applied, so a name a pending rename frees is free) plus every name
   * on the rendered view, which carries each pending create and rename.
   */
  usedIdents(modelName: string = this.modelName): ReadonlySet<string> {
    const model = this.snapshot.project?.models.get(modelName);
    const used = new Set<string>(model?.variables.keys() ?? []);
    for (const el of model?.views[0]?.elements ?? []) {
      if (isNamedViewElement(el)) {
        used.add(canonicalize(el.name));
      }
    }
    return used;
  }

  /** A default name for a new element of `modelName` that no variable or pending create uses. */
  newVariableName(base: string, modelName: string = this.modelName): string {
    return allocateVariableName(base, this.usedIdents(modelName));
  }

  /** The error to show when `newName` cannot name an element (see nameCollisionError). */
  nameError(newName: string, currentIdent: string | undefined, modelName: string = this.modelName): string | undefined {
    return nameCollisionError(newName, currentIdent, this.usedIdents(modelName));
  }

  // --- error surfacing

  /** Forward a transient error to the host's toast UI. */
  private reportError(err: string | Error): void {
    if (this.disposed) {
      return;
    }
    this.config.onError(err instanceof Error ? err : new Error(err));
  }
}

async function disposeQuietly(engine: EngineApi): Promise<void> {
  try {
    await engine.dispose();
  } catch {
    // ignored: the engine is being abandoned regardless
  }
}
