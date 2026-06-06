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
import type {
  JsonProjectPatch,
  JsonModelOperation,
  JsonSimSpecs,
  JsonArrayedEquation,
} from '@simlin/engine';
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
// PureComponent: allocating fresh arrow functions per render would defeat
// its shallow prop comparison and force a full re-render of every layer on
// every Editor render.
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
// so the PureComponent's shallow compare detects updates by identity). The
// remaining fields are genuinely Editor-owned UI/presentation state.
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
  // drawer offers a destructive "Delete project" action that calls this.
  // Resolving means the host has navigated away; rejecting surfaces the
  // error in the confirmation dialog. Hosts without a deletable backing
  // project (the local file-backed viewer, embeds) leave this undefined.
  onDeleteProject?: () => Promise<void>;
}

export type EditorProps = EditorPropsBase & ProjectInputProps;

export class Editor extends React.PureComponent<EditorProps, EditorState> {
  // Stable React keys for snackbar toasts. Keying by `${name}:${message}`
  // collided whenever two distinct errors shared a name and message (e.g.
  // the same engine error raised twice), so React reused one toast's
  // instance for the other and the auto-hide timer of the first dismissed
  // the second prematurely. Assigning a monotonically increasing id per
  // *instance* the first time it is rendered gives every appended error a
  // unique, render-stable key regardless of message text.
  private nextErrorKey = 1;
  private readonly errorKeys = new WeakMap<Error, number>();

  private errorKey(err: Error): number {
    let key = this.errorKeys.get(err);
    if (key === undefined) {
      key = this.nextErrorKey++;
      this.errorKeys.set(err, key);
    }
    return key;
  }

  // Single owner of the live snapshot object URL. State (`snapshotUrl`)
  // only mirrors this for render. Two snapshots completing before the
  // first commit would, if we read the previous URL from state, both see
  // the same stale value and leak one URL; reading and revoking the
  // instance field synchronously in setSnapshotUrl is race-free.
  private liveSnapshotUrl: string | undefined = undefined;

  // The headless coordination layer. Created in componentDidMount and
  // disposed in componentWillUnmount. A mount -> unmount -> mount cycle on
  // the same instance (React 18 StrictMode) therefore creates a *fresh*
  // controller on the second mount -- the first one was disposed -- so no
  // unmounted-flag/timer machinery is needed in the Editor anymore: the
  // controller's own `disposed` latch guards every async continuation it
  // owns. Undefined between unmount and the next mount (and before the first
  // mount); the snapshot in state covers render in those windows.
  private controller: ProjectController | undefined = undefined;
  private unsubscribe: (() => void) | undefined = undefined;
  // Tracks the navResetSeq we last reacted to so componentDidUpdate clears
  // selection exactly once per undo-driven navigation reset.
  private lastNavResetSeq = 0;

  constructor(props: EditorProps) {
    super(props);

    this.state = {
      controllerSnapshot: this.makeController(props).getSnapshot(),
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
    };
    // makeController() above only constructs the controller (side-effect free,
    // engine not yet opened) so the initial snapshot is available for the
    // constructor's state seed. The engine open and subscription are kicked
    // off in componentDidMount -- see there for the StrictMode rationale.
  }

  // Build a fresh ProjectController wired to this Editor's props. Constructing
  // a controller is side-effect free (no engine opened); openInitialProject()
  // is what loads the engine.
  private makeController(props: EditorProps): ProjectController {
    const controller = new ProjectController({
      initialProjectVersion: props.initialProjectVersion,
      input:
        props.inputFormat === 'protobuf'
          ? { format: 'protobuf', data: props.initialProjectBinary }
          : { format: 'json', data: props.initialProjectJson },
      // The concrete engine Project/Model/Run structurally satisfy the
      // controller's EngineApi surface; cast through unknown to bridge the
      // nominal type difference.
      openProtobuf: (data) => EngineProject.openProtobuf(data) as unknown as Promise<EngineApi>,
      openJson: (data) => EngineProject.openJson(data) as unknown as Promise<EngineApi>,
      save: async (project, currVersion) => {
        if (props.inputFormat === 'json') {
          // The controller hands back the format matching inputFormat, so a
          // 'json' input always produces a JsonProjectData payload here.
          return await props.onSave({ format: 'json', data: project.data as string }, currVersion);
        }
        return await props.onSave({ format: 'protobuf', data: project.data as Uint8Array }, currVersion);
      },
      onError: (err) => {
        // Append to the toast list. setState((prev) => ...) so concurrent
        // error reports (e.g. two sim-run failures) don't clobber each other.
        this.setState((prev) => ({ modelErrors: [...prev.modelErrors, err] }));
      },
    });
    this.controller = controller;
    this.lastNavResetSeq = controller.getSnapshot().navResetSeq;
    return controller;
  }

