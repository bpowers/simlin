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

import { Project as EngineProject } from '@simlin/engine';
import type { JsonProjectPatch, JsonModelOperation, JsonSimSpecs, JsonArrayedEquation } from '@simlin/engine';
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
  CloudViewElement,
  viewElementType,
  Rect,
  isNamedViewElement,
  stockToJson,
  flowToJson,
  auxToJson,
  moduleToJson,
  type ModuleReference,
} from '@simlin/core/datamodel';
import { defined, exists, setsEqual } from '@simlin/core/common';
import { getOrThrow, only } from '@simlin/core/collections';

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
import { Canvas, inCreationUid } from './drawing/Canvas';
import { Point, searchableName } from './drawing/common';
import { computeFlowAttachment } from './flow-attach';
import { applyGroupMovement } from './group-movement';
import { detectUndoRedo, isEditableElement } from './keyboard-shortcuts';
import { isStdlibModel } from './module-navigation';
import { countModelInstances } from './module-details-utils';
import { BreadcrumbBar } from './BreadcrumbBar';
import { ProjectController, type ProjectSnapshot, type EngineApi } from './project-controller';

import styles from './Editor.module.css';
// These must stay in sync with --panel-width-sm/-md/-lg in theme.css (and the
// media-query breakpoints in Editor.module.css).
const SearchbarWidthSm = 359;
const SearchbarWidthMd = 420;
const SearchbarWidthLg = 480;

// The effective right-panel width at the current viewport, mirroring the
// media queries in Editor.module.css. Used by viewport-centering math, which
// previously hardcoded one width and was wrong at the other breakpoints.
function panelWidth(): number {
  if (typeof window === 'undefined') {
    return SearchbarWidthSm;
  }
  const w = window.innerWidth;
  if (w >= 1200) {
    return SearchbarWidthLg;
  } else if (w >= 900) {
    return SearchbarWidthMd;
  }
  return SearchbarWidthSm;
}

// Stable no-op handlers for the read-only/embedded Canvas. Canvas is a
// React.memo function component: allocating fresh arrow functions per render
// would defeat its shallow prop comparison and force a full re-render of every
// layer on every Editor render.
const noopRename = (_oldName: string, _newName: string): void => {};
const noopSetSelection = (_selected: ReadonlySet<UID>): void => {};
const noopMoveSelection = (_position: Point): void => {};
const noopMoveFlow = (_e: FlowViewElement, _t: number, _p: Point): void => {};
const noopMoveLabel = (_u: UID, _s: 'top' | 'left' | 'bottom' | 'right'): void => {};
const noopAttachLink = (_element: LinkViewElement, _to: string): void => {};
const noopCreateVariable = (_element: ViewElement): void => {};
const noop = (): void => {};
const noopViewBoxChange = (_viewBox: Rect, _zoom: number): void => {};
const noopDrillIntoModule = (_moduleIdent: string, _targetModelName: string): void => {};

