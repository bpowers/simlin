// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import TextField from './components/TextField';
import Autocomplete, { type AutocompleteRenderInputParams } from './components/Autocomplete';
import Snackbar from './components/Snackbar';
import { ClearIcon, EditIcon } from './components/icons';
import SpeedDial, { CloseReason, SpeedDialAction, SpeedDialIcon } from './components/SpeedDial';
import Button from './components/Button';
import { canonicalize } from '@simlin/core/canonicalize';

import { Project as EngineProject, SimlinErrorKind, SimlinUnitErrorKind } from '@simlin/engine';
import type {
  JsonProjectPatch,
  JsonModelOperation,
  JsonSimSpecs,
  JsonArrayedEquation,
  JsonProject,
} from '@simlin/engine';
import type { ErrorDetail } from '@simlin/engine';
import { stockFlowViewToJson } from './view-conversion';
import {
  Project,
  Model,
  Variable,
  UID,
  Aux,
  ViewElement,
  NamedViewElement,
  StockFlowView,
  GraphicalFunction,
  LinkViewElement,
  FlowViewElement,
  StockViewElement,
  CloudViewElement,
  viewElementType,
  EquationError,
  SimError,
  ModelError,
  ErrorCode,
  Rect,
  UnitError,
  projectFromJson,
  projectAttachData,
  isNamedViewElement,
  stockToJson,
  flowToJson,
  auxToJson,
  moduleToJson,
  type ModuleReference,
} from '@simlin/core/datamodel';
import { defined, exists, mapSet, Series, setsEqual, toInt, uint8ArraysEqual } from '@simlin/core/common';
import { first, getOrThrow, last, only } from '@simlin/core/collections';

import { AuxIcon } from './AuxIcon';
import { Toast } from './ErrorToast';
import { FlowIcon } from './FlowIcon';
import { LinkIcon } from './LinkIcon';
import { ModuleIcon } from './ModuleIcon';
import { ModelPropertiesDrawer } from './ModelPropertiesDrawer';
import { renderSvgToString } from './render-common';
import { Status } from './Status';
import { StockIcon } from './StockIcon';
import { UndoRedoBar } from './UndoRedoBar';
import { VariableDetails } from './VariableDetails';
import { ModuleDetails } from './ModuleDetails';
import { ErrorDetails } from './ErrorDetails';
import { ZoomBar } from './ZoomBar';
import { Canvas, fauxCloudTargetUid, inCreationCloudUid, inCreationUid } from './drawing/Canvas';
import { Point, searchableName } from './drawing/common';
import { UpdateCloudAndFlow } from './drawing/Flow';
import { applyGroupMovement } from './group-movement';
import { detectUndoRedo, isEditableElement } from './keyboard-shortcuts';
import { preserveLiveView } from './merge-live-view';
import { advanceProjectHistory } from './project-history';
import { type ModuleStackEntry, currentModelName, pushModule, popModule, navigateToLevel, isStdlibModel } from './module-navigation';
import { countModelInstances } from './module-details-utils';
import { BreadcrumbBar } from './BreadcrumbBar';

import styles from './Editor.module.css';

const MaxUndoSize = 5;
// These must stay in sync with --panel-width-sm and --panel-width-lg in theme.css
const SearchbarWidthSm = 359;
const SearchbarWidthLg = 480;