  componentDidMount() {
    // React 18 StrictMode (dev) drives every committed component through
    // componentDidMount -> componentWillUnmount -> componentDidMount on the
    // *same* instance, without re-running the constructor. componentWillUnmount
    // disposes the controller; the second mount must therefore create a fresh
    // one. The constructor's controller is reused on the very first mount and
    // recreated on every subsequent mount.
    if (!this.controller) {
      this.makeController(this.props);
    }
    const controller = defined(this.controller);

    // Mirror controller snapshots into one state field. The PureComponent's
    // shallow compare sees a new snapshot identity and re-renders; an
    // unchanged snapshot is a no-op. Seed state from the (fresh) snapshot in
    // case makeController ran just above.
    this.setState({ controllerSnapshot: controller.getSnapshot() });
    this.unsubscribe = controller.subscribe(() => {
      const c = this.controller;
      if (c) {
        this.setState({ controllerSnapshot: c.getSnapshot() });
      }
    });

    if (this.props.readOnlyMode)
      this.setState({
        modelErrors: [
          ...this.state.modelErrors,
          new Error("This is a read-only version. Any changes you make won't be saved."),
        ],
      });

    document.addEventListener('keydown', this.handleKeyDown);

    // Open the engine, then schedule the first sim run. The controller guards
    // its own dispose-races internally (see ProjectController.dispose), so no
    // Editor-side timer or unmounted flag is needed here.
    void controller.openInitialProject().then(() => {
      this.controller?.scheduleSimRun();
    });
  }

  componentWillUnmount() {
    document.removeEventListener('keydown', this.handleKeyDown);

    // Unsubscribe before disposing so a final controller notification can't
    // setState on an unmounting component, and dispose the controller, which
    // releases the WASM EngineProject handle (the Editor mounts/unmounts on
    // every wouter route change in src/app and every EditorHost path swap in
    // src/simlin-serve; without this every navigation away leaks ~several MB
    // of WASM linear memory plus salsa caches). dispose() is best-effort and
    // latches the controller's `disposed` flag so any in-flight open/undo
    // releases its own engine.
    if (this.unsubscribe) {
      this.unsubscribe();
      this.unsubscribe = undefined;
    }
    const controller = this.controller;
    this.controller = undefined;
    if (controller) {
      // dispose() resolves by contract (it swallows engine-teardown errors),
      // but attach a catch defensively so a rejected teardown can never become
      // an unhandled rejection that crashes the host.
      controller.dispose().catch(() => {});
    }

    // Revoke any outstanding snapshot object URL so navigating away from a
    // project with an open snapshot doesn't strand the blob. Read the live
    // URL from the owning instance field, not state, in case a snapshot
    // completed without its setState having committed yet.
    if (this.liveSnapshotUrl) {
      URL.revokeObjectURL(this.liveSnapshotUrl);
      this.liveSnapshotUrl = undefined;
    }
  }