// Extends the built-in Error so instances carry a stack trace and satisfy
// `instanceof Error` (a bare `implements Error` produced a plain object with
// neither). The explicit name assignment survives minification, where the
// subclass's constructor name is mangled.
class EditorError extends Error {
  constructor(msg: string) {
    super(msg);
    this.name = 'EditorError';
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

// Editor state is now split in two: the project/engine coordination state
// lives in the ProjectController and is mirrored here as a single immutable
// `controllerSnapshot` field (replaced wholesale on every controller change,
// so a new snapshot identity drives a re-render). The remaining fields are
// genuinely Editor-owned UI/presentation state. Held as one useState object
// (see the function component) with a class-like merging setState helper.
interface EditorState {
  // The latest immutable snapshot published by the ProjectController. Holds
  // project, projectVersion, projectGeneration, status, cachedErrors, data,
  // modelName, modelStack, and the undo/redo predicates.
  controllerSnapshot: ProjectSnapshot;
  // Toast-style transient errors. These STAY in the Editor as UI state: the
  // controller surfaces errors via its onError config callback, which appends
  // here. The controller never owns presentation state.
  modelErrors: readonly Error[];
  dialOpen: boolean;
  dialVisible: boolean;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;
  selection: ReadonlySet<UID>;
  showDetails: 'variable' | 'errors' | undefined;
  flowStillBeingCreated: boolean;
  drawerOpen: boolean;
  // Object URL for the diagram snapshot image, created once when the
  // snapshot blob is produced (see takeSnapshot) rather than on every
  // render. Held as state so getSnapshot can render it without leaking a
  // fresh URL per render; revoked when replaced, cleared, or on unmount.
  snapshotUrl: string | undefined;
  variableDetailsActiveTab: number;
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
  // drawer offers a destructive "Delete project" action that calls
  // Resolving means the host has navigated away; rejecting surfaces the
  // error in the confirmation dialog. Hosts without a deletable backing
  // project (the local file-backed viewer, embeds) leave this undefined.
  onDeleteProject?: () => Promise<void>;
}

export type EditorProps = EditorPropsBase & ProjectInputProps;

// The mutable, non-render instance state that lived as class instance fields
// (*) and is read/written by escaped callbacks (the controller
// subscription, the global keydown listener, the async snapshot image
// callbacks) and the post-commit effects. Collected into a single ref so the
// function component keeps one "current" view -- exactly as `*` always
// reflected the latest values.
interface EditorRefs {
  // The headless coordination layer. Created in the mount effect and disposed
  // in its cleanup. A mount -> unmount -> mount cycle (React 18 StrictMode)
  // therefore creates a *fresh* controller on the second mount -- the first
  // one was disposed -- so no unmounted-flag/timer machinery is needed: the
  // controller's own `disposed` latch guards every async continuation it owns.
  // Undefined between unmount and the next mount; the snapshot in state covers
  // render in those windows.
  controller: ProjectController | undefined;
  unsubscribe: (() => void) | undefined;
  // Tracks the navResetSeq we last reacted to so the nav-reset effect clears
  // selection exactly once per undo-driven navigation reset. Seeded from the
  // controller's initial snapshot so an unchanged value never fires on mount.
  lastNavResetSeq: number;
  // Stable React keys for snackbar toasts. Keying by `${name}:${message}`
  // collided whenever two distinct errors shared a name and message (e.g. the
  // same engine error raised twice), so React reused one toast's instance for
  // the other and the auto-hide timer of the first dismissed the second
  // prematurely. Assigning a monotonically increasing id per *instance* the
  // first time it is rendered gives every appended error a unique,
  // render-stable key regardless of message text. The WeakMap MUST persist
  // across renders (it is the identity-keyed registry), so it lives here, not
  // as a render-scope allocation.
  nextErrorKey: number;
  errorKeys: WeakMap<Error, number>;
  // Single owner of the live snapshot object URL. State (`snapshotUrl`) only
  // mirrors this for render. Two snapshots completing before the first commit
  // would, if we read the previous URL from state, both see the same stale
  // value and leak one URL; reading and revoking this field synchronously in
  // setSnapshotUrl is race-free.
  liveSnapshotUrl: string | undefined;
  // Guards the read-only toast to a single append for the lifetime of this
  // Editor instance. The class appended it in componentDidMount; under React
  // 18 StrictMode (mount -> unmount -> mount on the same fiber, state
  // preserved) the mount effect runs twice, so this latch keeps the
  // documented "appended once on mount" invariant.
  readOnlyToastAppended: boolean;
}

// The snapshot of props + state that escaped callbacks (the controller
// subscription, the global keydown listener, the async snapshot image
// onload/onerror) must see CURRENT, not as captured by a stale render closure.
// Refreshed synchronously on every render so any escaped callback reads the
// same values `props` / `state` would have. Also read by every
// handler (which are useCallback([])-stable, mirroring the class's bound
// methods) so "event-time reads go through `latest`" is uniform.
interface EditorLatest {
  props: EditorProps;
  state: EditorState;
}

// Main model editor (the imperative shell). Converted from a
// React.PureComponent to a React.memo function component: React.memo replaces
// PureComponent's shallow-prop gate (state changes always re-render in both
// worlds). EditorState is held as a single useState object with a class-like
// merging `setState` helper, preserving the class's merged-snapshot semantics
// for handlers that issue several setState calls in sequence. Former instance
// fields become refs (see EditorRefs); former props/state reads from
// escaped callbacks go through the `latest` ref (see EditorLatest).
export const Editor = React.memo(function Editor(props: EditorProps): React.ReactElement {
  // ---- Instance fields (formerly *) as one ref -----------------------
  const refs = React.useRef<EditorRefs>(undefined as unknown as EditorRefs);

  // Build a fresh ProjectController wired to the given props. Constructing a
  // controller is side-effect free (no engine opened); openInitialProject() is
  // what loads the engine. Stored into refs.current.controller and seeds
  // lastNavResetSeq from the controller's initial snapshot. Reads props
  // through `latest` for onError/save so it stays current under prop changes;
  // the input format is captured from the passed `p` (immutable per project).
  const makeController = (p: EditorProps): ProjectController => {
    const controller = new ProjectController({
      initialProjectVersion: p.initialProjectVersion,
      input:
        p.inputFormat === 'protobuf'
          ? { format: 'protobuf', data: p.initialProjectBinary }
          : { format: 'json', data: p.initialProjectJson },
      // The concrete engine Project/Model/Run structurally satisfy the
      // controller's EngineApi surface; cast through unknown to bridge the
      // nominal type difference.
      openProtobuf: (data) => EngineProject.openProtobuf(data) as unknown as Promise<EngineApi>,
      openJson: (data) => EngineProject.openJson(data) as unknown as Promise<EngineApi>,
      save: async (project, currVersion) => {
        // Read the freshest onSave/inputFormat through `latest` so a prop
        // change between controller construction and a save uses the new one.
        const cur = latest.current.props;
        if (cur.inputFormat === 'json') {
          // The controller hands back the format matching inputFormat, so a
          // 'json' input always produces a JsonProjectData payload here.
          return await cur.onSave({ format: 'json', data: project.data as string }, currVersion);
        }
        return await cur.onSave({ format: 'protobuf', data: project.data as Uint8Array }, currVersion);
      },
      onError: (err) => {
        // Append to the toast list. setState((prev) => ...) so concurrent
        // error reports (e.g. two sim-run failures) don't clobber each other.
        setState((prev) => ({ modelErrors: [...prev.modelErrors, err] }));
      },
    });
    refs.current.controller = controller;
    refs.current.lastNavResetSeq = controller.getSnapshot().navResetSeq;
    return controller;
  };

  // ---- Lazy one-time init: refs + the initial controller ------------------
  // Mirrors the class constructor: makeController() is side-effect free (no
  // engine opened) so the initial snapshot is available to seed state. The
  // engine open and subscription are kicked off in the mount effect -- see
  // there for the StrictMode rationale. The controller is constructed HERE
  // (inside the `refs.current === undefined` guard), not in the useState
  // initializer: React.StrictMode double-invokes the useState initializer in
  // dev, which would construct two controllers and orphan one; the refs guard
  // runs exactly once per fiber, so exactly one controller is built -- matching
  // the class, whose constructor (and thus makeController) ran once.
  if (refs.current === undefined) {
    refs.current = {
      controller: undefined,
      unsubscribe: undefined,
      lastNavResetSeq: 0,
      nextErrorKey: 1,
      errorKeys: new WeakMap<Error, number>(),
      liveSnapshotUrl: undefined,
      readOnlyToastAppended: false,
    };
    makeController(props);
  }
  const r = refs.current;

  // ---- EditorState as one useState object with class-like merge -----------
  // The initializer reads the already-constructed controller's snapshot; it is
  // idempotent under StrictMode's double-invoke (it never constructs a
  // controller, only reads the one the refs guard built).
  const [state, setStateRaw] = React.useState<EditorState>(() => ({
    controllerSnapshot: defined(r.controller).getSnapshot(),
    modelErrors: [],
    dialOpen: false,
    dialVisible: true,
    selectedTool: undefined,
    selection: new Set<number>(),
    showDetails: undefined,
    flowStillBeingCreated: false,
    drawerOpen: false,
    snapshotUrl: undefined,
    variableDetailsActiveTab: 0,
  }));

  // Class-parity setState: merges a partial patch (or a functional updater that
  // returns one) onto the previous state, exactly like React.Component's
  // setState. Multiple calls in one handler batch into a single commit (React
  // 18), so a handler that calls setState several times still produces one
  // render carrying the net transition -- matching the class's batching.
  const setState = React.useCallback(
    (patch: Partial<EditorState> | ((prev: EditorState) => Partial<EditorState>)): void => {
      setStateRaw((prev) => {
        const next = typeof patch === 'function' ? patch(prev) : patch;
        return { ...prev, ...next };
      });
    },
    [],
  );

  // ---- Latest props/state snapshot for escaped callbacks ------------------
  // Updated synchronously below on every render. The controller subscription,
  // the keydown listener, the async snapshot callbacks, and every handler read
  // through this so they observe CURRENT values (the class read
  // props/state, which were always current). Writing during render
  // is safe: it is the same data the render below uses, just exposed to
  // non-render-scope callers.
  const latest = React.useRef<EditorLatest>(undefined as unknown as EditorLatest);
  latest.current = { props, state };

  const errorKey = (err: Error): number => {
    let key = r.errorKeys.get(err);
    if (key === undefined) {
      key = r.nextErrorKey++;
      r.errorKeys.set(err, key);
    }
    return key;
  };

  // ---- Mount / unmount effect (componentDidMount / componentWillUnmount) ---
  // Runs once (empty deps); reads the latest props/state through `latest`.
  // Cleanup is symmetric so a StrictMode mount/unmount/mount cycle disposes the
  // first controller and the second mount builds a fresh one, leaving nothing
  // stuck (no orphaned listener, subscription, or engine handle).
  React.useEffect(() => {
    // React 18 StrictMode (dev) drives every committed component through
    // mount -> unmount -> mount on the *same* fiber, without re-running the
    // lazy init. The cleanup disposes the controller and clears it; the second
    // mount must therefore create a fresh one. The lazy-init controller is
    // reused on the very first mount and recreated on every subsequent mount.
    if (!r.controller) {
      makeController(latest.current.props);
    }
    const controller = defined(r.controller);

    // Mirror controller snapshots into one state field. React.memo + state
    // identity drive the re-render; an unchanged snapshot is a no-op. Seed
    // state from the (fresh) snapshot in case makeController ran just above.
    setState({ controllerSnapshot: controller.getSnapshot() });
    r.unsubscribe = controller.subscribe(() => {
      const c = r.controller;
      if (c) {
        setState({ controllerSnapshot: c.getSnapshot() });
      }
    });

    // Append the read-only toast exactly once per Editor instance. The class
    // appended it on each componentDidMount; under StrictMode (state preserved
    // across the double mount) that would double-append, so the latch keeps the
    // documented "appended once on mount" behavior. Functional updater so it
    // composes with any concurrent error append.
    if (latest.current.props.readOnlyMode && !r.readOnlyToastAppended) {
      r.readOnlyToastAppended = true;
      setState((prev) => ({
        modelErrors: [
          ...prev.modelErrors,
          new Error("This is a read-only version. Any changes you make won't be saved."),
        ],
      }));
    }

    document.addEventListener('keydown', handleKeyDown);

    // Open the engine, then schedule the first sim run. The controller guards
    // its own dispose-races internally (see ProjectController.dispose), so no
    // Editor-side timer or unmounted flag is needed here.
    void controller.openInitialProject().then(() => {
      r.controller?.scheduleSimRun();
    });

    return () => {
      // componentWillUnmount: remove the keydown listener, unsubscribe (before
      // disposing, so a final controller notification can't setState on an
      // unmounting component), dispose the controller (releasing the WASM
      // EngineProject handle -- the Editor mounts/unmounts on every wouter
      // route change in src/app and every EditorHost path swap in
      // src/simlin-serve; without this every navigation away leaks ~several MB
      // of WASM linear memory plus salsa caches), and revoke the snapshot URL.
      // dispose() is best-effort and latches the controller's `disposed` flag
      // so any in-flight open/undo releases its own engine. Symmetric with the
      // setup above so a StrictMode mount/unmount/mount cycle builds a fresh
      // controller on remount and leaves nothing stuck.
      document.removeEventListener('keydown', handleKeyDown);

      if (r.unsubscribe) {
        r.unsubscribe();
        r.unsubscribe = undefined;
      }
      const controllerToDispose = r.controller;
      r.controller = undefined;
      if (controllerToDispose) {
        // dispose() resolves by contract (it swallows engine-teardown errors),
        // but attach a catch defensively so a rejected teardown can never
        // become an unhandled rejection that crashes the host.
        controllerToDispose.dispose().catch(() => {});
      }

      // Revoke any outstanding snapshot object URL so navigating away from a
      // project with an open snapshot doesn't strand the blob. Read the live
      // URL from the owning ref field, not state, in case a snapshot completed
      // without its setState having committed yet.
      if (r.liveSnapshotUrl) {
        URL.revokeObjectURL(r.liveSnapshotUrl);
        r.liveSnapshotUrl = undefined;
      }
    };
    // Intentionally empty deps: this effect mirrors componentDidMount/Unmount.
    // Everything it reads goes through `latest`/`r`, and the escaped callbacks
    // (subscription, keydown, openInitialProject continuation) likewise read
    // `latest`/`r`, so nothing here closes over stale values. (The repo lint
    // config does not enable react-hooks/exhaustive-deps, so no disable
    // directive is needed.)
  }, []);

  // ---- Post-commit effect: onSelectionChanged (componentDidUpdate part 1) --
  // Fire onSelectionChanged whenever the committed selection actually changed,
  // but NOT on initial mount. Driving this from an effect keyed on the
  // committed selection (rather than a setTimeout(0) inside handleSelection)
  // means the host observes *every* committed selection change -- not just
  // clicks routed through handleSelection, but also selections cleared by a
  // delete and resets on module drill-in/back. (A normal undo/redo preserves
  // the selection and fires nothing; the selection only resets when the viewed
  // model disappears from the restored project -- see the navResetSeq effect
  // below.) getSelectionIdents reads the already-committed state, so no
  // deferral is needed; effects never run after unmount.
  //
  // The class compared prevState.selection to the committed selection with
  // setsEqual. A useEffect keyed on `state.selection` re-runs on every commit
  // that changed the selection's *identity*; the prevSelection ref + setsEqual
  // reproduce the class's content-equality guard (undo/navigate-back rebuild a
  // content-identical Set, which must NOT re-notify) and the "not on mount"
  // rule (the ref is seeded to the initial selection so the first run is a
  // no-op).
  const prevSelectionRef = React.useRef<ReadonlySet<UID>>(state.selection);
  React.useEffect(() => {
    if (setsEqual(prevSelectionRef.current, state.selection)) {
      return;
    }
    prevSelectionRef.current = state.selection;
    const onSelectionChanged = latest.current.props.onSelectionChanged;
    if (onSelectionChanged) {
      onSelectionChanged(getSelectionIdents());
    }
  }, [state.selection]);

  // ---- Post-commit effect: navResetSeq (componentDidUpdate part 2) ---------
  // When undo/redo restores a project that no longer contains the viewed model,
  // the controller resets navigation to 'main' and bumps navResetSeq. Clear the
  // Editor's selection/details/tool UI state for that case only (an ordinary
  // undo preserves them). Drill-in / back / level manage the selection through
  // their own handlers, so they do not bump navResetSeq. r.lastNavResetSeq is
  // seeded from the initial snapshot so an unchanged value never fires on mount.
  const navResetSeq = state.controllerSnapshot.navResetSeq;
  React.useEffect(() => {
    if (navResetSeq !== r.lastNavResetSeq) {
      r.lastNavResetSeq = navResetSeq;
      setState({
        selection: new Set<UID>(),
        showDetails: undefined,
        selectedTool: undefined,
      });
    }
  }, [navResetSeq]);

  // ---- Handlers (formerly bound class methods) ----------------------------
  // Each is wrapped in useCallback([]) so its identity is stable across
  // renders -- exactly as the class's bound methods were -- which preserves the
  // memoization of the React.memo'd children they are passed to (Canvas,
  // Status, UndoRedoBar, ZoomBar). They read CURRENT props/state through
  // `latest`/`r`, never a stale render closure, so empty deps is correct.

  const handleKeyDown = React.useCallback((e: KeyboardEvent): void => {
    const { props: p } = latest.current;
    // Don't handle shortcuts in embedded mode or editable fields
    if (p.embedded || isEditableElement(e.target)) {
      return;
    }

    const action = detectUndoRedo(e);
    if (!action) {
      return;
    }

    const isEnabled = action === 'undo' ? isUndoEnabled() : isRedoEnabled();
    if (isEnabled) {
      e.preventDefault();
      handleUndoRedo(action);
    }
  }, []);

  const isUndoEnabled = (): boolean => {
    return latest.current.state.controllerSnapshot.canUndo;
  };

  const isRedoEnabled = (): boolean => {
    return latest.current.state.controllerSnapshot.canRedo;
  };

  // Delegating accessor for the active data-model Project. Kept for the
  // Editor's own render/op-building reads. No external consumer (HostedWebEditor,
  // simlin-serve's EditorHost) uses it. Named getProject (the class method was
  // project()) to avoid colliding with the many `const project = ...` locals.
  const getProject = (): Project | undefined => {
    return latest.current.state.controllerSnapshot.project;
  };

  // Op-building helpers go through the controller's apply* / view methods, so
  // they generally don't need the raw engine handle. Retained as a delegating
  // accessor (returns undefined before the engine opens / after dispose). Named
  // getEngineProject (the class method was engine()) to avoid colliding with the
  // `const engine = ...` locals.
  const getEngineProject = (): EngineProject | undefined => {
    return r.controller?.getEngine() as EngineProject | undefined;
  };

  // Convenience wrapper for the simple edit handlers: apply a patch and, on
  // success, refresh from the engine. All engine/save/sim coordination lives
  // in the controller now. Returns false (without refreshing) on patch failure.
  const applyPatchAndRefresh = async (patch: JsonProjectPatch, label: string): Promise<boolean> => {
    const controller = r.controller;
    if (!controller) {
      return false;
    }
    return await controller.applyPatch(patch, label);
  };

  // Surface a transient error to the toast list. Op-building handlers that
  // detect a problem before reaching the engine (or that report a synchronous
  // failure) call this; the controller surfaces its own errors via onError,
  // which appends to the same list.
  const appendModelError = (msg: string): void => {
    setState((prevState: EditorState) => ({
      modelErrors: [...prevState.modelErrors, new EditorError(msg)],
    }));
  };

  // The active model name lives in the controller snapshot now. Op-building
  // patches target it so operations work at any module nesting depth.
  const modelName = (): string => {
    return latest.current.state.controllerSnapshot.modelName;
  };

  // Thin delegating wrappers so the Editor's op-building handlers can keep
  // their shape. All engine/save/sim/history coordination lives in the
  // controller. Each is a no-op when no controller is mounted.
  const applyPatchOrReportError = async (patch: JsonProjectPatch, label: string): Promise<boolean> => {
    const controller = r.controller;
    if (!controller) {
      return false;
    }
    return await controller.applyPatchOrReportError(patch, label);
  };

  const refreshFromEngine = async (): Promise<void> => {
    await r.controller?.refreshFromEngine();
  };

  const scheduleSimRun = (): void => {
    r.controller?.scheduleSimRun();
  };

  const updateView = async (view: StockFlowView): Promise<void> => {
    await r.controller?.updateView(view);
  };

  const queueViewUpdate = async (view: StockFlowView): Promise<void> => {
    await r.controller?.queueViewUpdate(view);
  };

  const handleDialClick = React.useCallback((_event: React.MouseEvent<HTMLButtonElement>): void => {
    setState((prev) => ({ dialOpen: !prev.dialOpen }));
  }, []);

  const handleDialClose = React.useCallback((_e: React.SyntheticEvent, reason: CloseReason): void => {
    if (reason === 'mouseLeave' || reason === 'blur') {
      return;
    }
    // escapeKeyDown: close dial and clear tool
    setState({
      dialOpen: false,
      selectedTool: undefined,
    });
  }, []);

  const handleRename = React.useCallback(async (oldName: string, newName: string): Promise<void> => {
    if (oldName === newName) {
      return;
    }

    const eng = getEngineProject();
    if (!eng) {
      return;
    }

    const view = defined(getView());
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
      models: [{ name: modelName(), ops }],
    };

    if (!(await applyPatchOrReportError(patch, 'rename'))) {
      // A failed rename leaves flowStillBeingCreated untouched.
      return;
    }

    // Clear the in-progress flow-creation flag synchronously after the
    // patch succeeds and BEFORE the engine round-trip in refreshFromEngine.
    // This matches the pre-refactor ordering: the details panel for a
    // just-named flow must un-suppress immediately, not wait out the
    // serialize/JSON/setState round-trip.
    setState({
      flowStillBeingCreated: false,
    });
    await refreshFromEngine();
  }, []);

  const handleSelection = React.useCallback((selection: ReadonlySet<UID>): void => {
    setState({
      selection,
      flowStillBeingCreated: false,
      variableDetailsActiveTab: 0,
    });
    if (selection.size === 0) {
      setState({ showDetails: undefined });
    }
    // The host's onSelectionChanged callback is no longer fired here. It is
    // fired from the selection-change effect when the committed selection
    // changes, which covers this path plus every other route that mutates the
    // selection (delete, module drill-in/back, undo/redo). Reading the
    // selection there guarantees it observes the committed state without a
    // setTimeout(0) deferral.
  }, []);

  const handleShowVariableDetails = React.useCallback((): void => {
    setState({ showDetails: 'variable' });
  }, []);

  const getLatexEquation = React.useCallback(async (ident: string): Promise<string | undefined> => {
    const eng = getEngineProject();
    if (!eng) return undefined;
    try {
      const model = await eng.getModel(modelName());
      return (await model.getLatexEquation(ident)) ?? undefined;
    } catch {
      return undefined;
    }
  }, []);

  const handleSelectionDelete = React.useCallback(async (): Promise<void> => {
    const selection = latest.current.state.selection;
    const mName = modelName();
    const view = defined(getView());

    // this will remove the selected elements, clouds, and connectors
    let elements = view.elements.filter((element: ViewElement) => {
      const remove =
        selection.has(element.uid) ||
        (element.type === 'cloud' && selection.has(element.flowUid)) ||
        (element.type === 'link' && (selection.has(element.toUid) || selection.has(element.fromUid)));
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

    // Parity with the pre-refactor `if (!engine) return`: bail before clearing
    // the selection or running the optimistic view update if the engine hasn't
    // finished opening yet, so a delete attempted in that brief window cleanly
    // no-ops instead of mutating UI state against a project that can't apply it.
    if (!r.controller?.getEngine()) {
      return;
    }

    const deleteOps: JsonModelOperation[] = getSelectionIdents().map((ident) => ({
      type: 'deleteVariable' as const,
      payload: { ident },
    }));

    // Clear the selection now, in the same synchronous block (before any
    // await) as the view update below, so React batches them into a single
    // render: no consumer should ever observe a selection that references an
    // element the view no longer contains. (Clearing it after
    // `await updateView(...)` instead left a window where props.view had
    // dropped the deleted element but props.selection still pointed at it --
    // Canvas's buildSelectionMap now tolerates that, but the state transition
    // should still be atomic.) The deleteOps above were computed from the
    // pre-clear selection.
    setState({
      selection: new Set<number>(),
    });

    if (deleteOps.length > 0) {
      const patch: JsonProjectPatch = {
        models: [{ name: mName, ops: deleteOps }],
      };
      // The controller reports any failure via onError; we ignore the boolean
      // here because the view update below must run regardless (matching the
      // original, which committed the cloud/view changes even on a delete-op
      // failure).
      await applyPatchOrReportError(patch, 'delete');
    }

    await updateView({ ...view, elements, nextUid });
    scheduleSimRun();
  }, []);

  const handleMoveLabel = React.useCallback(
    async (uid: UID, side: 'top' | 'left' | 'bottom' | 'right'): Promise<void> => {
      const view = defined(getView());

      const elements = view.elements.map((element: ViewElement) => {
        if (element.uid !== uid || !isNamedViewElement(element)) {
          return element;
        }
        return { ...element, labelSide: side };
      });

      await updateView({ ...view, elements });
    },
    [],
  );

  const handleFlowAttach = React.useCallback(
    async (
      flow: FlowViewElement,
      targetUid: number,
      cursorMoveDelta: Point,
      fauxTargetCenter: Point | undefined,
      inCreation: boolean,
      isSourceAttach?: boolean,
    ): Promise<void> => {
      const view = defined(getView());
      const model = defined(getModel());

      // Pure core: compute the new view (elements + nextUid), the model
      // operations to apply, and the selection/creation state. See
      // flow-attach.ts for the source/sink and op-builder deduplication.
      const result = computeFlowAttachment(view, model.variables, {
        flow,
        targetUid,
        cursorMoveDelta,
        fauxTargetCenter,
        inCreation,
        isSourceAttach: !!isSourceAttach,
      });

      // The pure core only assigns a selection when creating a new flow;
      // otherwise it returns undefined and the existing selection is preserved.
      // This matches the original, which seeded `selection` from state and only
      // reassigned it in the creation path.
      const selection = result.selection ?? latest.current.state.selection;

      // Preserve the original's early return on a missing engine: it bailed
      // before applying ops or updating the view (no setState, no sim run).
      if (!r.controller?.getEngine()) {
        return;
      }

      if (result.ops.length > 0) {
        const patch: JsonProjectPatch = {
          models: [{ name: modelName(), ops: [...result.ops] }],
        };
        // On patch failure, commit the selection/creation flag but DO NOT
        // update the view -- preserving the original behavior exactly.
        if (!(await applyPatchOrReportError(patch, 'flow attach'))) {
          setState({ selection, flowStillBeingCreated: inCreation });
          return;
        }
      }

      await updateView({ ...view, nextUid: result.nextUid, elements: [...result.elements] });
      setState({
        selection,
        flowStillBeingCreated: inCreation,
      });
      scheduleSimRun();
    },
    [],
  );

  const handleLinkAttach = React.useCallback(async (link: LinkViewElement, newTarget: string): Promise<void> => {
    let { selection } = latest.current.state;
    let view = defined(getView());

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

    await updateView(view);
    setState({ selection });
  }, []);

  const handleCreateVariable = React.useCallback(async (element: ViewElement): Promise<void> => {
    const view = defined(getView());
    // Parity with the pre-refactor `if (!engine) return`: bail before the
    // optimistic view update if the engine hasn't finished opening yet, so a
    // create attempted in that window cleanly no-ops.
    if (!r.controller?.getEngine()) {
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

    // AC5.2: patch targets modelName() (not a hardcoded value), so module
    // creation works at any nesting depth -- navigating into a child model
    // updates modelName, and newly created modules land in that child.
    const patch: JsonProjectPatch = {
      models: [{ name: modelName(), ops: [op] }],
    };

    // The controller reports any failure via onError; the view update below
    // runs regardless, matching the original (which committed the new element
    // even when the upsert errored).
    await applyPatchOrReportError(patch, 'variable creation');

    await updateView({ ...view, nextUid, elements });
    setState({
      selection: new Set<number>(),
    });
  }, []);

  const handleSelectionMove = React.useCallback(
    async (delta: Point, arcPoint?: Point, segmentIndex?: number): Promise<void> => {
      const view = defined(getView());
      const selection = latest.current.state.selection;

      const { updatedElements } = applyGroupMovement({
        elements: view.elements,
        selection,
        delta,
        arcPoint,
        segmentIndex,
      });

      const elements = view.elements.map((el) => updatedElements.get(el.uid) ?? el);
      await updateView({ ...view, elements });
    },
    [],
  );

  const handleDrawerToggle = React.useCallback((isOpen: boolean): void => {
    setState({
      drawerOpen: isOpen,
    });
  }, []);

  const applySimSpecChange = async (updates: Partial<JsonSimSpecs>): Promise<void> => {
    // The engine is re-checked inside applyPatchAndRefresh; here we only
    // need the project to read the current sim specs.
    const project = getProject();
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

    await applyPatchAndRefresh(patch, 'sim specs');
  };

  const handleStartTimeChange = React.useCallback(async (event: React.ChangeEvent<HTMLInputElement>): Promise<void> => {
    const value = Number(event.target.value);
    await applySimSpecChange({ startTime: value });
  }, []);

  const handleStopTimeChange = React.useCallback(async (event: React.ChangeEvent<HTMLInputElement>): Promise<void> => {
    const value = Number(event.target.value);
    await applySimSpecChange({ endTime: value });
  }, []);

  const handleDtChange = React.useCallback(async (event: React.ChangeEvent<HTMLInputElement>): Promise<void> => {
    const value = Number(event.target.value);
    await applySimSpecChange({ dt: `${value}` });
  }, []);

  const handleTimeUnitsChange = React.useCallback(async (event: React.ChangeEvent<HTMLInputElement>): Promise<void> => {
    const value = event.target.value;
    await applySimSpecChange({ timeUnits: value });
  }, []);

  const handleDownloadXmile = React.useCallback(async (): Promise<void> => {
    const engine = getEngineProject();
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
      a.download = `${latest.current.props.name}-${latest.current.state.controllerSnapshot.projectVersion | 0}.stmx`;
      a.click();
      window.URL.revokeObjectURL(url);
    } catch (err: unknown) {
      const details = getErrorDetails(err);
      if (details.message) {
        appendModelError(details.message);
      }
    }
  }, []);

  const getDrawer = (): React.ReactElement | undefined => {
    const project = getProject();
    if (!project || latest.current.props.embedded) {
      return;
    }

    const model = project.models.get(modelName());
    if (!model) {
      return;
    }

    const simSpec = project.simSpecs;
    const dt = simSpec.dt.isReciprocal ? 1 / simSpec.dt.value : simSpec.dt.value;

    // A read-only viewer should never see a delete affordance even if a host
    // wired the callback.
    const onDelete = !latest.current.props.readOnlyMode ? latest.current.props.onDeleteProject : undefined;

    return (
      <ModelPropertiesDrawer
        modelName={project.name}
        open={latest.current.state.drawerOpen}
        onDrawerToggle={handleDrawerToggle}
        startTime={simSpec.start}
        stopTime={simSpec.stop}
        dt={dt}
        timeUnits={simSpec.timeUnits || ''}
        onStartTimeChange={handleStartTimeChange}
        onStopTimeChange={handleStopTimeChange}
        onDtChange={handleDtChange}
        onTimeUnitsChange={handleTimeUnitsChange}
        onDownloadXmile={handleDownloadXmile}
        onDelete={onDelete}
      />
    );
  };

  const getModel = (): Model | undefined => {
    const project = getProject();
    if (!project) {
      return;
    }
    const mName = modelName();
    return project.models.get(mName);
  };

  const getView = (): StockFlowView | undefined => {
    const project = getProject();
    if (!project) {
      return;
    }
    const mName = modelName();
    const model = project.models.get(mName);
    if (!model) {
      return;
    }

    return model.views[0];
  };

  const handleViewBoxChange = React.useCallback(async (viewBox: Rect, zoom: number): Promise<void> => {
    const view = defined(getView());
    await queueViewUpdate({ ...view, viewBox, zoom });
  }, []);

  const centerVariable = async (element: ViewElement): Promise<void> => {
    const view = defined(getView());
    const zoom = view.zoom;

    const cx = element.x;
    const cy = element.y;

    const viewCy = view.viewBox.height / 2 / zoom;
    const viewCx = (view.viewBox.width - panelWidth()) / 2 / zoom;

    const viewBox: Rect = {
      ...view.viewBox,
      x: viewCx - cx,
      y: viewCy - cy,
    };

    await queueViewUpdate({ ...view, viewBox });
  };

  const getCanvas = (): React.ReactElement | undefined => {
    const project = getProject();
    if (!project) {
      return;
    }

    const { embedded } = props;

    const model = getModel();
    if (!model) {
      return;
    }

    const view = getView();
    if (!view) {
      return;
    }

    // Stdlib models are read-only: disable all mutation handlers while
    // keeping selection, viewbox, and drill-in navigation active.
    const readOnly = embedded || isStdlibModel(modelName());
    const onRenameVariable = !readOnly ? handleRename : noopRename;
    const onSetSelection = !embedded ? handleSelection : noopSetSelection;
    const onMoveSelection = !readOnly ? handleSelectionMove : noopMoveSelection;
    const onMoveFlow = !readOnly ? handleFlowAttach : noopMoveFlow;
    const onMoveLabel = !readOnly ? handleMoveLabel : noopMoveLabel;
    const onAttachLink = !readOnly ? handleLinkAttach : noopAttachLink;
    const onCreateVariable = !readOnly ? handleCreateVariable : noopCreateVariable;
    const onClearSelectedTool = !readOnly ? handleClearSelectedTool : noop;
    const onDeleteSelection = !readOnly ? handleSelectionDelete : noop;
    const onShowVariableDetails = !readOnly ? handleShowVariableDetails : noop;
    const onViewBoxChange = !embedded ? handleViewBoxChange : noopViewBoxChange;
    const onDrillIntoModule = !embedded ? handleDrillIntoModule : noopDrillIntoModule;

    return (
      <Canvas
        embedded={!!embedded}
        project={project}
        model={model}
        view={view}
        version={latest.current.state.controllerSnapshot.projectVersion}
        selectedTool={readOnly ? undefined : latest.current.state.selectedTool}
        selection={latest.current.state.selection}
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
  };

  // Remove the single error identified by its per-instance toast id (the
  // same id used as the React key). Filtering by message text instead would
  // dismiss every error sharing that text -- so a repeated failing edit's
  // first auto-hide timer would close all of its duplicate toasts at once.
  const handleCloseSnackbar = React.useCallback((id: string | number): void => {
    setState((prevState) => ({
      modelErrors: prevState.modelErrors.filter((err) => errorKey(err) !== id),
    }));
  }, []);

  const getSnackbar = (): React.ReactElement | undefined => {
    const { embedded } = props;

    if (embedded) {
      return undefined;
    }

    return (
      <Snackbar
        anchorOrigin={{
          vertical: 'bottom',
          horizontal: 'center',
        }}
        open={latest.current.state.modelErrors.length > 0}
        autoHideDuration={6000}
      >
        <div>
          {latest.current.state.modelErrors.map((err) => {
            const id = errorKey(err);
            // These are genuine failures (engine open, sim-run, save/service
            // errors), so use the red error variant rather than amber warning.
            return <Toast variant="error" id={id} onClose={handleCloseSnackbar} message={err.message} key={id} />;
          })}
        </div>
      </Snackbar>
    );
  };

  const getSelectionIdents = (): string[] => {
    const names: string[] = [];
    const { selection } = latest.current.state;
    const view = getView();
    if (!view) {
      return names;
    }

    for (const e of view.elements) {
      if (selection.has(e.uid) && isNamedViewElement(e)) {
        names.push(defined(e.ident));
      }
    }

    return names;
  };

  // FIXME: use a map
  const getNamedSelectedElement = (): ViewElement | undefined => {
    if (latest.current.state.selection.size !== 1) {
      return;
    }

    const uid = only(latest.current.state.selection);

    const view = getView();
    if (!view) {
      return;
    }

    for (const e of view.elements) {
      if (e.uid === uid && isNamedViewElement(e)) {
        return e;
      }
    }

    return;
  };

  const getNamedElement = (ident: string): ViewElement | undefined => {
    const view = getView();
    if (!view) {
      return;
    }

    for (const e of view.elements) {
      if (isNamedViewElement(e) && e.ident === ident) {
        return e;
      }
    }

    return;
  };

  const handleShowDrawer = React.useCallback((): void => {
    setState({
      drawerOpen: true,
    });
  }, []);

  const handleDrillIntoModule = React.useCallback((moduleIdent: string, targetModelName: string): void => {
    const controller = r.controller;
    const view = getView();
    if (!controller || !view) {
      return;
    }
    // The controller owns the navigation stack and the active model; it guards
    // against drilling into a model the project doesn't contain (undefined
    // outcome). On success it returns the selection the Editor should adopt
    // (empty) and drives the model-scoped error refresh internally.
    const outcome = controller.drillIntoModule(
      moduleIdent,
      targetModelName,
      latest.current.state.selection,
      view.viewBox,
      view.zoom,
    );
    if (!outcome.restoredSelection) {
      return;
    }
    const newModelName = controller.getModelName();
    setState({
      selection: outcome.restoredSelection,
      showDetails: undefined,
      // Clear selected tool when entering a stdlib model (tool palette is hidden)
      selectedTool: isStdlibModel(newModelName) ? undefined : latest.current.state.selectedTool,
    });
  }, []);

  const handleNavigateBack = React.useCallback((): void => {
    const controller = r.controller;
    if (!controller) {
      return;
    }
    // The controller restores the parent's viewport internally (its modelName
    // updates synchronously, so its getView resolves to the restored model
    // with no setState-callback deferral) and returns the parent's selection
    // for the Editor to adopt. Undefined outcome means the stack was empty.
    const outcome = controller.navigateBack();
    if (!outcome.restoredSelection) {
      return;
    }
    setState({
      selection: outcome.restoredSelection,
      showDetails: undefined,
    });
  }, []);

  const handleNavigateToLevel = React.useCallback((targetLevel: number): void => {
    const controller = r.controller;
    if (!controller) {
      return;
    }
    const outcome = controller.navigateToLevel(targetLevel);
    if (!outcome.restoredSelection) {
      return;
    }
    setState({
      selection: outcome.restoredSelection,
      showDetails: undefined,
    });
  }, []);

  const handleSearchChange = React.useCallback(
    async (_event: React.SyntheticEvent | null, newValue: string | null): Promise<void> => {
      if (!newValue) {
        handleSelection(new Set());
        return;
      }
      const element = getNamedElement(canonicalize(newValue));
      handleSelection(element ? new Set([element.uid]) : new Set());
      // Don't open the mutation-capable details panel for read-only
      // models (stdlib models, embedded mode). The Canvas-level guard
      // at line ~1480 handles double-click, but search bypasses it.
      const readOnly = latest.current.props.embedded || isStdlibModel(modelName());
      setState({
        showDetails: readOnly ? undefined : 'variable',
      });
      if (element) {
        await centerVariable(element);
      }
    },
    [],
  );

  const handleStatusClick = React.useCallback((): void => {
    setState((prev) => ({
      showDetails: prev.showDetails === 'errors' ? undefined : 'errors',
    }));
  }, []);

  const getSearchBar = (): React.ReactElement | undefined => {
    const { embedded } = props;

    if (embedded) {
      return undefined;
    }

    let autocompleteOptions: Array<string> = [];
    const elements = getView()?.elements;
    if (elements) {
      autocompleteOptions = elements
        .filter((e) => isNamedViewElement(e))
        .map((e) => searchableName((e as NamedViewElement).name));
    }

    const namedElement = getNamedSelectedElement();
    let name;
    let placeholder: string | undefined = 'Find in Model';
    if (namedElement) {
      name = searchableName(defined((namedElement as NamedViewElement).name));
      placeholder = undefined;
    }

    const status = latest.current.state.controllerSnapshot.status;

    return (
      <div className={styles.searchBar}>
        <BreadcrumbBar
          modelStack={latest.current.state.controllerSnapshot.modelStack}
          modelName={modelName()}
          onBack={handleNavigateBack}
          onNavigateToLevel={handleNavigateToLevel}
          onShowDrawer={handleShowDrawer}
        />
        <div className={styles.searchBox}>
          <Autocomplete
            key={name}
            value={name}
            onChange={handleSearchChange}
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
        <Status status={status} onClick={handleStatusClick} />
      </div>
    );
  };

  // Returns the equation fields for a JSON patch operation.
  // For scalar equations, returns { equation: string }.
  // For arrayed equations, returns { arrayedEquation: JsonArrayedEquation }.
  const getEquationFields = (variable: Variable): { equation?: string; arrayedEquation?: JsonArrayedEquation } => {
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
  };

  const handleEquationChange = React.useCallback(
    async (
      ident: string,
      newEquation: string | undefined,
      newUnits: string | undefined,
      newDocs: string | undefined,
    ): Promise<void> => {
      const model = getModel();
      if (!model) {
        return;
      }

      const variable = model.variables.get(ident);
      if (!variable) {
        return;
      }

      // When newEquation is provided, use it as a scalar equation.
      // Otherwise, preserve the existing equation structure (including arrayed equations).
      const existingEqFields = getEquationFields(variable);

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
              references: variable.references.map((ref) => ({ src: ref.src, dst: ref.dst })),
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
        models: [{ name: modelName(), ops: [op] }],
      };

      await applyPatchAndRefresh(patch, 'equation update');
    },
    [],
  );

  const handleTableChange = React.useCallback(
    async (ident: string, newTable: GraphicalFunction | null): Promise<void> => {
      const model = getModel();
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
      const existingEqFields = getEquationFields(variable);

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
        models: [{ name: modelName(), ops: [op] }],
      };

      await applyPatchAndRefresh(patch, 'table update');
    },
    [],
  );

  // Updates the model reference for a module variable.
  const handleModuleModelReferenceChange = React.useCallback(
    async (ident: string, newModelName: string): Promise<void> => {
      const model = getModel();
      if (!model) return;
      const variable = model.variables.get(ident);
      if (!variable || variable.type !== 'module') return;

      const op: JsonModelOperation = {
        type: 'upsertModule',
        payload: {
          module: {
            name: variable.ident,
            modelName: newModelName,
            references: variable.references.map((ref) => ({ src: ref.src, dst: ref.dst })),
            units: variable.units || undefined,
            documentation: variable.documentation || undefined,
          },
        },
      };

      const patch: JsonProjectPatch = {
        models: [{ name: modelName(), ops: [op] }],
      };

      await applyPatchAndRefresh(patch, 'model reference update');
    },
    [],
  );

  // Updates units and/or documentation for a module variable.
  const handleModuleUnitsDocsChange = React.useCallback(
    async (ident: string, newUnits: string | undefined, newDocs: string | undefined): Promise<void> => {
      const model = getModel();
      if (!model) return;
      const variable = model.variables.get(ident);
      if (!variable || variable.type !== 'module') return;

      const op: JsonModelOperation = {
        type: 'upsertModule',
        payload: {
          module: {
            name: variable.ident,
            modelName: variable.modelName,
            references: variable.references.map((ref) => ({ src: ref.src, dst: ref.dst })),
            units: newUnits ?? variable.units ?? undefined,
            documentation: newDocs ?? variable.documentation ?? undefined,
          },
        },
      };

      const patch: JsonProjectPatch = {
        models: [{ name: modelName(), ops: [op] }],
      };

      await applyPatchAndRefresh(patch, 'module update');
    },
    [],
  );

  // Updates the input references array for a module variable via upsertModule.
  // The engine does full variable replacement (not merge), so we send the
  // complete module with the new references array.
  const handleModuleReferencesChange = React.useCallback(
    async (ident: string, newReferences: ReadonlyArray<ModuleReference>): Promise<void> => {
      const model = getModel();
      if (!model) return;
      const variable = model.variables.get(ident);
      if (!variable || variable.type !== 'module') return;

      const op: JsonModelOperation = {
        type: 'upsertModule',
        payload: {
          module: {
            name: variable.ident,
            modelName: variable.modelName,
            references: newReferences.map((ref) => ({ src: ref.src, dst: ref.dst })),
            units: variable.units || undefined,
            documentation: variable.documentation || undefined,
          },
        },
      };

      const patch: JsonProjectPatch = {
        models: [{ name: modelName(), ops: [op] }],
      };

      await applyPatchAndRefresh(patch, 'references update');
    },
    [],
  );

  // Creates a new empty model and sets it as the module's reference.
  // The engine processes projectOps before model ops (see patch.rs),
  // so AddModel creates the model before upsertModule references it.
  const handleCreateModelForModule = React.useCallback(async (moduleIdent: string): Promise<void> => {
    const project = getProject();
    if (!project) return;

    // Generate a unique model name to avoid collisions when the module
    // ident already matches an existing model name.
    let newModelName = moduleIdent;
    if (project.models.has(newModelName)) {
      newModelName = getUniqueDuplicateName(moduleIdent, project);
    }

    // Look up existing module to preserve metadata through the model reference change
    const model = getModel();
    const existingModule = model?.variables.get(moduleIdent);
    const modulePayload: {
      name: string;
      modelName: string;
      references?: { src: string; dst: string }[];
      units?: string;
      documentation?: string;
    } = {
      name: moduleIdent,
      modelName: newModelName,
    };
    if (existingModule && existingModule.type === 'module') {
      if (existingModule.references.length > 0) {
        modulePayload.references = existingModule.references.map((ref) => ({ src: ref.src, dst: ref.dst }));
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
          name: modelName(),
          ops: [{ type: 'upsertModule', payload: { module: modulePayload } }],
        },
      ],
    };

    await applyPatchAndRefresh(patch, 'model creation');
  }, []);

  // Duplicates the source model and sets the copy as the module's reference.
  // Copies all variables and the primary view from the source model.
  const handleDuplicateModelForModule = React.useCallback(
    async (moduleIdent: string, sourceModelName: string): Promise<void> => {
      const project = getProject();
      if (!project) return;

      const sourceModel = project.models.get(sourceModelName);
      if (!sourceModel) return;

      const newModelName = getUniqueDuplicateName(sourceModelName, project);

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
      const currentModel = getModel();
      const existingModule = currentModel?.variables.get(moduleIdent);
      const dupModulePayload: {
        name: string;
        modelName: string;
        references?: { src: string; dst: string }[];
        units?: string;
        documentation?: string;
      } = {
        name: moduleIdent,
        modelName: newModelName,
      };
      if (existingModule && existingModule.type === 'module') {
        if (existingModule.references.length > 0) {
          dupModulePayload.references = existingModule.references.map((ref) => ({ src: ref.src, dst: ref.dst }));
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
            name: modelName(),
            ops: [
              {
                type: 'upsertModule',
                payload: { module: dupModulePayload },
              },
            ],
          },
        ],
      };

      await applyPatchAndRefresh(patch, 'model duplication');
    },
    [],
  );

  const getUniqueDuplicateName = (baseName: string, project: Project): string => {
    let name = `${baseName}_copy`;
    let i = 2;
    while (project.models.has(name)) {
      name = `${baseName}_copy_${i}`;
      i++;
    }
    return name;
  };

  // Renamed from the class method getErrorDetails() to avoid colliding with the
  // module-level getErrorDetails(error) helper (used by handleDownloadXmile).
  const getErrorDetailsPanel = (varDetailsClassName: string): React.ReactElement => {
    const { cachedErrors } = latest.current.state.controllerSnapshot;

    return (
      <div className={varDetailsClassName}>
        <ErrorDetails
          status={latest.current.state.controllerSnapshot.status}
          simError={cachedErrors.simError}
          modelErrors={cachedErrors.modelErrors}
          varErrors={cachedErrors.varErrors}
          varUnitErrors={cachedErrors.unitErrors}
        />
      </div>
    );
  };

  // Decides whether the shared-model info banner shows, and with what label.
  // The banner and the detail panel's banner-aware top inset must agree on this
  // single decision: the banner overlays the top of the same top-right slot as
  // the panels, so when it is present the open panel reserves extra top room to
  // clear it (see .varDetailsWithBanner). Computing the decision once here keeps
  // the two consumers from drifting apart.
  type SharedModelBannerInfo = { visible: false } | { visible: true; label: React.ReactNode };

  const getSharedModelBannerInfo = (): SharedModelBannerInfo => {
    const { modelStack, modelName } = latest.current.state.controllerSnapshot;
    if (modelStack.length === 0) return { visible: false };

    const project = getProject();
    if (!project) return { visible: false };

    // AC4.4: stdlib models show read-only message
    if (isStdlibModel(modelName)) {
      return { visible: true, label: 'Standard library model (read-only)' };
    }

    // AC4.1, AC4.2: count instances
    const count = countModelInstances(project, modelName);

    // AC4.3: single instance shows no banner
    if (count <= 1) return { visible: false };

    return {
      visible: true,
      label: <>This model is used by {count} modules &mdash; changes affect all instances</>,
    };
  };

  // Shows a thin info banner when inside a module whose model is shared
  // by multiple module instances, or when viewing a stdlib model.
  const getSharedModelBanner = (info: SharedModelBannerInfo): React.ReactNode => {
    if (!info.visible) return undefined;
    return <div className={styles.sharedModelBanner}>{info.label}</div>;
  };

  // bannerVisible lifts the open panel's reserved top band (via the
  // .varDetailsWithBanner modifier) so its content clears the shared-model
  // banner that overlays the top of this same slot.
  const getDetails = (bannerVisible: boolean): React.ReactElement | undefined => {
    const { embedded } = props;

    if (embedded) {
      return;
    }

    if (latest.current.state.flowStillBeingCreated) {
      return;
    }

    const varDetailsClassName = clsx(styles.varDetails, bannerVisible && styles.varDetailsWithBanner);

    if (latest.current.state.showDetails === 'errors') {
      return getErrorDetailsPanel(varDetailsClassName);
    }

    const namedElement = getNamedSelectedElement();
    if (!namedElement || latest.current.state.showDetails !== 'variable') {
      return;
    }

    const model = defined(getModel());

    const ident = defined(namedElement.ident);
    const variable = getOrThrow(model.variables, ident);

    if (variable.type === 'module') {
      return (
        <div className={varDetailsClassName}>
          <ModuleDetails
            key={`md-${latest.current.state.controllerSnapshot.projectGeneration}-${ident}`}
            variable={variable}
            viewElement={namedElement}
            project={defined(getProject())}
            currentModelName={modelName()}
            onDelete={handleVariableDelete}
            onModelReferenceChange={handleModuleModelReferenceChange}
            onUnitsDocsChange={handleModuleUnitsDocsChange}
            onDrillIntoModule={handleDrillIntoModule}
            onCreateModel={handleCreateModelForModule}
            onDuplicateModel={handleDuplicateModelForModule}
            onReferencesChange={handleModuleReferencesChange}
          />
        </div>
      );
    }

    const activeTab = latest.current.state.variableDetailsActiveTab;

    return (
      <div className={varDetailsClassName}>
        <VariableDetails
          key={`vd-${latest.current.state.controllerSnapshot.projectGeneration}-${ident}`}
          variable={variable}
          viewElement={namedElement}
          getLatexEquation={getLatexEquation}
          activeTab={activeTab}
          onActiveTabChange={handleVariableDetailsActiveTabChange}
          onDelete={handleVariableDelete}
          onEquationChange={handleEquationChange}
          onTableChange={handleTableChange}
        />
      </div>
    );
  };

  const handleVariableDetailsActiveTabChange = React.useCallback((variableDetailsActiveTab: number): void => {
    setState({ variableDetailsActiveTab });
  }, []);

  const handleVariableDelete = React.useCallback((ident: string): void => {
    const namedElement = getNamedSelectedElement();
    if (!namedElement) {
      return;
    }

    if (namedElement.ident !== ident) {
      return;
    }

    handleSelectionDelete();
  }, []);

  const handleClearSelectedTool = React.useCallback((): void => {
    setState({ selectedTool: undefined });
  }, []);

  const handleSelectStock = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState({
      selectedTool: 'stock',
    });
  }, []);

  const handleSelectFlow = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState({
      selectedTool: 'flow',
    });
  }, []);

  const handleSelectAux = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState({
      selectedTool: 'aux',
    });
  }, []);

  const handleSelectLink = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState({
      selectedTool: 'link',
    });
  }, []);

  const handleSelectModule = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState({
      selectedTool: 'module',
    });
  }, []);

  // Undo/redo is fully owned by the controller: it moves the undo cursor,
  // bumps version/generation synchronously (so the details panels remount),
  // reopens the engine from the restored snapshot, and -- when the restored
  // project no longer contains the viewed model -- resets navigation to 'main'
  // and bumps navResetSeq, which the navReset effect observes to clear the
  // Editor's selection/details/tool UI state.
  const handleUndoRedo = React.useCallback((kind: 'undo' | 'redo'): void => {
    r.controller?.undoRedo(kind);
  }, []);

  const handleZoomChange = React.useCallback(async (newZoom: number): Promise<void> => {
    const view = defined(getView());
    const oldViewBox = view.viewBox;

    const widthAdjust = latest.current.state.showDetails ? panelWidth() : 0;

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
    await handleViewBoxChange(newViewBox, newZoom);
  }, []);

  // True once the unmount cleanup has cleared the controller. The snapshot
  // image's decode/toBlob callbacks are genuinely async and can fire after a
  // route change unmounts the Editor; they bail on this so a setState or a
  // createObjectURL never runs on a dead instance (the unmount-time revoke
  // already ran). This replaces the old `unmounted` flag for the UI-only
  // snapshot path -- engine/save/sim lifecycle is the controller's concern.
  const isUnmounted = (): boolean => {
    return r.controller === undefined;
  };

  const takeSnapshot = (): void => {
    const project = getProject();
    const mName = modelName();
    if (!project || !mName) {
      return;
    }

    const [svg, viewbox] = renderSvgToString(project, mName);
    const osCanvas = document.createElement('canvas');
    osCanvas.width = viewbox.width * 4;
    osCanvas.height = viewbox.height * 4;
    const ctx = exists(osCanvas.getContext('2d'));
    const svgBlob = new Blob([svg], { type: 'image/svg+xml;charset=utf-8' });
    const svgUrl = URL.createObjectURL(svgBlob);

    const image = new Image();
    image.onload = () => {
      // The SVG source URL has served its purpose now that the image is
      // decoded; revoke it so the intermediate blob isn't retained. This must
      // run even when unmounted so the svg blob isn't stranded.
      URL.revokeObjectURL(svgUrl);
      // Image decode is async, so this callback can fire after the Editor has
      // unmounted (e.g. a route change during snapshot generation). Bail
      // before setState/createObjectURL: the unmount cleanup has already run,
      // so a URL created here would never be revoked, and setState on an
      // unmounted component is a no-op warning.
      if (isUnmounted()) {
        return;
      }
      ctx.drawImage(image, 0, 0, viewbox.width * 4, viewbox.height * 4);

      osCanvas.toBlob((snapshotBlob) => {
        // toBlob is itself async; re-check the unmount flag. Crucially, do not
        // create the object URL when unmounted -- no URL has been created at
        // this point, and one created here would leak (the unmount-time
        // revoke already ran).
        if (isUnmounted()) {
          return;
        }
        if (snapshotBlob) {
          // Create the display URL exactly once here (not per render) and
          // revoke any previous snapshot URL via setSnapshotUrl.
          setSnapshotUrl(URL.createObjectURL(snapshotBlob));
        } else {
          setState((prev) => ({
            modelErrors: [...prev.modelErrors, new Error('snapshot creation failed (1).')],
          }));
        }
      });
    };
    image.onerror = () => {
      URL.revokeObjectURL(svgUrl);
      if (isUnmounted()) {
        return;
      }
      setState((prev) => ({
        modelErrors: [...prev.modelErrors, new Error('snapshot creation failed (2).')],
      }));
    };

    image.src = svgUrl;
  };

  // Replace the current snapshot object URL, revoking the previous one so
  // the underlying blob can be garbage-collected. Pass undefined to clear.
  // The live URL is owned by the `liveSnapshotUrl` ref field (read and updated
  // synchronously here, so back-to-back snapshots never both revoke the same
  // stale value); state only mirrors it for render.
  const setSnapshotUrl = (url: string | undefined): void => {
    const previous = r.liveSnapshotUrl;
    if (previous && previous !== url) {
      URL.revokeObjectURL(previous);
    }
    r.liveSnapshotUrl = url;
    setState({ snapshotUrl: url });
  };

  const handleSnapshot = React.useCallback((kind: 'show' | 'close'): void => {
    if (kind === 'show') {
      setTimeout(() => {
        takeSnapshot();
      });
    }
  }, []);
  // handleSnapshot is wired into the (currently commented-out) Snapshotter; keep
  // the reference alive so it isn't flagged as unused while the UI is disabled.
  void handleSnapshot;

  const getMetaActionsBar = (): React.ReactElement | undefined => {
    const { embedded } = props;
    if (embedded) {
      return undefined;
    }

    const zoom = getView()?.zoom || 1;

    return (
      <div className={styles.undoRedoBar}>
        <UndoRedoBar undoEnabled={isUndoEnabled()} redoEnabled={isRedoEnabled()} onUndoRedo={handleUndoRedo} />
        {/*<Snapshotter onSnapshot={handleSnapshot} />*/}
        <ZoomBar zoom={zoom} onChangeZoom={handleZoomChange} />
      </div>
    );
  };

  const getEditorControls = (): React.ReactElement | undefined => {
    const { embedded } = props;
    const { dialOpen, dialVisible, selectedTool } = state;

    if (embedded || isStdlibModel(modelName())) {
      return undefined;
    }

    return (
      <SpeedDial
        ariaLabel="hide or show editor tools"
        className={styles.speedDial}
        hidden={!dialVisible}
        icon={<SpeedDialIcon icon={<EditIcon />} openIcon={<ClearIcon />} />}
        onClick={handleDialClick}
        onClose={handleDialClose}
        open={dialOpen}
      >
        <SpeedDialAction
          icon={<StockIcon />}
          title="Stock"
          onClick={handleSelectStock}
          selected={selectedTool === 'stock'}
        />
        <SpeedDialAction
          icon={<FlowIcon />}
          title="Flow"
          onClick={handleSelectFlow}
          selected={selectedTool === 'flow'}
        />
        <SpeedDialAction
          icon={<AuxIcon />}
          title="Variable"
          onClick={handleSelectAux}
          selected={selectedTool === 'aux'}
        />
        <SpeedDialAction
          icon={<LinkIcon />}
          title="Link"
          onClick={handleSelectLink}
          selected={selectedTool === 'link'}
        />
        <SpeedDialAction
          icon={<ModuleIcon />}
          title="Module"
          onClick={handleSelectModule}
          selected={selectedTool === 'module'}
        />
      </SpeedDial>
    );
  };

  const getSnapshot = (): React.ReactElement | undefined => {
    const { embedded } = props;
    const { snapshotUrl } = state;

    if (embedded || !snapshotUrl) {
      return undefined;
    }

    return (
      <div className={styles.snapshotCard}>
        <div className={styles.snapshotCardContent}>
          <img src={snapshotUrl} className={styles.snapshotImg} alt="diagram snapshot" />
        </div>
        <div className={styles.snapshotCardActions}>
          <Button size="small" color="primary" onClick={handleClearSnapshot}>
            Close
          </Button>
        </div>
      </div>
    );
  };

  const handleClearSnapshot = React.useCallback((): void => {
    setSnapshotUrl(undefined);
  }, []);

  // ---- Render -------------------------------------------------------------
  const { embedded } = props;

  const classNames = clsx(styles.editor, embedded ? '' : styles.editorBg);

  // Compute the shared-model banner decision once so the banner and the detail
  // panel's banner-aware top inset agree. getDetails() is rendered BEFORE
  // getSearchBar() so the opaque search bar paints over the panel's reserved
  // empty top band -- the banner-aware inset only grows that band, preserving
  // the paint-order overlay (it does NOT lift the panel above the search bar).
  const sharedModelBannerInfo = getSharedModelBannerInfo();

  return (
    <div className={classNames}>
      {getDrawer()}
      {getDetails(sharedModelBannerInfo.visible)}
      {getSearchBar()}
      {getSharedModelBanner(sharedModelBannerInfo)}
      {getCanvas()}
      {getSnackbar()}
      {getEditorControls()}
      {getMetaActionsBar()}
      {getSnapshot()}
    </div>
  );
});