function convertErrorDetails(
  errors: ErrorDetail[],
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

class EditorError implements Error {
  name = 'EditorError';
  message: string;
  constructor(msg: string) {
    this.message = msg;
  }
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

interface CachedErrorDetails {
  varErrors: ReadonlyMap<string, readonly EquationError[]>;
  unitErrors: ReadonlyMap<string, readonly UnitError[]>;
  simError: SimError | undefined;
  modelErrors: readonly ModelError[];
}

interface EditorState {
  modelErrors: readonly Error[];
  activeProject: Project | undefined;
  projectHistory: readonly Readonly<Uint8Array>[];
  projectOffset: number;
  modelName: string;
  modelStack: ReadonlyArray<ModuleStackEntry>;
  dialOpen: boolean;
  dialVisible: boolean;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;
  data: ReadonlyMap<string, Series>;
  selection: ReadonlySet<UID>;
  status: 'ok' | 'error' | 'disabled';
  showDetails: 'variable' | 'errors' | undefined;
  flowStillBeingCreated: boolean;
  drawerOpen: boolean;
  projectVersion: number;
  snapshotBlob: Blob | undefined;
  variableDetailsActiveTab: number;
  cachedErrors: CachedErrorDetails;
}

// Discriminated union types for project data formats
export type ProtobufProjectData = {
  format: 'protobuf';
  data: Readonly<Uint8Array>;
};

export type JsonProjectData = {
  format: 'json';
  data: string;
};

export type ProjectData = ProtobufProjectData | JsonProjectData;

type ProtobufInputProps = {
  inputFormat: 'protobuf';
  initialProjectBinary: Readonly<Uint8Array>;
  onSave: (project: ProtobufProjectData, currVersion: number) => Promise<number | undefined>;
};

type JsonInputProps = {
  inputFormat: 'json';
  initialProjectJson: string;
  onSave: (project: JsonProjectData, currVersion: number) => Promise<number | undefined>;
};

type ProjectInputProps = ProtobufInputProps | JsonInputProps;

interface EditorPropsBase {
  initialProjectVersion: number;
  name: string; // used when saving
  embedded?: boolean;
  readOnlyMode?: boolean;
  // Optional selection callback fired after each selection change. Hosts
  // (e.g. simlin-serve's EditorHost) use this to forward selection state
  // to backend listeners; HostedWebEditor in src/app does not subscribe.
  onSelectionChanged?: (idents: string[]) => void;
  // When provided (and the editor is not read-only), the model-properties
  // drawer offers a destructive "Delete project" action that calls this.
  // Resolving means the host has navigated away; rejecting surfaces the
  // error in the confirmation dialog. Hosts without a deletable backing
  // project (the local file-backed viewer, embeds) leave this undefined.
  onDeleteProject?: () => Promise<void>;
}

export type EditorProps = EditorPropsBase & ProjectInputProps;

export class Editor extends React.PureComponent<EditorProps, EditorState> {
  engineProject?: EngineProject;
  newEngineShouldPullView = false;
  newEngineQueuedView?: StockFlowView;

  inSave = false;
  saveQueued = false;
  // Pending setTimeout(0) handle for the deferred onSelectionChanged
  // callback. Held so componentWillUnmount can cancel — without that,
  // a remount triggered by an EditorHost path swap (which keys the
  // Editor by `${path}#${loadGeneration}`) would let the prior
  // instance's callback fire against the new instance, re-introducing
  // the stale-idents-on-new-path bug.
  private selectionDeferralTimer: ReturnType<typeof setTimeout> | null = null;
  // Pending setTimeout(0) handles for componentDidMount's deferred
  // openInitialProject() and the scheduleSimRun() / scheduleSave() /
  // handleUndoRedo() dispatches. Held so componentWillUnmount can cancel
  // them — the Editor remounts on every wouter route change in src/app
  // and on every EditorHost path swap in src/simlin-serve. If a
  // componentDidMount or handleUndoRedo callback fires after unmount it
  // opens an EngineProject on a stale `this` and leaks ~several MB of WASM
  // linear memory plus salsa caches; if scheduleSimRun / scheduleSave
  // fire after unmount they touch a disposed engine and may setState on
  // an unmounted component.
  private openInitialProjectTimer: ReturnType<typeof setTimeout> | null = null;
  private simRunTimer: ReturnType<typeof setTimeout> | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private undoRedoTimer: ReturnType<typeof setTimeout> | null = null;
  // Set in componentWillUnmount before any pending callbacks could fire
  // and re-enter the instance. Each scheduled callback short-circuits
  // when this is true so a setTimeout already drained from the macrotask
  // queue at unmount time (clearTimeout no longer reaches it) does not
  // touch state, open an engine, or call into props.onSave.
  private unmounted = false;

  constructor(props: EditorProps) {
    super(props);

    this.state = {
      activeProject: undefined,
      projectHistory:
        props.inputFormat === 'protobuf'
          ? [props.initialProjectBinary]
          : [],
      projectOffset: 0,
      modelErrors: [],
      modelName: 'main',
      modelStack: [],
      dialOpen: false,
      dialVisible: true,
      selectedTool: undefined,
      data: new Map<string, Series>(),
      selection: new Set<number>(),
      status: 'disabled',
      showDetails: undefined,
      flowStillBeingCreated: false,
      drawerOpen: false,
      projectVersion: props.initialProjectVersion,
      snapshotBlob: undefined,
      variableDetailsActiveTab: 0,
      cachedErrors: {
        varErrors: new Map<string, readonly EquationError[]>(),
        unitErrors: new Map<string, readonly UnitError[]>(),
        simError: undefined,
        modelErrors: [],
      },
    };
    // The deferred initial project load is kicked off in componentDidMount,
    // not here — see the comment there. Keep this constructor side-effect
    // free (state setup only).
  }

  componentDidMount() {
    // React 18 StrictMode (dev) drives every committed component through
    // componentDidMount → componentWillUnmount → componentDidMount on the
    // *same* instance, without re-running the constructor. That shapes two
    // things here:
    //
    //  - `unmounted` must be (re)set false on every mount. componentWillUnmount
    //    sets it true; if the only place it were set false were the constructor
    //    (and the load scheduled there too), the second StrictMode mount would
    //    leave it stuck true and every scheduled callback would short-circuit.
    //  - The deferred openInitialProject() must be scheduled here, not in the
    //    constructor. componentWillUnmount cancels openInitialProjectTimer, so
    //    a constructor-scheduled timer would be cancelled by the StrictMode
    //    unmount and never rescheduled — the editor would sit on a blank canvas
    //    forever (engineProject and state.activeProject never get populated).
    //    Scheduling here also keeps a StrictMode-discarded *render-phase*
    //    instance (whose componentWillUnmount never runs, so it can't cancel
    //    anything) from leaking the timer onto a zombie `this`: such an
    //    instance never reaches componentDidMount.
    this.unmounted = false;

    if (this.props.readOnlyMode)
      this.setState({
        modelErrors: [
          ...this.state.modelErrors,
          new Error("This is a read-only version. Any changes you make won't be saved."),
        ],
      });

    document.addEventListener('keydown', this.handleKeyDown);

    // componentWillUnmount clears this handle, so a StrictMode unmount/remount
    // becomes schedule → cancel → schedule and the load still happens once.
    this.openInitialProjectTimer = setTimeout(async () => {
      this.openInitialProjectTimer = null;
      // If unmount drained this callback off the macrotask queue before
      // clearTimeout could cancel it, abort: opening an engine here would
      // wire it onto a stale `this` that componentWillUnmount cannot reach.
      if (this.unmounted) {
        return;
      }
      await this.openInitialProject();
      if (this.unmounted) {
        // openInitialProject finished against an unmounted instance — the
        // engine handle is on `this.engineProject`. Dispose it here so the
        // navigation away from this route does not strand the WASM
        // allocation (componentWillUnmount has already run).
        const orphan = this.engineProject;
        this.engineProject = undefined;
        if (orphan) {
          await this.disposeOrphanedEngine(orphan);
        }
        return;
      }
      this.scheduleSimRun();
    });
  }

  componentWillUnmount() {
    document.removeEventListener('keydown', this.handleKeyDown);
    // Set the unmount flag BEFORE cancelling timers: any callback that
    // already drained off the macrotask queue between the unmount tick
    // starting and clearTimeout reaching it must short-circuit. The
    // clearTimeout calls below are best-effort optimization on top.
    this.unmounted = true;
    if (this.selectionDeferralTimer !== null) {
      clearTimeout(this.selectionDeferralTimer);
      this.selectionDeferralTimer = null;
    }
    if (this.openInitialProjectTimer !== null) {
      clearTimeout(this.openInitialProjectTimer);
      this.openInitialProjectTimer = null;
    }
    if (this.simRunTimer !== null) {
      clearTimeout(this.simRunTimer);
      this.simRunTimer = null;
    }
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    if (this.undoRedoTimer !== null) {
      clearTimeout(this.undoRedoTimer);
      this.undoRedoTimer = null;
    }
    // Release the WASM EngineProject handle. The Editor mounts/unmounts
    // on every wouter route change in src/app and on every EditorHost
    // path swap in src/simlin-serve, so without this every navigation
    // away from a project leaks ~several MB of WASM linear memory plus
    // the engine's salsa caches. Best-effort: a throwing dispose must
    // not crash the host (see disposeOrphanedEngine).
    const engine = this.engineProject;
    this.engineProject = undefined;
    if (engine) {
      void this.disposeOrphanedEngine(engine);
    }
  }

  handleKeyDown = (e: KeyboardEvent) => {
    // Don't handle shortcuts in embedded mode or editable fields
    if (this.props.embedded || isEditableElement(e.target)) {
      return;
    }

    const action = detectUndoRedo(e);
    if (!action) {
      return;
    }

    const isEnabled = action === 'undo' ? this.isUndoEnabled() : this.isRedoEnabled();
    if (isEnabled) {
      e.preventDefault();
      this.handleUndoRedo(action);
    }
  };

  private isUndoEnabled(): boolean {
    return this.state.projectHistory.length > 1 && this.state.projectOffset < this.state.projectHistory.length - 1;
  }

  private isRedoEnabled(): boolean {
    return this.state.projectOffset > 0;
  }

  project(): Project | undefined {
    return this.state.activeProject;
  }

  engine(): EngineProject | undefined {
    return this.engineProject;
  }

  scheduleSimRun(): void {
    // Track only the most recent handle; any earlier pending timer is
    // covered by the `unmounted` short-circuit below. componentWillUnmount
    // clearTimeout()s the latest handle as a polite no-op for the
    // pending tick.
    this.simRunTimer = setTimeout(() => {
      this.simRunTimer = null;
      if (this.unmounted) {
        return;
      }
      const engine = this.engine();
      if (!engine) {
        return;
      }
      this.loadSim(engine);
    });
  }

  async loadSim(engine: EngineProject) {
    await this.recalculateStatus();

    if (!(await engine.isSimulatable())) {
      return;
    }
    // Sparklines don't need Loops-That-Matter analysis, and LTM compilation
    // can blow up WASM memory on dense causal graphs (World3: ~1.8M
    // elementary circuits → `RuntimeError: unreachable` from the allocator).
    // We ask for a plain simulation first; on any failure we retry with LTM
    // explicitly disabled so a future default flip cannot starve the UI of
    // sparkline data.  The first failure is surfaced as a warning-style
    // entry in `modelErrors` so the error panel still shows the user what
    // went wrong.
    const model = await engine.mainModel();
    let run;
    try {
      run = await model.run();
    } catch (e) {
      this.setState({
        modelErrors: [...this.state.modelErrors, e as Error],
      });
      try {
        run = await model.run({}, { analyzeLtm: false });
      } catch (e2) {
        this.setState({
          modelErrors: [...this.state.modelErrors, e2 as Error],
        });
        await this.refreshCachedErrors();
        return;
      }
    }
    const idents = run.varNames;
    const time = run.getSeries('time') ?? new Float64Array(0);
    const data = new Map<string, Series>(
      idents.map((ident) => {
        const values = run.getSeries(ident) ?? new Float64Array(0);
        return [ident, { name: ident, time, values }];
      }),
    );
    const project = defined(this.project());
    // Simulation data comes from mainModel(), so variable idents are
    // root-model-scoped. Always attach data to 'main' so root sparklines
    // stay populated even when a sim runs while viewing a child model.
    this.setState({
      activeProject: projectAttachData(project, data, 'main'),
      data,
    });
    // Refresh cached errors after simulation so the error panel reflects
    // any new simulation errors (e.g. runtime divide-by-zero).
    await this.refreshCachedErrors();
  }

  async updateProject(
    serializedProject: Readonly<Uint8Array>,
    opts: { scheduleSave?: boolean; recordHistory?: boolean } = {},
  ) {
    const { scheduleSave = true, recordHistory = true } = opts;
    if (this.state.projectHistory.length > 0) {
      const current = this.state.projectHistory[this.state.projectOffset];
      if (uint8ArraysEqual(serializedProject, current)) {
        return;
      }
    }

    const engine = this.engineProject;
    if (!engine) {
      return;
    }
    // Include stdlib model definitions so the editor can display and
    // navigate into stdlib modules. The save path does NOT pass
    // includeStdlib, so stdlib models are never persisted.
    const json = JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject;
    let activeProject = await this.updateVariableErrors(projectFromJson(json));
    if (this.state.data) {
      activeProject = projectAttachData(activeProject, this.state.data, 'main');
    }
    // Preserve the latest optimistic view for the active model. This
    // updateProject call may have raced with a newer setView() (e.g. the
    // user kept panning while our engine round-trip was in flight), so the
    // engine snapshot we just rebuilt is potentially behind. Without this,
    // the rebuilt activeProject would snap the diagram back to the engine's
    // older view, then the next engine round-trip would catch up -- producing
    // the visible "pan jumps back and forth" effect.
    activeProject = preserveLiveView(activeProject, this.state.activeProject, this.state.modelName);

    // fractionally increase the version -- the server will only send back integer versions,
    // but this will ensure we can use a simple version check in the Canvas to invalidate caches.
    const projectVersion = this.state.projectVersion + 0.01;

    if (recordHistory) {
      const nextHistory = advanceProjectHistory(
        { projectHistory: this.state.projectHistory, projectOffset: this.state.projectOffset },
        serializedProject,
        MaxUndoSize,
      );
      this.setState({
        projectHistory: nextHistory.projectHistory,
        projectOffset: nextHistory.projectOffset,
        activeProject,
        projectVersion,
      });
    } else {
      // View-only updates (pan/zoom) refresh the rendered project but must
      // not consume undo slots or move the undo cursor: viewBox/zoom are
      // serialized into the protobuf, so recording them would let a single
      // momentum flick evict every real edit from the small history buffer.
      this.setState({ activeProject, projectVersion });
    }
    if (scheduleSave) {
      this.scheduleSave();
    }
  }

  scheduleSave(): void {
    const { projectVersion } = this.state;

    // See scheduleSimRun for why we track only the latest handle.
    this.saveTimer = setTimeout(async () => {
      this.saveTimer = null;
      if (this.unmounted) {
        return;
      }
      await this.save(toInt(projectVersion));
    });
  }

  async save(currVersion: number): Promise<void> {
    if (this.inSave) {
      this.saveQueued = true;
      return;
    }

    this.inSave = true;

    let version: number | undefined;
    try {
      const engine = defined(this.engineProject);
      if (this.props.inputFormat === 'json') {
        version = await this.props.onSave({ format: 'json', data: await engine.serializeJson() }, currVersion);
      } else {
        version = await this.props.onSave({ format: 'protobuf', data: await engine.serializeProtobuf() }, currVersion);
      }
      if (version) {
        this.setState({ projectVersion: version });
      }
    } catch (err) {
      this.setState({
        modelErrors: [...this.state.modelErrors, err as Error],
      });
    } finally {
      this.inSave = false;
      if (this.saveQueued) {
        this.saveQueued = false;
        // Use the new server version when available; fall back to
        // currVersion on error so the queued edit still attempts to flush
        // rather than being silently dropped.
        await this.save(version ?? currVersion);
      }
    }
  }

  appendModelError(msg: string) {
    this.setState((prevState: EditorState) => ({
      modelErrors: [...prevState.modelErrors, new EditorError(msg)],
    }));
  }

  handleDialClick = (_event: React.MouseEvent<HTMLButtonElement>) => {
    this.setState({
      dialOpen: !this.state.dialOpen,
    });
  };

  handleDialClose = (_e: React.SyntheticEvent, reason: CloseReason) => {
    if (reason === 'mouseLeave' || reason === 'blur') {
      return;
    }
    // escapeKeyDown: close dial and clear tool
    this.setState({
      dialOpen: false,
      selectedTool: undefined,
    });
  };

  handleRename = async (oldName: string, newName: string) => {
    if (oldName === newName) {
      return;
    }

    const engine = this.engine();
    if (!engine) {
      return;
    }

    const view = defined(this.getView());
    const oldIdent = canonicalize(oldName);
    newName = newName.replace('\n', '\\n');

    const elements = view.elements.map((element: ViewElement) => {
      if (!isNamedViewElement(element)) {
        return element;
      }
      if (element.ident !== oldIdent) {
        return element;
      }

      return { ...element, name: newName };
    });

    const updatedView: StockFlowView = { ...view, elements };

    const ops: JsonModelOperation[] = [
      {
        type: 'renameVariable',
        payload: { from: oldIdent, to: canonicalize(newName) },
      },
      {
        type: 'upsertView',
        payload: { index: 0, view: stockFlowViewToJson(updatedView) },
      },
    ];

    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (rename):', err.code, err.message, err.details);
      const msg = err.message ?? 'Unknown error during rename';
      this.appendModelError(msg);
      return;
    }

    this.setState({
      flowStillBeingCreated: false,
    });
    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  handleSelection = (selection: ReadonlySet<UID>) => {
    this.setState({
      selection,
      flowStillBeingCreated: false,
      variableDetailsActiveTab: 0,
    });
    if (selection.size === 0) {
      this.setState({ showDetails: undefined });
    }
    // Defer one tick so React commits the new selection before the host
    // observes the callback. getSelectionIdents reads `this.state.selection`,
    // which is asynchronous in React 19 — calling it inline here would
    // return the prior selection. Reading inside the setTimeout closure
    // guarantees the call happens after the setState commit.
    //
    // Track the timer in an instance field so componentWillUnmount can
    // cancel it; a host that key-swaps the Editor (path change in
    // EditorHost) must not see this instance's idents land on the new
    // instance's path. A new selection in the same instance also
    // supersedes any pending deferral so the host always sees the
    // latest committed selection.
    const onSelectionChanged = this.props.onSelectionChanged;
    if (onSelectionChanged) {
      if (this.selectionDeferralTimer !== null) {
        clearTimeout(this.selectionDeferralTimer);
      }
      this.selectionDeferralTimer = setTimeout(() => {
        this.selectionDeferralTimer = null;
        onSelectionChanged(this.getSelectionIdents());
      }, 0);
    }
  };

  handleShowVariableDetails = () => {
    this.setState({ showDetails: 'variable' });
  };

  getLatexEquation = async (ident: string): Promise<string | undefined> => {
    const engine = this.engine();
    if (!engine) return undefined;
    try {
      const model = await engine.getModel(this.state.modelName);
      return (await model.getLatexEquation(ident)) ?? undefined;
    } catch {
      return undefined;
    }
  };

  handleSelectionDelete = async () => {
    const selection = this.state.selection;
    const { modelName } = this.state;
    const view = defined(this.getView());

    // this will remove the selected elements, clouds, and connectors
    let elements = view.elements.filter((element: ViewElement) => {
      const remove =
        selection.has(element.uid) ||
        (element.type === 'cloud' && selection.has(element.flowUid)) ||
        (element.type === 'link' &&
          (selection.has(element.toUid) || selection.has(element.fromUid)));
      return !remove;
    });

    // next we have to potentially make new clouds if we've deleted a stock
    let { nextUid } = view;
    const clouds: CloudViewElement[] = [];
    elements = elements.map((element: ViewElement) => {
      if (element.type !== 'flow') {
        return element;
      }
      const points = element.points.map((pt) => {
        if (!pt.attachedToUid || !selection.has(pt.attachedToUid)) {
          return pt;
        }

        const cloud: CloudViewElement = {
          type: 'cloud',
          uid: nextUid++,
          x: pt.x,
          y: pt.y,
          flowUid: element.uid,
          isZeroRadius: false,
          ident: undefined,
        };

        clouds.push(cloud);

        return { ...pt, attachedToUid: cloud.uid };
      });
      return { ...element, points };
    });
    elements = [...elements, ...clouds];

    const engine = this.engine();
    if (!engine) {
      return;
    }

    const deleteOps: JsonModelOperation[] = this.getSelectionIdents().map((ident) => ({
      type: 'deleteVariable' as const,
      payload: { ident },
    }));

    // Clear the selection now, in the same synchronous block (before any
    // await) as the view update below, so React batches them into a single
    // render: no consumer should ever observe a selection that references an
    // element the view no longer contains. (Clearing it after
    // `await this.updateView(...)` instead left a window where props.view had
    // dropped the deleted element but props.selection still pointed at it --
    // Canvas.buildSelectionMap now tolerates that, but the state transition
    // should still be atomic.) The deleteOps above were computed from the
    // pre-clear selection.
    this.setState({
      selection: new Set<number>(),
    });

    if (deleteOps.length > 0) {
      const patch: JsonProjectPatch = {
        models: [{ name: modelName, ops: deleteOps }],
      };
      try {
        await engine.applyPatch(patch, { allowErrors: true });
      } catch (e: unknown) {
        const err = getErrorDetails(e);
        console.error('applyPatch error (delete):', err.code, err.message, err.details);
        this.appendModelError(err.message ?? 'Unknown error during delete');
      }
    }

    await this.updateView({ ...view, elements, nextUid });
    this.scheduleSimRun();
  };

  handleMoveLabel = async (uid: UID, side: 'top' | 'left' | 'bottom' | 'right') => {
    const view = defined(this.getView());

    const elements = view.elements.map((element: ViewElement) => {
      if (element.uid !== uid || !isNamedViewElement(element)) {
        return element;
      }
      return { ...element, labelSide: side };
    });

    await this.updateView({ ...view, elements });
  };

  handleFlowAttach = async (
    flow: FlowViewElement,
    targetUid: number,
    cursorMoveDelta: Point,
    fauxTargetCenter: Point | undefined,
    inCreation: boolean,
    isSourceAttach?: boolean,
  ) => {
    let { selection } = this.state;
    const view = defined(this.getView());

    let isCreatingNew = false;
    let stockDetachingIdent: string | undefined;
    let stockAttachingIdent: string | undefined;
    let sourceStockIdent: string | undefined;
    let sourceStockDetachingIdent: string | undefined;
    let sourceStockAttachingIdent: string | undefined;
    let uidToDelete: number | undefined;
    let updatedCloud: ViewElement | undefined;
    const newClouds: ViewElement[] = [];

    let nextUid = view.nextUid;
    const getUid = (uid: number) => {
      for (const e of view.elements) {
        if (e.uid === uid) {
          return e;
        }
      }
      throw new Error(`unknown uid ${uid}`);
    };

    let elements = view.elements.map((element: ViewElement) => {
      if (element.uid !== flow.uid) {
        return element;
      }
      if (element.type !== 'flow') {
        return element;
      }

      if (isSourceAttach) {
        // Handle source attachment (first point)
        const oldFrom = getUid(defined(first(element.points).attachedToUid));
        let newCloud = false;
        let updateCloud = false;
        let from: StockViewElement | CloudViewElement;

        if (targetUid) {
          if (oldFrom.type === 'cloud') {
            uidToDelete = oldFrom.uid;
          }
          const newTarget = getUid(targetUid);
          if (newTarget.type !== 'stock' && newTarget.type !== 'cloud') {
            throw new Error(`new target isn't a stock or cloud (uid ${newTarget.uid})`);
          }
          from = newTarget;
        } else if (oldFrom.type === 'cloud') {
          updateCloud = true;
          from = {
            ...oldFrom,
            x: oldFrom.x - cursorMoveDelta.x,
            y: oldFrom.y - cursorMoveDelta.y,
          };
        } else {
          // Detaching from a stock - create a new cloud at the release position.
          // Use the same approach as the sink path: oldFrom.x - cursorMoveDelta.x/y
          // This ensures the cloud appears where the user released, not where they started.
          newCloud = true;
          from = {
            type: 'cloud' as const,
            uid: nextUid++,
            x: oldFrom.x - cursorMoveDelta.x,
            y: oldFrom.y - cursorMoveDelta.y,
            flowUid: flow.uid,
            isZeroRadius: false,
            ident: undefined,
          };
        }

        if (oldFrom.uid !== from.uid) {
          if (oldFrom.type === 'stock') {
            sourceStockDetachingIdent = oldFrom.ident;
          }
          if (from.type === 'stock') {
            sourceStockAttachingIdent = from.ident;
          }
        }

        const moveDelta = {
          x: oldFrom.x - from.x,
          y: oldFrom.y - from.y,
        };
        const points = element.points.map((point) => {
          if (point.attachedToUid !== oldFrom.uid) {
            return point;
          }
          return { ...point, attachedToUid: from.uid };
        });
        from = {
          ...from,
          x: oldFrom.x,
          y: oldFrom.y,
        } as StockViewElement | CloudViewElement;
        element = { ...element, points };

        [from, element] = UpdateCloudAndFlow(from, element as FlowViewElement, moveDelta);
        if (newCloud) {
          newClouds.push(from);
        } else if (updateCloud) {
          updatedCloud = from;
        }

        return element;
      }

      // Handle sink attachment (last point) - original behavior
      const oldTo = getUid(defined(last(element.points).attachedToUid));
      let newCloud = false;
      let updateCloud = false;
      let to: StockViewElement | CloudViewElement;
      if (targetUid) {
        if (oldTo.type === 'cloud') {
          uidToDelete = oldTo.uid;
        }
        const newTarget = getUid(targetUid);
        if (newTarget.type !== 'stock' && newTarget.type !== 'cloud') {
          throw new Error(`new target isn't a stock or cloud (uid ${newTarget.uid})`);
        }
        to = newTarget;
      } else if (oldTo.type === 'cloud') {
        updateCloud = true;
        to = {
          ...oldTo,
          x: oldTo.x - cursorMoveDelta.x,
          y: oldTo.y - cursorMoveDelta.y,
        };
      } else {
        newCloud = true;
        to = {
          type: 'cloud' as const,
          uid: nextUid++,
          x: oldTo.x - cursorMoveDelta.x,
          y: oldTo.y - cursorMoveDelta.y,
          flowUid: flow.uid,
          isZeroRadius: false,
          ident: undefined,
        };
      }

      if (oldTo.uid !== to.uid) {
        if (oldTo.type === 'stock') {
          stockDetachingIdent = oldTo.ident;
        }
        if (to.type === 'stock') {
          stockAttachingIdent = to.ident;
        }
      }

      const moveDelta = {
        x: oldTo.x - to.x,
        y: oldTo.y - to.y,
      };
      const points = element.points.map((point) => {
        if (point.attachedToUid !== oldTo.uid) {
          return point;
        }
        return { ...point, attachedToUid: to.uid };
      });
      to = {
        ...to,
        x: oldTo.x,
        y: oldTo.y,
      } as StockViewElement | CloudViewElement;
      element = { ...element, points };

      [to, element] = UpdateCloudAndFlow(to, element as FlowViewElement, moveDelta);
      if (newCloud) {
        newClouds.push(to);
      } else if (updateCloud) {
        updatedCloud = to;
      }

      return element;
    });
    // we might have updated some clouds
    elements = elements.map((element: ViewElement) => {
      if (updatedCloud && updatedCloud.uid === element.uid) {
        return updatedCloud;
      }
      return element;
    });
    // if we have something to delete, do it here
    elements = elements.filter((e) => e.uid !== uidToDelete);
    if (flow.uid === inCreationUid) {
      flow = {
        ...flow,
        uid: nextUid++,
      };
      const firstPt = first(flow.points);
      const sourceUid = firstPt.attachedToUid;
      if (sourceUid === inCreationCloudUid) {
        const newCloud: CloudViewElement = {
          type: 'cloud',
          uid: nextUid++,
          x: firstPt.x,
          y: firstPt.y,
          flowUid: flow.uid,
          isZeroRadius: false,
          ident: undefined,
        };
        elements = [...elements, newCloud];
        flow = {
          ...flow,
          points: flow.points.map((pt) => {
            if (pt.attachedToUid === inCreationCloudUid) {
              return { ...pt, attachedToUid: newCloud.uid };
            }
            return pt;
          }),
        };
      } else if (sourceUid) {
        const sourceStock = getUid(sourceUid) as StockViewElement;
        sourceStockIdent = defined(sourceStock.ident);
      }
      const lastPt = last(flow.points);
      if (lastPt.attachedToUid === fauxCloudTargetUid) {
        let newCloud = false;
        let to: StockViewElement | CloudViewElement;
        if (targetUid) {
          to = getUid(targetUid) as StockViewElement | CloudViewElement;
          stockAttachingIdent = defined(to.ident);
          cursorMoveDelta = {
            x: 0,
            y: 0,
          };
        } else {
          to = {
            type: 'cloud' as const,
            uid: nextUid++,
            x: defined(fauxTargetCenter).x,
            y: defined(fauxTargetCenter).y,
            flowUid: flow.uid,
            isZeroRadius: false,
            ident: undefined,
          };
          newCloud = true;
        }
        flow = {
          ...flow,
          points: flow.points.map((pt) => {
            if (pt.attachedToUid === fauxCloudTargetUid) {
              return { ...pt, attachedToUid: to.uid };
            }
            return pt;
          }),
        };
        [to, flow] = UpdateCloudAndFlow(to, flow, cursorMoveDelta);
        if (newCloud) {
          elements = [...elements, to];
        }
      }
      elements = [...elements, flow];
      selection = new Set([flow.uid]);
      isCreatingNew = true;
    }
    elements = [...elements, ...newClouds];

    const engine = this.engine();
    if (!engine) {
      return;
    }

    const ops: JsonModelOperation[] = [];

    if (isCreatingNew) {
      ops.push({
        type: 'upsertFlow',
        payload: {
          flow: {
            name: (flow as NamedViewElement).name,
            equation: '',
          },
        },
      });
    }

    if (sourceStockIdent) {
      const model = defined(this.getModel());
      const stockVar = model.variables.get(sourceStockIdent);
      if (stockVar?.type === 'stock') {
        ops.push({
          type: 'updateStockFlows',
          payload: {
            ident: stockVar.ident,
            inflows: [...stockVar.inflows],
            outflows: [...stockVar.outflows, flow.ident],
          },
        });
      }
    }

    // Handle source stock attaching (outflows)
    if (sourceStockAttachingIdent) {
      const model = defined(this.getModel());
      const stockVar = model.variables.get(sourceStockAttachingIdent);
      if (stockVar?.type === 'stock') {
        ops.push({
          type: 'updateStockFlows',
          payload: {
            ident: stockVar.ident,
            inflows: [...stockVar.inflows],
            outflows: [...stockVar.outflows, flow.ident],
          },
        });
      }
    }

    // Handle source stock detaching (outflows)
    if (sourceStockDetachingIdent) {
      const model = defined(this.getModel());
      const stockVar = model.variables.get(sourceStockDetachingIdent);
      if (stockVar?.type === 'stock') {
        ops.push({
          type: 'updateStockFlows',
          payload: {
            ident: stockVar.ident,
            inflows: [...stockVar.inflows],
            outflows: stockVar.outflows.filter((f) => f !== flow.ident),
          },
        });
      }
    }

    // Handle sink stock attaching (inflows)
    if (stockAttachingIdent) {
      const model = defined(this.getModel());
      const stockVar = model.variables.get(stockAttachingIdent);
      if (stockVar?.type === 'stock') {
        ops.push({
          type: 'updateStockFlows',
          payload: {
            ident: stockVar.ident,
            inflows: [...stockVar.inflows, flow.ident],
            outflows: [...stockVar.outflows],
          },
        });
      }
    }

    // Handle sink stock detaching (inflows)
    if (stockDetachingIdent) {
      const model = defined(this.getModel());
      const stockVar = model.variables.get(stockDetachingIdent);
      if (stockVar?.type === 'stock') {
        ops.push({
          type: 'updateStockFlows',
          payload: {
            ident: stockVar.ident,
            inflows: stockVar.inflows.filter((f) => f !== flow.ident),
            outflows: [...stockVar.outflows],
          },
        });
      }
    }

    if (ops.length > 0) {
      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops }],
      };
      try {
        await engine.applyPatch(patch, { allowErrors: true });
      } catch (e: unknown) {
        const err = getErrorDetails(e);
        console.error('applyPatch error (flow attach):', err.code, err.message, err.details);
        this.appendModelError(err.message ?? 'Unknown error during flow attach');
        this.setState({ selection, flowStillBeingCreated: inCreation });
        return;
      }
    }

    await this.updateView({ ...view, nextUid, elements });
    this.setState({
      selection,
      flowStillBeingCreated: inCreation,
    });
    this.scheduleSimRun();
  };

  handleLinkAttach = async (link: LinkViewElement, newTarget: string) => {
    let { selection } = this.state;
    let view = defined(this.getView());

    const getName = (ident: string) => {
      for (const e of view.elements) {
        if (isNamedViewElement(e) && e.ident === ident) {
          return e;
        }
      }
      throw new Error(`unknown name ${ident}`);
    };

    let nextUid = view.nextUid;
    let elements: ViewElement[];
    if (link.uid === inCreationUid) {
      const to = getName(newTarget);
      const newLink: LinkViewElement = {
        ...link,
        uid: nextUid++,
        toUid: to.uid,
      };
      elements = [...view.elements, newLink];
      selection = new Set([newLink.uid]);
    } else {
      // Reattachment: Canvas already computed the correct arc in
      // link.arc, so we just update the target.
      const to = getName(defined(newTarget));
      elements = view.elements.map((element: ViewElement) => {
        if (element.uid !== link.uid || element.type !== 'link') {
          return element;
        }
        return { ...element, arc: link.arc, toUid: to.uid };
      });
    }
    view = { ...view, nextUid, elements };

    await this.updateView(view);
    this.setState({ selection });
  };

  async updateView(view: StockFlowView) {
    // Optimistic update: reflect the new view in state immediately so the
    // UI never flashes stale positions while the async engine round-trip
    // (applyPatch + serialize) completes.
    this.setView(view);
    this.setState({ projectVersion: this.state.projectVersion + 0.001 });

    const engine = this.engine();
    if (engine) {
      const ops: JsonModelOperation[] = [
        {
          type: 'upsertView',
          payload: { index: 0, view: stockFlowViewToJson(view) },
        },
      ];
      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops }],
      };
      try {
        await engine.applyPatch(patch, { allowErrors: true });
      } catch (e: unknown) {
        const err = getErrorDetails(e);
        console.error('applyPatch error (view update):', err.code, err.message, err.details);
        const msg = err.message ?? 'Unknown error during view update';
        this.appendModelError(msg);
        return;
      }
      await this.updateProject(await engine.serializeProtobuf());
    }
  }

  handleCreateVariable = async (element: ViewElement) => {
    const view = defined(this.getView());
    const engine = this.engine();
    if (!engine) {
      return;
    }

    let nextUid = view.nextUid;
    const elements = [...view.elements, { ...element, uid: nextUid++ }];
    const elementType = viewElementType(element);
    const name = (element as NamedViewElement).name;

    let op: JsonModelOperation;
    if (elementType === 'stock') {
      op = {
        type: 'upsertStock',
        payload: {
          stock: {
            name,
            inflows: [],
            outflows: [],
            initialEquation: '',
          },
        },
      };
    } else if (elementType === 'flow') {
      op = {
        type: 'upsertFlow',
        payload: {
          flow: {
            name,
            equation: '',
          },
        },
      };
    } else if (elementType === 'module') {
      op = {
        type: 'upsertModule',
        payload: {
          module: {
            name,
            modelName: '',
            references: [],
          },
        },
      };
    } else {
      op = {
        type: 'upsertAux',
        payload: {
          aux: {
            name,
            equation: '',
          },
        },
      };
    }

    // AC5.2: patch targets this.state.modelName (not a hardcoded value), so
    // module creation works at any nesting depth -- navigating into a child
    // model updates modelName, and newly created modules land in that child.
    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops: [op] }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (variable creation):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during variable creation');
    }

    await this.updateView({ ...view, nextUid, elements });
    this.setState({
      selection: new Set<number>(),
    });
  };

  handleSelectionMove = async (delta: Point, arcPoint?: Point, segmentIndex?: number) => {
    const view = defined(this.getView());
    const selection = this.state.selection;

    const { updatedElements } = applyGroupMovement({
      elements: view.elements,
      selection,
      delta,
      arcPoint,
      segmentIndex,
    });

    const elements = view.elements.map((el) => updatedElements.get(el.uid) ?? el);
    await this.updateView({ ...view, elements });
  };

  handleDrawerToggle = (isOpen: boolean) => {
    this.setState({
      drawerOpen: isOpen,
    });
  };

  async applySimSpecChange(updates: Partial<JsonSimSpecs>) {
    const engine = this.engine();
    if (!engine) {
      return;
    }

    const project = this.project();
    if (!project) {
      return;
    }

    const simSpec = project.simSpecs;
    const dt = simSpec.dt.isReciprocal ? `1/${simSpec.dt.value}` : `${simSpec.dt.value}`;

    // Convert saveStep Dt to the actual numeric step size
    let saveStep: number | undefined;
    if (simSpec.saveStep) {
      saveStep = simSpec.saveStep.isReciprocal ? 1 / simSpec.saveStep.value : simSpec.saveStep.value;
    }

    const simSpecs: JsonSimSpecs = {
      startTime: updates.startTime ?? simSpec.start,
      endTime: updates.endTime ?? simSpec.stop,
      dt: updates.dt ?? dt,
      timeUnits: updates.timeUnits ?? simSpec.timeUnits,
      saveStep: updates.saveStep ?? saveStep,
      method: updates.method ?? simSpec.simMethod,
    };

    const patch: JsonProjectPatch = {
      projectOps: [
        {
          type: 'setSimSpecs',
          payload: { simSpecs: simSpecs },
        },
      ],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (sim specs):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error updating sim specs');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  }

  handleStartTimeChange = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    await this.applySimSpecChange({ startTime: value });
  };

  handleStopTimeChange = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    await this.applySimSpecChange({ endTime: value });
  };

  handleDtChange = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    await this.applySimSpecChange({ dt: `${value}` });
  };

  handleTimeUnitsChange = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = event.target.value;
    await this.applySimSpecChange({ timeUnits: value });
  };

  handleDownloadXmile = async () => {
    const engine = this.engine();
    if (!engine) {
      return;
    }
    try {
      const xmile = await engine.toXmileString();
      const encoder = new TextEncoder();
      const xmileBytes = encoder.encode(xmile);
      const blob = new Blob([xmileBytes], {
        type: 'application/octet-stream',
      });
      const url = window.URL.createObjectURL(blob);
      const a = document.createElement('a');
      document.body.appendChild(a);
      try {
        a.style.display = 'none';
      } catch {
        // oh well
      }
      a.href = url;
      a.download = `${this.props.name}-${this.state.projectVersion | 0}.stmx`;
      a.click();
      window.URL.revokeObjectURL(url);
    } catch (err: unknown) {
      const details = getErrorDetails(err);
      if (details.message) {
        this.appendModelError(details.message);
      }
    }
  };

  getDrawer() {
    const project = this.project();
    if (!project || this.props.embedded) {
      return;
    }

    const model = project.models.get(this.state.modelName);
    if (!model) {
      return;
    }

    const simSpec = project.simSpecs;
    const dt = simSpec.dt.isReciprocal ? 1 / simSpec.dt.value : simSpec.dt.value;

    // A read-only viewer should never see a delete affordance even if a host
    // wired the callback.
    const onDelete = !this.props.readOnlyMode ? this.props.onDeleteProject : undefined;

    return (
      <ModelPropertiesDrawer
        modelName={project.name}
        open={this.state.drawerOpen}
        onDrawerToggle={this.handleDrawerToggle}
        startTime={simSpec.start}
        stopTime={simSpec.stop}
        dt={dt}
        timeUnits={simSpec.timeUnits || ''}
        onStartTimeChange={this.handleStartTimeChange}
        onStopTimeChange={this.handleStopTimeChange}
        onDtChange={this.handleDtChange}
        onTimeUnitsChange={this.handleTimeUnitsChange}
        onDownloadXmile={this.handleDownloadXmile}
        onDelete={onDelete}
      />
    );
  }

  getModel(): Model | undefined {
    const project = this.project();
    if (!project) {
      return;
    }
    const modelName = this.state.modelName;
    return project.models.get(modelName);
  }

  getView(): StockFlowView | undefined {
    const project = this.project();
    if (!project) {
      return;
    }
    const modelName = this.state.modelName;
    const model = project.models.get(modelName);
    if (!model) {
      return;
    }

    return model.views[0];
  }

  setView(view: StockFlowView): void {
    const project = defined(this.project());
    const model = defined(project.models.get(this.state.modelName));
    const views = [...model.views];
    views[0] = view;
    const updatedModel = { ...model, views };
    const activeProject = { ...project, models: mapSet(project.models, this.state.modelName, updatedModel) };
    this.setState({ activeProject });
  }

  async queueViewUpdate(view: StockFlowView): Promise<void> {
    // Optimistic update: same pattern as updateView — reflect the new
    // viewBox/zoom in state immediately to avoid a frame of flicker.
    this.setView(view);
    this.setState({ projectVersion: this.state.projectVersion + 0.001 });

    const engine = this.engine();
    if (engine) {
      const ops: JsonModelOperation[] = [
        {
          type: 'upsertView',
          payload: { index: 0, view: stockFlowViewToJson(view) },
        },
      ];
      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops }],
      };
      try {
        await engine.applyPatch(patch, { allowErrors: true });
      } catch (e: unknown) {
        const err = getErrorDetails(e);
        console.error('applyPatch error (queue view update):', err.code, err.message, err.details);
        const msg = err.message ?? 'Unknown error during view update';
        this.appendModelError(msg);
        return;
      }

      await this.updateProject(await engine.serializeProtobuf(), { scheduleSave: false, recordHistory: false });
    } else {
      // there exists a race where we need to center/update the viewBox when
      // displaying a newly imported model, but the async wasm stuff doesn't
      // complete before we want to save the viewBox change.  In this case
      // set a flag we check when finalizing installation of the new engine.
      this.newEngineShouldPullView = true;
      this.newEngineQueuedView = view;
    }
  }

  handleViewBoxChange = async (viewBox: Rect, zoom: number) => {
    const view = defined(this.getView());
    await this.queueViewUpdate({ ...view, viewBox, zoom });
  };

  async centerVariable(element: ViewElement): Promise<void> {
    const view = defined(this.getView());
    const zoom = view.zoom;

    const cx = element.x;
    const cy = element.y;

    const viewCy = view.viewBox.height / 2 / zoom;
    const viewCx = (view.viewBox.width - SearchbarWidthSm) / 2 / zoom;

    const viewBox: Rect = {
      ...view.viewBox,
      x: viewCx - cx,
      y: viewCy - cy,
    };

    await this.queueViewUpdate({ ...view, viewBox });
  }

  getCanvas() {
    const project = this.project();
    if (!project) {
      return;
    }

    const { embedded } = this.props;

    const model = this.getModel();
    if (!model) {
      return;
    }

    const view = this.getView();
    if (!view) {
      return;
    }

    // Stdlib models are read-only: disable all mutation handlers while
    // keeping selection, viewbox, and drill-in navigation active.
    const readOnly = embedded || isStdlibModel(this.state.modelName);
    const onRenameVariable = !readOnly ? this.handleRename : (_oldName: string, _newName: string): void => {};
    const onSetSelection = !embedded ? this.handleSelection : (_selected: ReadonlySet<UID>): void => {};
    const onMoveSelection = !readOnly ? this.handleSelectionMove : (_position: Point): void => {};
    const onMoveFlow = !readOnly ? this.handleFlowAttach : (_e: ViewElement, _t: number, _p: Point): void => {};
    const onMoveLabel = !readOnly
      ? this.handleMoveLabel
      : (_u: UID, _s: 'top' | 'left' | 'bottom' | 'right'): void => {};
    const onAttachLink = !readOnly ? this.handleLinkAttach : (_element: ViewElement, _to: string): void => {};
    const onCreateVariable = !readOnly ? this.handleCreateVariable : (_element: ViewElement): void => {};
    const onClearSelectedTool = !readOnly ? this.handleClearSelectedTool : () => {};
    const onDeleteSelection = !readOnly ? this.handleSelectionDelete : () => {};
    const onShowVariableDetails = !readOnly ? this.handleShowVariableDetails : () => {};
    const onViewBoxChange = !embedded ? this.handleViewBoxChange : () => {};
    const onDrillIntoModule = !embedded
      ? this.handleDrillIntoModule
      : (_moduleIdent: string, _targetModelName: string): void => {};

    return (
      <Canvas
        embedded={!!embedded}
        project={project}
        model={model}
        view={view}
        version={this.state.projectVersion}
        selectedTool={readOnly ? undefined : this.state.selectedTool}
        selection={this.state.selection}
        onRenameVariable={onRenameVariable}
        onSetSelection={onSetSelection}
        onMoveSelection={onMoveSelection}
        onMoveFlow={onMoveFlow}
        onMoveLabel={onMoveLabel}
        onAttachLink={onAttachLink}
        onCreateVariable={onCreateVariable}
        onClearSelectedTool={onClearSelectedTool}
        onDeleteSelection={onDeleteSelection}
        onShowVariableDetails={onShowVariableDetails}
        onViewBoxChange={onViewBoxChange}
        onDrillIntoModule={onDrillIntoModule}
      />
    );
  }

  handleCloseSnackbar = (msg: string) => {
    this.setState((prevState) => ({
      modelErrors: prevState.modelErrors.filter((err) => err.message !== msg),
    }));
  };

  getSnackbar() {
    const { embedded } = this.props;

    if (embedded) {
      return undefined;
    }

    return (
      <Snackbar
        anchorOrigin={{
          vertical: 'bottom',
          horizontal: 'center',
        }}
        open={this.state.modelErrors.length > 0}
        autoHideDuration={6000}
      >
        <div>
          {this.state.modelErrors.map((err) => (
            <Toast
              variant="warning"
              onClose={this.handleCloseSnackbar}
              message={err.message}
              key={`${err.name}:${err.message}`}
            />
          ))}
        </div>
      </Snackbar>
    );
  }

  getSelectionIdents(): string[] {
    const names: string[] = [];
    const { selection } = this.state;
    const view = this.getView();
    if (!view) {
      return names;
    }

    for (const e of view.elements) {
      if (selection.has(e.uid) && isNamedViewElement(e)) {
        names.push(defined(e.ident));
      }
    }

    return names;
  }

  // FIXME: use a map
  getNamedSelectedElement(): ViewElement | undefined {
    if (this.state.selection.size !== 1) {
      return;
    }

    const uid = only(this.state.selection);

    const view = this.getView();
    if (!view) {
      return;
    }

    for (const e of view.elements) {
      if (e.uid === uid && isNamedViewElement(e)) {
        return e;
      }
    }

    return;
  }

  getNamedElement(ident: string): ViewElement | undefined {
    const view = this.getView();
    if (!view) {
      return;
    }

    for (const e of view.elements) {
      if (isNamedViewElement(e) && e.ident === ident) {
        return e;
      }
    }

    return;
  }

  handleShowDrawer = () => {
    this.setState({
      drawerOpen: true,
    });
  };

  handleDrillIntoModule = (moduleIdent: string, targetModelName: string): void => {
    const view = this.getView();
    if (!view) {
      return;
    }
    // Guard: don't push a nonexistent model onto the navigation stack.
    // Stdlib models are included in project.models because the editor
    // passes includeStdlib=true when calling serializeJson().
    const project = this.project();
    if (!project || !project.models.has(targetModelName)) {
      return;
    }
    const newStack = pushModule(
      this.state.modelStack,
      targetModelName,
      moduleIdent,
      this.state.selection,
      view.viewBox,
      view.zoom,
    );
    const newModelName = currentModelName(newStack);
    this.setState({
      modelStack: newStack,
      modelName: newModelName,
      selection: new Set<UID>(),
      showDetails: undefined,
      // Clear selected tool when entering a stdlib model (tool palette is hidden)
      selectedTool: isStdlibModel(newModelName) ? undefined : this.state.selectedTool,
    });
    // Refresh errors for the newly active model so warning dots and the
    // error panel reflect the child model's state immediately.
    setTimeout(() => void this.refreshCachedErrors());
  };

  handleNavigateBack = (): void => {
    const { modelStack } = this.state;
    if (modelStack.length === 0) {
      return;
    }
    const result = popModule(modelStack);
    this.setState({
      modelStack: result.newStack,
      modelName: result.restoredModelName,
      selection: result.restoredSelection,
      showDetails: undefined,
    });
    // Restore the parent model's viewport asynchronously via queueViewUpdate.
    // The parent model's view already exists in the project; we just need to
    // apply the saved viewBox and zoom back onto it.
    setTimeout(async () => {
      const view = this.getView();
      if (view) {
        await this.queueViewUpdate({ ...view, viewBox: result.restoredViewBox, zoom: result.restoredZoom });
      }
    });
    // Refresh errors for the restored model
    setTimeout(() => void this.refreshCachedErrors());
  };

  handleNavigateToLevel = (targetLevel: number): void => {
    const { modelStack } = this.state;
    if (targetLevel >= modelStack.length) {
      return;
    }
    const result = navigateToLevel(modelStack, targetLevel);
    this.setState({
      modelStack: result.newStack,
      modelName: result.restoredModelName,
      selection: result.restoredSelection,
      showDetails: undefined,
    });
    // Restore the target level's viewport asynchronously
    setTimeout(async () => {
      const view = this.getView();
      if (view) {
        await this.queueViewUpdate({ ...view, viewBox: result.restoredViewBox, zoom: result.restoredZoom });
      }
    });
    // Refresh errors for the target level's model
    setTimeout(() => void this.refreshCachedErrors());
  };

  handleSearchChange = async (_event: React.SyntheticEvent | null, newValue: string | null) => {
    if (!newValue) {
      this.handleSelection(new Set());
      return;
    }
    const element = this.getNamedElement(canonicalize(newValue));
    this.handleSelection(element ? new Set([element.uid]) : new Set());
    // Don't open the mutation-capable details panel for read-only
    // models (stdlib models, embedded mode). The Canvas-level guard
    // at line ~1480 handles double-click, but search bypasses it.
    const readOnly = this.props.embedded || isStdlibModel(this.state.modelName);
    this.setState({
      showDetails: readOnly ? undefined : 'variable',
    });
    if (element) {
      await this.centerVariable(element);
    }
  };

  handleStatusClick = () => {
    this.setState({
      showDetails: this.state.showDetails === 'errors' ? undefined : 'errors',
    });
  };

  getSearchBar() {
    const { embedded } = this.props;

    if (embedded) {
      return undefined;
    }

    let autocompleteOptions: Array<string> = [];
    const elements = this.getView()?.elements;
    if (elements) {
      autocompleteOptions = elements
        .filter((e) => isNamedViewElement(e))
        .map((e) => searchableName((e as NamedViewElement).name));
    }

    const namedElement = this.getNamedSelectedElement();
    let name;
    let placeholder: string | undefined = 'Find in Model';
    if (namedElement) {
      name = searchableName(defined((namedElement as NamedViewElement).name));
      placeholder = undefined;
    }

    const status = this.state.status;

    return (
      <div className={styles.searchBar}>
        <BreadcrumbBar
          modelStack={this.state.modelStack}
          modelName={this.state.modelName}
          onBack={this.handleNavigateBack}
          onNavigateToLevel={this.handleNavigateToLevel}
          onShowDrawer={this.handleShowDrawer}
        />
        <div className={styles.searchBox}>
          <Autocomplete
            key={name}
            value={name}
            onChange={this.handleSearchChange}
            clearOnEscape={true}
            defaultValue={name}
            options={autocompleteOptions}
            renderInput={(params: AutocompleteRenderInputParams) => {
              if (params.InputProps) {
                params.InputProps.disableUnderline = true;
              }
              return <TextField {...params} variant="standard" placeholder={placeholder} fullWidth />;
            }}
          />
        </div>
        <div className={styles.divider} />
        <Status status={status} onClick={this.handleStatusClick} />
      </div>
    );
  }

  handleClearSelected = (e: React.MouseEvent<SVGSVGElement>) => {
    e.preventDefault();
    this.handleSelection(new Set());
  };

  // Returns the equation fields for a JSON patch operation.
  // For scalar equations, returns { equation: string }.
  // For arrayed equations, returns { arrayedEquation: JsonArrayedEquation }.
  getEquationFields(variable: Variable): { equation?: string; arrayedEquation?: JsonArrayedEquation } {
    const eq = variable.type === 'module' ? undefined : variable.equation;
    if (!eq || eq.type === 'scalar') {
      return { equation: eq?.equation ?? '' };
    } else if (eq.type === 'applyToAll') {
      return {
        arrayedEquation: {
          dimensions: [...eq.dimensionNames],
          equation: eq.equation,
        },
      };
    } else if (eq.type === 'arrayed') {
      return {
        arrayedEquation: {
          dimensions: [...eq.dimensionNames],
          elements: [...eq.elements.entries()].map(([subscript, eqStr]) => ({
            subscript,
            equation: eqStr,
          })),
        },
      };
    }
    return { equation: '' };
  }

  handleEquationChange = async (
    ident: string,
    newEquation: string | undefined,
    newUnits: string | undefined,
    newDocs: string | undefined,
  ) => {
    const engine = this.engine();
    if (!engine) {
      return;
    }

    const model = this.getModel();
    if (!model) {
      return;
    }

    const variable = model.variables.get(ident);
    if (!variable) {
      return;
    }

    // When newEquation is provided, use it as a scalar equation.
    // Otherwise, preserve the existing equation structure (including arrayed equations).
    const existingEqFields = this.getEquationFields(variable);

    let op: JsonModelOperation;
    if (variable.type === 'stock') {
      // Use stockToJson to preserve all fields (including compat flags
      // like nonNegative, canBeModuleInput, isPublic), then override
      // the fields being edited.
      const base = stockToJson(variable);
      op = {
        type: 'upsertStock',
        payload: {
          stock: {
            ...base,
            initialEquation: newEquation ?? existingEqFields.equation,
            arrayedEquation: newEquation !== undefined ? undefined : existingEqFields.arrayedEquation,
            units: newUnits ?? variable.units ?? undefined,
            documentation: newDocs ?? variable.documentation ?? undefined,
          },
        },
      };
    } else if (variable.type === 'flow') {
      const base = flowToJson(variable);
      op = {
        type: 'upsertFlow',
        payload: {
          flow: {
            ...base,
            equation: newEquation ?? existingEqFields.equation,
            arrayedEquation: newEquation !== undefined ? undefined : existingEqFields.arrayedEquation,
            units: newUnits ?? variable.units ?? undefined,
            documentation: newDocs ?? variable.documentation ?? undefined,
          },
        },
      };
    } else if (variable.type === 'module') {
      // Modules have no equations or graphical functions -- only units and docs
      op = {
        type: 'upsertModule',
        payload: {
          module: {
            name: variable.ident,
            modelName: variable.modelName,
            references: variable.references.map((r) => ({ src: r.src, dst: r.dst })),
            units: newUnits ?? variable.units ?? undefined,
            documentation: newDocs ?? variable.documentation ?? undefined,
          },
        },
      };
    } else {
      const auxVar = variable as Aux;
      const base = auxToJson(auxVar);
      op = {
        type: 'upsertAux',
        payload: {
          aux: {
            ...base,
            equation: newEquation ?? existingEqFields.equation,
            arrayedEquation: newEquation !== undefined ? undefined : existingEqFields.arrayedEquation,
            units: newUnits ?? auxVar.units ?? undefined,
            documentation: newDocs ?? auxVar.documentation ?? undefined,
          },
        },
      };
    }

    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops: [op] }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (equation update):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during equation update');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  handleTableChange = async (ident: string, newTable: GraphicalFunction | null) => {
    const engine = this.engine();
    if (!engine) {
      return;
    }

    const model = this.getModel();
    if (!model) {
      return;
    }

    const variable = model.variables.get(ident);
    if (!variable) {
      return;
    }

    const gf = newTable
      ? {
          yPoints: [...newTable.yPoints],
          kind: newTable.kind,
          xScale: newTable.xScale ? { min: newTable.xScale.min, max: newTable.xScale.max } : undefined,
          yScale: newTable.yScale ? { min: newTable.yScale.min, max: newTable.yScale.max } : undefined,
        }
      : undefined;

    // Preserve the existing equation structure when updating the graphical function
    const existingEqFields = this.getEquationFields(variable);

    // Use *ToJson to preserve all fields (including compat flags),
    // then override the graphical function.
    let op: JsonModelOperation;
    if (variable.type === 'flow') {
      const base = flowToJson(variable);
      op = {
        type: 'upsertFlow',
        payload: {
          flow: {
            ...base,
            equation: existingEqFields.equation,
            arrayedEquation: existingEqFields.arrayedEquation,
            graphicalFunction: gf,
          },
        },
      };
    } else {
      const auxVar = variable as Aux;
      const base = auxToJson(auxVar);
      op = {
        type: 'upsertAux',
        payload: {
          aux: {
            ...base,
            equation: existingEqFields.equation,
            arrayedEquation: existingEqFields.arrayedEquation,
            graphicalFunction: gf,
          },
        },
      };
    }

    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops: [op] }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (table update):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during table update');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  // Updates the model reference for a module variable.
  handleModuleModelReferenceChange = async (ident: string, newModelName: string) => {
    const engine = this.engine();
    if (!engine) return;
    const model = this.getModel();
    if (!model) return;
    const variable = model.variables.get(ident);
    if (!variable || variable.type !== 'module') return;

    const op: JsonModelOperation = {
      type: 'upsertModule',
      payload: {
        module: {
          name: variable.ident,
          modelName: newModelName,
          references: variable.references.map((r) => ({ src: r.src, dst: r.dst })),
          units: variable.units || undefined,
          documentation: variable.documentation || undefined,
        },
      },
    };

    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops: [op] }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (model reference update):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during model reference update');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  // Updates units and/or documentation for a module variable.
  handleModuleUnitsDocsChange = async (ident: string, newUnits: string | undefined, newDocs: string | undefined) => {
    const engine = this.engine();
    if (!engine) return;
    const model = this.getModel();
    if (!model) return;
    const variable = model.variables.get(ident);
    if (!variable || variable.type !== 'module') return;

    const op: JsonModelOperation = {
      type: 'upsertModule',
      payload: {
        module: {
          name: variable.ident,
          modelName: variable.modelName,
          references: variable.references.map((r) => ({ src: r.src, dst: r.dst })),
          units: newUnits ?? variable.units ?? undefined,
          documentation: newDocs ?? variable.documentation ?? undefined,
        },
      },
    };

    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops: [op] }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (module units/docs update):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during module update');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  // Updates the input references array for a module variable via upsertModule.
  // The engine does full variable replacement (not merge), so we send the
  // complete module with the new references array.
  handleModuleReferencesChange = async (ident: string, newReferences: ReadonlyArray<ModuleReference>) => {
    const engine = this.engine();
    if (!engine) return;
    const model = this.getModel();
    if (!model) return;
    const variable = model.variables.get(ident);
    if (!variable || variable.type !== 'module') return;

    const op: JsonModelOperation = {
      type: 'upsertModule',
      payload: {
        module: {
          name: variable.ident,
          modelName: variable.modelName,
          references: newReferences.map((r) => ({ src: r.src, dst: r.dst })),
          units: variable.units || undefined,
          documentation: variable.documentation || undefined,
        },
      },
    };

    const patch: JsonProjectPatch = {
      models: [{ name: this.state.modelName, ops: [op] }],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (references update):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during references update');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  // Creates a new empty model and sets it as the module's reference.
  // The engine processes projectOps before model ops (see patch.rs),
  // so AddModel creates the model before upsertModule references it.
  handleCreateModelForModule = async (moduleIdent: string) => {
    const engine = this.engine();
    if (!engine) return;
    const project = this.project();
    if (!project) return;

    // Generate a unique model name to avoid collisions when the module
    // ident already matches an existing model name.
    let newModelName = moduleIdent;
    if (project.models.has(newModelName)) {
      newModelName = this.getUniqueDuplicateName(moduleIdent, project);
    }

    // Look up existing module to preserve metadata through the model reference change
    const model = this.getModel();
    const existingModule = model?.variables.get(moduleIdent);
    const modulePayload: { name: string; modelName: string; references?: { src: string; dst: string }[]; units?: string; documentation?: string } = {
      name: moduleIdent,
      modelName: newModelName,
    };
    if (existingModule && existingModule.type === 'module') {
      if (existingModule.references.length > 0) {
        modulePayload.references = existingModule.references.map((r) => ({ src: r.src, dst: r.dst }));
      }
      if (existingModule.units) modulePayload.units = existingModule.units;
      if (existingModule.documentation) modulePayload.documentation = existingModule.documentation;
    }

    const patch: JsonProjectPatch = {
      projectOps: [{ type: 'addModel', payload: { name: newModelName } }],
      models: [
        // Seed a default empty view so getCanvas() works after drilling in
        {
          name: newModelName,
          ops: [{ type: 'upsertView', payload: { index: 0, view: { elements: [] } } }],
        },
        {
          name: this.state.modelName,
          ops: [{ type: 'upsertModule', payload: { module: modulePayload } }],
        },
      ],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (create model for module):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during model creation');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  // Duplicates the source model and sets the copy as the module's reference.
  // Copies all variables and the primary view from the source model.
  handleDuplicateModelForModule = async (moduleIdent: string, sourceModelName: string) => {
    const engine = this.engine();
    if (!engine) return;
    const project = this.project();
    if (!project) return;

    const sourceModel = project.models.get(sourceModelName);
    if (!sourceModel) return;

    const newModelName = this.getUniqueDuplicateName(sourceModelName, project);

    // Build ops to copy all variables from source model
    const variableOps: JsonModelOperation[] = [];
    for (const variable of sourceModel.variables.values()) {
      if (variable.type === 'stock') {
        variableOps.push({ type: 'upsertStock', payload: { stock: stockToJson(variable) } });
      } else if (variable.type === 'flow') {
        variableOps.push({ type: 'upsertFlow', payload: { flow: flowToJson(variable) } });
      } else if (variable.type === 'aux') {
        variableOps.push({ type: 'upsertAux', payload: { aux: auxToJson(variable) } });
      } else if (variable.type === 'module') {
        variableOps.push({ type: 'upsertModule', payload: { module: moduleToJson(variable) } });
      }
    }

    // Copy the primary view, or seed an empty one so getCanvas() works
    if (sourceModel.views.length > 0) {
      variableOps.push({
        type: 'upsertView',
        payload: { index: 0, view: stockFlowViewToJson(sourceModel.views[0]) },
      });
    } else {
      variableOps.push({
        type: 'upsertView',
        payload: { index: 0, view: { elements: [] } },
      });
    }

    // Preserve existing module metadata through the model reference change
    const currentModel = this.getModel();
    const existingModule = currentModel?.variables.get(moduleIdent);
    const dupModulePayload: { name: string; modelName: string; references?: { src: string; dst: string }[]; units?: string; documentation?: string } = {
      name: moduleIdent,
      modelName: newModelName,
    };
    if (existingModule && existingModule.type === 'module') {
      if (existingModule.references.length > 0) {
        dupModulePayload.references = existingModule.references.map((r) => ({ src: r.src, dst: r.dst }));
      }
      if (existingModule.units) dupModulePayload.units = existingModule.units;
      if (existingModule.documentation) dupModulePayload.documentation = existingModule.documentation;
    }

    // Combined patch: create model, copy contents, update module reference.
    // Engine processes projectOps before model ops (patch.rs).
    const patch: JsonProjectPatch = {
      projectOps: [{ type: 'addModel', payload: { name: newModelName } }],
      models: [
        { name: newModelName, ops: variableOps },
        {
          name: this.state.modelName,
          ops: [{
            type: 'upsertModule',
            payload: { module: dupModulePayload },
          }],
        },
      ],
    };

    try {
      await engine.applyPatch(patch, { allowErrors: true });
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      console.error('applyPatch error (duplicate model):', err.code, err.message, err.details);
      this.appendModelError(err.message ?? 'Unknown error during model duplication');
      return;
    }

    await this.updateProject(await engine.serializeProtobuf());
    this.scheduleSimRun();
  };

  private getUniqueDuplicateName(baseName: string, project: Project): string {
    let name = `${baseName}_copy`;
    let i = 2;
    while (project.models.has(name)) {
      name = `${baseName}_copy_${i}`;
      i++;
    }
    return name;
  }

  getErrorDetails() {
    const { cachedErrors } = this.state;

    return (
      <div className={styles.varDetails}>
        <ErrorDetails
          status={this.state.status}
          simError={cachedErrors.simError}
          modelErrors={cachedErrors.modelErrors}
          varErrors={cachedErrors.varErrors}
          varUnitErrors={cachedErrors.unitErrors}
        />
      </div>
    );
  }

  // Shows a thin info banner when inside a module whose model is shared
  // by multiple module instances, or when viewing a stdlib model.
  getSharedModelBanner(): React.ReactNode {
    const { modelStack, modelName } = this.state;
    if (modelStack.length === 0) return undefined;

    const project = this.project();
    if (!project) return undefined;

    // AC4.4: stdlib models show read-only message
    if (isStdlibModel(modelName)) {
      return (
        <div className={styles.sharedModelBanner}>
          Standard library model (read-only)
        </div>
      );
    }

    // AC4.1, AC4.2: count instances
    const count = countModelInstances(project, modelName);

    // AC4.3: single instance shows no banner
    if (count <= 1) return undefined;

    return (
      <div className={styles.sharedModelBanner}>
        This model is used by {count} modules &mdash; changes affect all instances
      </div>
    );
  }

  getDetails() {
    const { embedded } = this.props;

    if (embedded) {
      return;
    }

    if (this.state.flowStillBeingCreated) {
      return;
    }

    if (this.state.showDetails === 'errors') {
      return this.getErrorDetails();
    }

    const namedElement = this.getNamedSelectedElement();
    if (!namedElement || this.state.showDetails !== 'variable') {
      return;
    }

    const model = defined(this.getModel());

    const ident = defined(namedElement.ident);
    const variable = getOrThrow(model.variables, ident);

    if (variable.type === 'module') {
      return (
        <div className={styles.varDetails}>
          <ModuleDetails
            key={`md-${this.state.projectVersion}-${this.state.projectOffset}-${ident}`}
            variable={variable}
            viewElement={namedElement}
            project={defined(this.project())}
            currentModelName={this.state.modelName}
            onDelete={this.handleVariableDelete}
            onModelReferenceChange={this.handleModuleModelReferenceChange}
            onUnitsDocsChange={this.handleModuleUnitsDocsChange}
            onDrillIntoModule={this.handleDrillIntoModule}
            onCreateModel={this.handleCreateModelForModule}
            onDuplicateModel={this.handleDuplicateModelForModule}
            onReferencesChange={this.handleModuleReferencesChange}
          />
        </div>
      );
    }

    const activeTab = this.state.variableDetailsActiveTab;

    return (
      <div className={styles.varDetails}>
        <VariableDetails
          key={`vd-${this.state.projectVersion}-${this.state.projectOffset}-${ident}`}
          variable={variable}
          viewElement={namedElement}
          getLatexEquation={this.getLatexEquation}
          activeTab={activeTab}
          onActiveTabChange={this.handleVariableDetailsActiveTabChange}
          onDelete={this.handleVariableDelete}
          onEquationChange={this.handleEquationChange}
          onTableChange={this.handleTableChange}
        />
      </div>
    );
  }

  handleVariableDetailsActiveTabChange = (variableDetailsActiveTab: number) => {
    this.setState({ variableDetailsActiveTab });
  };

  handleVariableDelete = (ident: string) => {
    const namedElement = this.getNamedSelectedElement();
    if (!namedElement) {
      return;
    }

    if (namedElement.ident !== ident) {
      return;
    }

    this.handleSelectionDelete();
  };

  handleClearSelectedTool = () => {
    this.setState({ selectedTool: undefined });
  };

  handleSelectStock = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    this.setState({
      selectedTool: 'stock',
    });
  };

  handleSelectFlow = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    this.setState({
      selectedTool: 'flow',
    });
  };

  handleSelectAux = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    this.setState({
      selectedTool: 'aux',
    });
  };

  handleSelectLink = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    this.setState({
      selectedTool: 'link',
    });
  };

  handleSelectModule = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    this.setState({
      selectedTool: 'module',
    });
  };

  async refreshCachedErrors(): Promise<CachedErrorDetails | undefined> {
    const engine = this.engine();
    if (!engine) {
      return undefined;
    }

    const modelName = this.state.modelName;
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
    this.setState({ cachedErrors });
    return cachedErrors;
  }

  async updateVariableErrors(project: Project): Promise<Project> {
    const cached = await this.refreshCachedErrors();
    if (!cached) {
      return project;
    }

    const modelName = this.state.modelName;
    const { varErrors, unitErrors } = cached;

    if (varErrors.size > 0) {
      const model = getOrThrow(project.models, modelName);

      // if all the errors are 'just' that we have no equations,
      // don't scream "error" at the user -- they are starting from
      // scratch on a new model and don't expect it to be running yet.
      if (varErrors.size === model.variables.size && setsEqual(new Set(varErrors.keys()), new Set(model.variables.keys()))) {
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

  // Release an engine handle that we opened but failed to wire into state,
  // so the WASM allocation doesn't leak. dispose() is best-effort: a
  // throwing dispose must not mask the original error we're surfacing.
  private async disposeOrphanedEngine(engine: EngineProject): Promise<void> {
    try {
      await engine.dispose();
    } catch {
      // ignored: the engine is being abandoned regardless
    }
  }

  async openInitialProject(): Promise<void> {
    let engine: EngineProject;
    try {
      if (this.props.inputFormat === 'json') {
        engine = await EngineProject.openJson(this.props.initialProjectJson);
      } else {
        engine = await EngineProject.openProtobuf(this.props.initialProjectBinary as Uint8Array);
      }
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      this.appendModelError(`opening the project in the engine failed: ${err.message ?? 'Unknown error'}`);
      return;
    }

    // The try/catch deliberately extends past the engine open: this method is
    // invoked from a fire-and-forget setTimeout in the constructor, and
    // src/app / src/diagram have no React error boundary. If serializeJson
    // panics in WASM or projectFromJson rejects the engine's JSON (e.g. an
    // unknown view element type), an unguarded throw becomes an unhandled
    // rejection and the user is left staring at editor chrome with a blank
    // canvas and no error message.
    try {
      this.engineProject = engine;

      const serializedProject = await engine.serializeProtobuf();

      const json = JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject;
      const project = await this.updateVariableErrors(projectFromJson(json));

      this.setState({
        projectHistory: [serializedProject],
        activeProject: project,
      });
    } catch (e: unknown) {
      this.engineProject = undefined;
      await this.disposeOrphanedEngine(engine);
      const err = getErrorDetails(e);
      this.appendModelError(`opening the project failed: ${err.message ?? 'Unknown error'}`);
    }
  }

  async openEngineProject(serializedProject: Readonly<Uint8Array>): Promise<EngineProject | undefined> {
    await this.engineProject?.dispose();
    this.engineProject = undefined;

    let engine: EngineProject;
    try {
      engine = await EngineProject.openProtobuf(serializedProject as Uint8Array);
    } catch (e: unknown) {
      const err = getErrorDetails(e);
      this.appendModelError(`opening the project in the engine failed: ${err.message ?? 'Unknown error'}`);
      return;
    }

    // See openInitialProject: the steps after a successful engine open can
    // still throw (serializeJson WASM panic, projectFromJson rejecting an
    // unknown view element type) and there is no error boundary above us.
    try {
      this.engineProject = engine;

      const json = JSON.parse(await engine.serializeJson(undefined, true)) as JsonProject;
      let project = projectFromJson(json);

      if (this.newEngineShouldPullView) {
        const queuedView = defined(this.newEngineQueuedView);
        this.newEngineShouldPullView = false;
        this.newEngineQueuedView = undefined;
        const model = defined(project.models.get(this.state.modelName));
        const views = [...model.views];
        views[0] = queuedView;
        const updatedModel = { ...model, views };
        project = { ...project, models: mapSet(project.models, this.state.modelName, updatedModel) };
        this.queueViewUpdate(queuedView);
      }

      this.setState({
        activeProject: await this.updateVariableErrors(project),
      });

      return engine;
    } catch (e: unknown) {
      this.engineProject = undefined;
      await this.disposeOrphanedEngine(engine);
      const err = getErrorDetails(e);
      this.appendModelError(`opening the project failed: ${err.message ?? 'Unknown error'}`);
      return undefined;
    }
  }

  async recalculateStatus() {
    const project = this.project();
    const engine = this.engine();

    let status: 'ok' | 'error' | 'disabled';
    if (!engine || !project || project.hasNoEquations) {
      status = 'disabled';
    } else if (!(await engine.isSimulatable())) {
      status = 'error';
    } else {
      status = 'ok';
    }

    this.setState({
      status,
    });
  }

  handleUndoRedo = (kind: 'undo' | 'redo') => {
    const delta = kind === 'undo' ? 1 : -1;
    let projectOffset = this.state.projectOffset + delta;
    // ensure our offset is always valid
    projectOffset = Math.min(projectOffset, this.state.projectHistory.length - 1);
    projectOffset = Math.max(projectOffset, 0);
    const serializedProject = defined(this.state.projectHistory[projectOffset]);
    const projectVersion = this.state.projectVersion + 0.01;
    this.setState({ projectOffset, projectVersion });

    this.undoRedoTimer = setTimeout(async () => {
      this.undoRedoTimer = null;
      if (this.unmounted) {
        return;
      }
      const engine = await this.openEngineProject(serializedProject);
      if (this.unmounted) {
        // openEngineProject opened a fresh engine onto an instance that
        // componentWillUnmount has already torn down — release it so the
        // navigation away from this route does not strand the WASM
        // allocation (componentWillUnmount cleared `this.engineProject`
        // before this callback ran).
        this.engineProject = undefined;
        if (engine) {
          await this.disposeOrphanedEngine(engine);
        }
        return;
      }
      // After undo/redo, the restored project may not contain the model
      // we were viewing (e.g. undo after creating and drilling into a new
      // submodel). Reset navigation state if the current model is gone.
      const project = this.project();
      if (project && this.state.modelStack.length > 0 && !project.models.has(this.state.modelName)) {
        this.setState({
          modelStack: [],
          modelName: 'main',
          selection: new Set<UID>(),
          showDetails: undefined,
          selectedTool: undefined,
        });
      }
      this.scheduleSimRun();
      this.scheduleSave();
    });
  };

  handleZoomChange = async (newZoom: number) => {
    const view = defined(this.getView());
    const oldViewBox = view.viewBox;

    const widthAdjust = this.state.showDetails ? SearchbarWidthLg : 0;

    const oldViewWidth = (oldViewBox.width - widthAdjust) / view.zoom;
    const oldViewHeight = oldViewBox.height / view.zoom;

    const newViewWidth = (oldViewBox.width - widthAdjust) / newZoom;
    const newViewHeight = oldViewBox.height / newZoom;

    const diffX = (newViewWidth - oldViewWidth) / 2;
    const diffY = (newViewHeight - oldViewHeight) / 2;

    const newViewBox: Rect = {
      ...oldViewBox,
      x: oldViewBox.x + diffX,
      y: oldViewBox.y + diffY,
    };
    await this.handleViewBoxChange(newViewBox, newZoom);
  };

  takeSnapshot() {
    const project = this.project();
    if (!project || !this.state.modelName) {
      return;
    }
    const { modelName } = this.state;

    const [svg, viewbox] = renderSvgToString(project, modelName);
    const osCanvas = document.createElement('canvas');
    osCanvas.width = viewbox.width * 4;
    osCanvas.height = viewbox.height * 4;
    const ctx = exists(osCanvas.getContext('2d'));
    const svgBlob = new Blob([svg], { type: 'image/svg+xml;charset=utf-8' });
    const svgUrl = URL.createObjectURL(svgBlob);

    const image = new Image();
    image.onload = () => {
      ctx.drawImage(image, 0, 0, viewbox.width * 4, viewbox.height * 4);

      osCanvas.toBlob((snapshotBlob) => {
        if (snapshotBlob) {
          this.setState({ snapshotBlob });
        } else {
          this.setState({
            modelErrors: [...this.state.modelErrors, new Error('snapshot creation failed (1).')],
          });
        }
      });
    };
    image.onerror = () => {
      this.setState({
        modelErrors: [...this.state.modelErrors, new Error('snapshot creation failed (2).')],
      });
    };

    image.src = svgUrl;
  }

  handleSnapshot = (kind: 'show' | 'close') => {
    if (kind === 'show') {
      setTimeout(() => {
        this.takeSnapshot();
      });
    }
  };

  getMetaActionsBar() {
    const { embedded } = this.props;
    if (embedded) {
      return undefined;
    }

    const zoom = this.getView()?.zoom || 1;

    return (
      <div className={styles.undoRedoBar}>
        <UndoRedoBar
          undoEnabled={this.isUndoEnabled()}
          redoEnabled={this.isRedoEnabled()}
          onUndoRedo={this.handleUndoRedo}
        />
        {/*<Snapshotter onSnapshot={this.handleSnapshot} />*/}
        <ZoomBar zoom={zoom} onChangeZoom={this.handleZoomChange} />
      </div>
    );
  }

  getEditorControls() {
    const { embedded } = this.props;
    const { dialOpen, dialVisible, selectedTool } = this.state;

    if (embedded || isStdlibModel(this.state.modelName)) {
      return undefined;
    }

    return (
      <SpeedDial
        ariaLabel="hide or show editor tools"
        className={styles.speedDial}
        hidden={!dialVisible}
        icon={<SpeedDialIcon icon={<EditIcon />} openIcon={<ClearIcon />} />}
        onClick={this.handleDialClick}
        onClose={this.handleDialClose}
        open={dialOpen}
      >
        <SpeedDialAction
          icon={<StockIcon />}
          title="Stock"
          onClick={this.handleSelectStock}
          selected={selectedTool === 'stock'}
        />
        <SpeedDialAction
          icon={<FlowIcon />}
          title="Flow"
          onClick={this.handleSelectFlow}
          selected={selectedTool === 'flow'}
        />
        <SpeedDialAction
          icon={<AuxIcon />}
          title="Variable"
          onClick={this.handleSelectAux}
          selected={selectedTool === 'aux'}
        />
        <SpeedDialAction
          icon={<LinkIcon />}
          title="Link"
          onClick={this.handleSelectLink}
          selected={selectedTool === 'link'}
        />
        <SpeedDialAction
          icon={<ModuleIcon />}
          title="Module"
          onClick={this.handleSelectModule}
          selected={selectedTool === 'module'}
        />
      </SpeedDial>
    );
  }

  getSnapshot() {
    const { embedded } = this.props;
    const { snapshotBlob } = this.state;

    if (embedded || !snapshotBlob) {
      return undefined;
    }

    return (
      <div className={styles.snapshotCard}>
        <div className={styles.snapshotCardContent}>
          <img src={URL.createObjectURL(snapshotBlob)} className={styles.snapshotImg} alt="diagram snapshot" />
        </div>
        <div className={styles.snapshotCardActions}>
          <Button size="small" color="primary" onClick={this.handleClearSnapshot}>
            Close
          </Button>
        </div>
      </div>
    );
  }

  handleClearSnapshot = () => {
    this.setState({ snapshotBlob: undefined });
  };

  render(): React.ReactNode {
    const { embedded } = this.props;

    const classNames = clsx(styles.editor, embedded ? '' : styles.editorBg);

    return (
      <div className={classNames}>
        {this.getDrawer()}
        {this.getDetails()}
        {this.getSearchBar()}
        {this.getSharedModelBanner()}
        {this.getCanvas()}
        {this.getSnackbar()}
        {this.getEditorControls()}
        {this.getMetaActionsBar()}
        {this.getSnapshot()}
      </div>
    );
  }
}