  componentDidUpdate(_prevProps: EditorProps, prevState: EditorState) {
    // Fire onSelectionChanged whenever the committed selection actually
    // changed. Driving this from componentDidUpdate (rather than a
    // setTimeout(0) inside handleSelection) means the host observes *every*
    // committed selection change -- not just clicks routed through
    // handleSelection, but also selections cleared by a delete and resets on
    // module drill-in/back. (A normal undo/redo preserves the selection and
    // fires nothing; the selection only resets when the viewed model
    // disappears from the restored project -- see the navResetSeq handling
    // below.) getSelectionIdents reads the already-committed
    // `this.state.selection`, so no deferral is needed; componentDidUpdate
    // never fires after unmount.
    if (this.props.onSelectionChanged && !setsEqual(prevState.selection, this.state.selection)) {
      this.props.onSelectionChanged(this.getSelectionIdents());
    }

    // When undo/redo restores a project that no longer contains the viewed
    // model, the controller resets navigation to 'main' and bumps navResetSeq.
    // Clear the Editor's selection/details/tool UI state for that case only
    // (an ordinary undo preserves them). Drill-in / back / level manage the
    // selection through their own handlers, so they do not bump navResetSeq.
    const navResetSeq = this.state.controllerSnapshot.navResetSeq;
    if (navResetSeq !== this.lastNavResetSeq) {
      this.lastNavResetSeq = navResetSeq;
      this.setState({
        selection: new Set<UID>(),
        showDetails: undefined,
        selectedTool: undefined,
      });
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
    return this.state.controllerSnapshot.canUndo;
  }

  private isRedoEnabled(): boolean {
    return this.state.controllerSnapshot.canRedo;
  }

  // Delegating accessor for the active data-model Project. Kept public for
  // tests and the Editor's own render/op-building reads. No external consumer
  // (HostedWebEditor, simlin-serve's EditorHost) uses it.
  project(): Project | undefined {
    return this.state.controllerSnapshot.project;
  }

  // Op-building helpers go through the controller's apply* / view methods, so
  // they generally don't need the raw engine handle. Retained as a delegating
  // accessor (returns undefined before the engine opens / after dispose).
  engine(): EngineProject | undefined {
    return this.controller?.getEngine() as EngineProject | undefined;
  }

  // Convenience wrapper for the simple edit handlers: apply a patch and, on
  // success, refresh from the engine. All engine/save/sim coordination lives
  // in the controller now. Returns false (without refreshing) on patch failure.
  private async applyPatchAndRefresh(patch: JsonProjectPatch, label: string): Promise<boolean> {
    const controller = this.controller;
    if (!controller) {
      return false;
    }
    return await controller.applyPatch(patch, label);
  }

  // Surface a transient error to the toast list. Op-building handlers that
  // detect a problem before reaching the engine (or that report a synchronous
  // failure) call this; the controller surfaces its own errors via onError,
  // which appends to the same list.
  private appendModelError(msg: string): void {
    this.setState((prevState: EditorState) => ({
      modelErrors: [...prevState.modelErrors, new EditorError(msg)],
    }));
  }

  // The active model name lives in the controller snapshot now. Op-building
  // patches target it so operations work at any module nesting depth.
  private modelName(): string {
    return this.state.controllerSnapshot.modelName;
  }

  // Thin delegating wrappers so the Editor's op-building handlers can keep
  // their shape. All engine/save/sim/history coordination lives in the
  // controller. Each is a no-op when no controller is mounted.
  private async applyPatchOrReportError(patch: JsonProjectPatch, label: string): Promise<boolean> {
    const controller = this.controller;
    if (!controller) {
      return false;
    }
    return await controller.applyPatchOrReportError(patch, label);
  }

  private async refreshFromEngine(): Promise<void> {
    await this.controller?.refreshFromEngine();
  }

  private scheduleSimRun(): void {
    this.controller?.scheduleSimRun();
  }

  private async updateView(view: StockFlowView): Promise<void> {
    await this.controller?.updateView(view);
  }

  private async queueViewUpdate(view: StockFlowView): Promise<void> {
    await this.controller?.queueViewUpdate(view);
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
      models: [{ name: this.modelName(), ops }],
    };

    if (!(await this.applyPatchOrReportError(patch, 'rename'))) {
      // A failed rename leaves flowStillBeingCreated untouched.
      return;
    }

    // Clear the in-progress flow-creation flag synchronously after the
    // patch succeeds and BEFORE the engine round-trip in refreshFromEngine.
    // This matches the pre-refactor ordering: the details panel for a
    // just-named flow must un-suppress immediately, not wait out the
    // serialize/JSON/setState round-trip.
    this.setState({
      flowStillBeingCreated: false,
    });
    await this.refreshFromEngine();
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
    // The host's onSelectionChanged callback is no longer fired here. It is
    // fired from componentDidUpdate when the committed selection changes,
    // which covers this path plus every other route that mutates the
    // selection (delete, module drill-in/back, undo/redo). Reading the
    // selection there guarantees it observes the committed state without a
    // setTimeout(0) deferral.
  };

  handleShowVariableDetails = () => {
    this.setState({ showDetails: 'variable' });
  };

  getLatexEquation = async (ident: string): Promise<string | undefined> => {
    const engine = this.engine();
    if (!engine) return undefined;
    try {
      const model = await engine.getModel(this.modelName());
      return (await model.getLatexEquation(ident)) ?? undefined;
    } catch {
      return undefined;
    }
  };

  handleSelectionDelete = async () => {
    const selection = this.state.selection;
    const modelName = this.modelName();
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

    // Parity with the pre-refactor `if (!engine) return`: bail before clearing
    // the selection or running the optimistic view update if the engine hasn't
    // finished opening yet, so a delete attempted in that brief window cleanly
    // no-ops instead of mutating UI state against a project that can't apply it.
    if (!this.controller?.getEngine()) {
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
      // The controller reports any failure via onError; we ignore the boolean
      // here because the view update below must run regardless (matching the
      // original, which committed the cloud/view changes even on a delete-op
      // failure).
      await this.applyPatchOrReportError(patch, 'delete');
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
    const view = defined(this.getView());
    const model = defined(this.getModel());

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
    const selection = result.selection ?? this.state.selection;

    // Preserve the original's early return on a missing engine: it bailed
    // before applying ops or updating the view (no setState, no sim run).
    if (!this.controller?.getEngine()) {
      return;
    }

    if (result.ops.length > 0) {
      const patch: JsonProjectPatch = {
        models: [{ name: this.modelName(), ops: [...result.ops] }],
      };
      // On patch failure, commit the selection/creation flag but DO NOT update
      // the view -- preserving the original behavior exactly.
      if (!(await this.applyPatchOrReportError(patch, 'flow attach'))) {
        this.setState({ selection, flowStillBeingCreated: inCreation });
        return;
      }
    }

    await this.updateView({ ...view, nextUid: result.nextUid, elements: [...result.elements] });
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

  handleCreateVariable = async (element: ViewElement) => {
    const view = defined(this.getView());
    // Parity with the pre-refactor `if (!engine) return`: bail before the
    // optimistic view update if the engine hasn't finished opening yet, so a
    // create attempted in that window cleanly no-ops.
    if (!this.controller?.getEngine()) {
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

    // AC5.2: patch targets this.modelName() (not a hardcoded value), so
    // module creation works at any nesting depth -- navigating into a child
    // model updates modelName, and newly created modules land in that child.
    const patch: JsonProjectPatch = {
      models: [{ name: this.modelName(), ops: [op] }],
    };

    // The controller reports any failure via onError; the view update below
    // runs regardless, matching the original (which committed the new element
    // even when the upsert errored).
    await this.applyPatchOrReportError(patch, 'variable creation');

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
    // The engine is re-checked inside applyPatchAndRefresh; here we only
    // need the project to read the current sim specs.
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

    await this.applyPatchAndRefresh(patch, 'sim specs');
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
      a.download = `${this.props.name}-${this.state.controllerSnapshot.projectVersion | 0}.stmx`;
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

    const model = project.models.get(this.modelName());
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
    const modelName = this.modelName();
    return project.models.get(modelName);
  }

  getView(): StockFlowView | undefined {
    const project = this.project();
    if (!project) {
      return;
    }
    const modelName = this.modelName();
    const model = project.models.get(modelName);
    if (!model) {
      return;
    }

    return model.views[0];
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
    const viewCx = (view.viewBox.width - panelWidth()) / 2 / zoom;

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
    const readOnly = embedded || isStdlibModel(this.modelName());
    const onRenameVariable = !readOnly ? this.handleRename : noopRename;
    const onSetSelection = !embedded ? this.handleSelection : noopSetSelection;
    const onMoveSelection = !readOnly ? this.handleSelectionMove : noopMoveSelection;
    const onMoveFlow = !readOnly ? this.handleFlowAttach : noopMoveFlow;
    const onMoveLabel = !readOnly ? this.handleMoveLabel : noopMoveLabel;
    const onAttachLink = !readOnly ? this.handleLinkAttach : noopAttachLink;
    const onCreateVariable = !readOnly ? this.handleCreateVariable : noopCreateVariable;
    const onClearSelectedTool = !readOnly ? this.handleClearSelectedTool : noop;
    const onDeleteSelection = !readOnly ? this.handleSelectionDelete : noop;
    const onShowVariableDetails = !readOnly ? this.handleShowVariableDetails : noop;
    const onViewBoxChange = !embedded ? this.handleViewBoxChange : noopViewBoxChange;
    const onDrillIntoModule = !embedded ? this.handleDrillIntoModule : noopDrillIntoModule;

    return (
      <Canvas
        embedded={!!embedded}
        project={project}
        model={model}
        view={view}
        version={this.state.controllerSnapshot.projectVersion}
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

  // Remove the single error identified by its per-instance toast id (the
  // same id used as the React key). Filtering by message text instead would
  // dismiss every error sharing that text -- so a repeated failing edit's
  // first auto-hide timer would close all of its duplicate toasts at once.
  handleCloseSnackbar = (id: string | number) => {
    this.setState((prevState) => ({
      modelErrors: prevState.modelErrors.filter((err) => this.errorKey(err) !== id),
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
          {this.state.modelErrors.map((err) => {
            const id = this.errorKey(err);
            return <Toast variant="warning" id={id} onClose={this.handleCloseSnackbar} message={err.message} key={id} />;
          })}
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
    const controller = this.controller;
    const view = this.getView();
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
      this.state.selection,
      view.viewBox,
      view.zoom,
    );
    if (!outcome.restoredSelection) {
      return;
    }
    const newModelName = controller.getModelName();
    this.setState({
      selection: outcome.restoredSelection,
      showDetails: undefined,
      // Clear selected tool when entering a stdlib model (tool palette is hidden)
      selectedTool: isStdlibModel(newModelName) ? undefined : this.state.selectedTool,
    });
  };

  handleNavigateBack = (): void => {
    const controller = this.controller;
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
    this.setState({
      selection: outcome.restoredSelection,
      showDetails: undefined,
    });
  };

  handleNavigateToLevel = (targetLevel: number): void => {
    const controller = this.controller;
    if (!controller) {
      return;
    }
    const outcome = controller.navigateToLevel(targetLevel);
    if (!outcome.restoredSelection) {
      return;
    }
    this.setState({
      selection: outcome.restoredSelection,
      showDetails: undefined,
    });
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
    const readOnly = this.props.embedded || isStdlibModel(this.modelName());
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

    const status = this.state.controllerSnapshot.status;

    return (
      <div className={styles.searchBar}>
        <BreadcrumbBar
          modelStack={this.state.controllerSnapshot.modelStack}
          modelName={this.modelName()}
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
      models: [{ name: this.modelName(), ops: [op] }],
    };

    await this.applyPatchAndRefresh(patch, 'equation update');
  };

  handleTableChange = async (ident: string, newTable: GraphicalFunction | null) => {
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
      models: [{ name: this.modelName(), ops: [op] }],
    };

    await this.applyPatchAndRefresh(patch, 'table update');
  };

  // Updates the model reference for a module variable.
  handleModuleModelReferenceChange = async (ident: string, newModelName: string) => {
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
      models: [{ name: this.modelName(), ops: [op] }],
    };

    await this.applyPatchAndRefresh(patch, 'model reference update');
  };

  // Updates units and/or documentation for a module variable.
  handleModuleUnitsDocsChange = async (ident: string, newUnits: string | undefined, newDocs: string | undefined) => {
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
      models: [{ name: this.modelName(), ops: [op] }],
    };

    await this.applyPatchAndRefresh(patch, 'module update');
  };

  // Updates the input references array for a module variable via upsertModule.
  // The engine does full variable replacement (not merge), so we send the
  // complete module with the new references array.
  handleModuleReferencesChange = async (ident: string, newReferences: ReadonlyArray<ModuleReference>) => {
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
      models: [{ name: this.modelName(), ops: [op] }],
    };

    await this.applyPatchAndRefresh(patch, 'references update');
  };

  // Creates a new empty model and sets it as the module's reference.
  // The engine processes projectOps before model ops (see patch.rs),
  // so AddModel creates the model before upsertModule references it.
  handleCreateModelForModule = async (moduleIdent: string) => {
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
          name: this.modelName(),
          ops: [{ type: 'upsertModule', payload: { module: modulePayload } }],
        },
      ],
    };

    await this.applyPatchAndRefresh(patch, 'model creation');
  };

  // Duplicates the source model and sets the copy as the module's reference.
  // Copies all variables and the primary view from the source model.
  handleDuplicateModelForModule = async (moduleIdent: string, sourceModelName: string) => {
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
          name: this.modelName(),
          ops: [{
            type: 'upsertModule',
            payload: { module: dupModulePayload },
          }],
        },
      ],
    };

    await this.applyPatchAndRefresh(patch, 'model duplication');
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
    const { cachedErrors } = this.state.controllerSnapshot;

    return (
      <div className={styles.varDetails}>
        <ErrorDetails
          status={this.state.controllerSnapshot.status}
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
    const { modelStack, modelName } = this.state.controllerSnapshot;
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
            key={`md-${this.state.controllerSnapshot.projectGeneration}-${ident}`}
            variable={variable}
            viewElement={namedElement}
            project={defined(this.project())}
            currentModelName={this.modelName()}
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
          key={`vd-${this.state.controllerSnapshot.projectGeneration}-${ident}`}
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

  // Undo/redo is fully owned by the controller: it moves the undo cursor,
  // bumps version/generation synchronously (so the details panels remount),
  // reopens the engine from the restored snapshot, and -- when the restored
  // project no longer contains the viewed model -- resets navigation to 'main'
  // and bumps navResetSeq, which componentDidUpdate observes to clear the
  // Editor's selection/details/tool UI state.
  handleUndoRedo = (kind: 'undo' | 'redo'): void => {
    this.controller?.undoRedo(kind);
  };

  handleZoomChange = async (newZoom: number) => {
    const view = defined(this.getView());
    const oldViewBox = view.viewBox;

    const widthAdjust = this.state.showDetails ? panelWidth() : 0;

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

  // True once componentWillUnmount has cleared the controller. The snapshot
  // image's decode/toBlob callbacks are genuinely async and can fire after a
  // route change unmounts the Editor; they bail on this so a setState or a
  // createObjectURL never runs on a dead instance (the unmount-time revoke
  // already ran). This replaces the old `unmounted` flag for the UI-only
  // snapshot path -- engine/save/sim lifecycle is the controller's concern.
  private isUnmounted(): boolean {
    return this.controller === undefined;
  }

  takeSnapshot() {
    const project = this.project();
    const modelName = this.modelName();
    if (!project || !modelName) {
      return;
    }

    const [svg, viewbox] = renderSvgToString(project, modelName);
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
      // before setState/createObjectURL: componentWillUnmount has already run,
      // so a URL created here would never be revoked, and setState on an
      // unmounted component is a no-op warning.
      if (this.isUnmounted()) {
        return;
      }
      ctx.drawImage(image, 0, 0, viewbox.width * 4, viewbox.height * 4);

      osCanvas.toBlob((snapshotBlob) => {
        // toBlob is itself async; re-check the unmount flag. Crucially, do not
        // create the object URL when unmounted -- no URL has been created at
        // this point, and one created here would leak (the unmount-time
        // revoke already ran).
        if (this.isUnmounted()) {
          return;
        }
        if (snapshotBlob) {
          // Create the display URL exactly once here (not per render) and
          // revoke any previous snapshot URL via setSnapshotUrl.
          this.setSnapshotUrl(URL.createObjectURL(snapshotBlob));
        } else {
          this.setState({
            modelErrors: [...this.state.modelErrors, new Error('snapshot creation failed (1).')],
          });
        }
      });
    };
    image.onerror = () => {
      URL.revokeObjectURL(svgUrl);
      if (this.isUnmounted()) {
        return;
      }
      this.setState({
        modelErrors: [...this.state.modelErrors, new Error('snapshot creation failed (2).')],
      });
    };

    image.src = svgUrl;
  }

  // Replace the current snapshot object URL, revoking the previous one so
  // the underlying blob can be garbage-collected. Pass undefined to clear.
  // The live URL is owned by the `liveSnapshotUrl` instance field (read and
  // updated synchronously here, so back-to-back snapshots never both revoke
  // the same stale value); state only mirrors it for render.
  private setSnapshotUrl(url: string | undefined): void {
    const previous = this.liveSnapshotUrl;
    if (previous && previous !== url) {
      URL.revokeObjectURL(previous);
    }
    this.liveSnapshotUrl = url;
    this.setState({ snapshotUrl: url });
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

    if (embedded || isStdlibModel(this.modelName())) {
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
    const { snapshotUrl } = this.state;

    if (embedded || !snapshotUrl) {
      return undefined;
    }

    return (
      <div className={styles.snapshotCard}>
        <div className={styles.snapshotCardContent}>
          <img src={snapshotUrl} className={styles.snapshotImg} alt="diagram snapshot" />
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
    this.setSnapshotUrl(undefined);
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
