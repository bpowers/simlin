// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import TextField from './components/TextField';
import Autocomplete, { type AutocompleteRenderInputParams } from './components/Autocomplete';
import { PortalContainerContext } from './components/portal-container';
import Snackbar from './components/Snackbar';
import { ClearIcon, EditIcon } from './components/icons';
import SpeedDial, { CloseReason, SpeedDialAction, SpeedDialIcon } from './components/SpeedDial';
import Button from './components/Button';
import { canonicalize } from '@simlin/core/canonicalize';

import { Project as EngineProject } from '@simlin/engine';
import type { JsonProjectPatch, JsonModelOperation, JsonSimSpecs, JsonArrayedEquation } from '@simlin/engine';
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
  Rect,
  isNamedViewElement,
  stockToJson,
  flowToJson,
  auxToJson,
  moduleToJson,
  arrayedEquationToJson,
  stockFlowViewToJson,
  type ModuleReference,
} from '@simlin/core/datamodel';
import { defined, exists, setsEqual } from '@simlin/core/common';
import { only } from '@simlin/core/collections';

import { AuxIcon } from './AuxIcon';
import { Toast } from './ErrorToast';
import { FlowIcon } from './FlowIcon';
import { LinkIcon } from './LinkIcon';
import { ModuleIcon } from './ModuleIcon';
import { ModelPropertiesDrawer } from './ModelPropertiesDrawer';
import type { SimSpecField } from './sim-spec-draft';
import { renderSvgToString } from './render-common';
import { Status } from './Status';
import { StockIcon } from './StockIcon';
import { UNDO_REDO_BAR_ATTRIBUTE, UndoRedoBar } from './UndoRedoBar';
import { VariableDetails, type PendingSubmission } from './VariableDetails';
import { ModuleDetails } from './ModuleDetails';
import { ErrorDetails } from './ErrorDetails';
import { ZoomBar } from './ZoomBar';
import { Canvas, type GestureCommit } from './drawing/Canvas';
import { encodeNameNewlines, searchableName } from './drawing/common';
import { sameGeometry } from './gesture-planner';
import { detectUndoRedo, isEditableElement } from './keyboard-shortcuts';
import {
  EDITOR_ROOT_ATTRIBUTE,
  activeEditorRoot,
  editorOwnsKeyEvent,
  markActiveEditorRoot,
  releaseEditorRoot,
} from './editor-key-scope';
import { isStdlibModel } from './module-navigation';
import { countModelInstances } from './module-details-utils';
import { buildModuleReferencePayload } from './module-wiring';
import { relabelVariable } from './rename-ops';
import { planDelete } from './plan-delete';
import { BreadcrumbBar } from './BreadcrumbBar';
import { ProjectController, type ProjectSnapshot, type EngineApi, type Viewport } from './project-controller';

export type { Viewport } from './project-controller';

import styles from './Editor.module.css';
// These must stay in sync with --panel-width-sm/-md/-lg in theme.css (and the
// media-query breakpoints in Editor.module.css).
// Marks the details slot. A press inside it is the panel's own (it blurs and
// commits normally); a press anywhere else flushes the panel's draft first.
const DETAILS_SLOT_ATTRIBUTE = 'data-simlin-details-slot';

const SearchbarWidthSm = 359;
const SearchbarWidthMd = 420;
const SearchbarWidthLg = 480;

// The effective right-panel width at the current viewport, mirroring the
// media queries in Editor.module.css. Used by viewport-centering math, which
// previously hardcoded one width and was wrong at the other breakpoints.
// window.innerWidth is deliberately what is read here: the CSS breakpoints
// are viewport media queries (they size the panel by how much room the USER
// has, not by the editor's box), so this must evaluate the same quantity or
// the centering math and the rendered panel would disagree in an embedded
// editor narrower than the page. Only the panel's overflow CLAMP is
// container-relative (calc(100% - 16px) in the stylesheets).
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
const noopCommitGesture = (_commit: GestureCommit): void => {};
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

/**
 * The React key of the details panel for `variable`. The panels seed their
 * Slate editors once per mount, so the key is what re-seeds them: it changes
 * exactly when the selected variable's committed, user-editable content changes
 * (a landed edit to it, an undo), or the read-only flag flips. It deliberately
 * excludes errors (the highlight is decorated from props), sim data, connector
 * drift, and every global counter, so an unrelated edit landing while the user
 * types does not remount the panel and discard the draft.
 */
export function detailsPanelKey(
  modelName: string,
  elementUid: UID,
  variable: Variable,
  restoreSeq: number,
  readOnly: boolean,
): string {
  // The element (a uid is unique only within its model), not the variable's
  // ident: a rename changes the ident (at once, while it is pending) but none of
  // the content a panel seeds.
  const content =
    variable.type === 'module'
      ? [
          modelName,
          elementUid,
          variable.type,
          variable.modelName,
          variable.references,
          variable.units,
          variable.documentation,
        ]
      : [
          modelName,
          elementUid,
          variable.type,
          variable.equation,
          variable.units,
          variable.documentation,
          variable.type === 'stock' ? undefined : variable.gf,
        ];
  // restoreSeq: restored content can equal the content the panel was seeded
  // from (a draft's edit landed and was undone before a render), and the panel
  // must still drop that text; see ProjectSnapshot.restoreSeq.
  return `${JSON.stringify(content)}-r${restoreSeq}${readOnly ? '-ro' : ''}`;
}

// The project/engine coordination state lives in the ProjectController and is
// mirrored here as a single immutable `controllerSnapshot` field (replaced
// wholesale on every controller change, so a new snapshot identity drives a
// re-render). The remaining fields are genuinely Editor-owned UI/presentation
// state. Held as one useState object (see the function component) with a
// class-like merging setState helper.
interface EditorState {
  // The latest immutable snapshot published by the ProjectController. Holds
  // the rendered project, projectVersion, serverVersion, status, cachedErrors,
  // data, modelName, modelStack, the undo/redo predicates, and the token.
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
  // Whether the open details panel holds a draft (onDraftStateChange); while it
  // does, the panel's key is held (see getDetails).
  panelHasDraft: boolean;
}

// Enforces the Editor's selection invariant for the element-mutation paths
// (delete, create, flow/link attach): when such a path leaves the selection
// EMPTY, the selection-tied variable-details panel must close and the
// variable-details tab must reset. Every one of those setState sites composes
// this (spread into its patch) so the rule holds uniformly instead of each call
// site re-deriving it and drifting apart.
//
// The errors panel is EXEMPT. showDetails is a single field that is either
// 'variable' (a panel bound to the selected variable), 'errors' (the
// model-level error list, independent of any selection), or undefined. Only the
// 'variable' case is tied to the selection, so an emptied selection clears
// showDetails ONLY when it is currently 'variable' -- deleting a variable while
// triaging the error list must not dismiss that list. currentShowDetails is
// therefore an input.
//
// A non-empty selection is returned unchanged: the caller keeps ownership of
// showDetails for the paths that intentionally open a panel. handleSelection's
// empty-click/Escape dismiss and the navigation handlers deliberately close
// EVERYTHING (variable AND errors), so they do NOT route through here.
//
// The bug this closes: the delete/create/attach handlers mutated `selection`
// directly and skipped the variable-panel reset, leaving showDetails at
// 'variable' with nothing selected -- masked only because getDetails() also
// guards on a named selected element, but fragile (a later single-select would
// pop the stale panel open, and any change to that render guard would surface a
// broken panel over an empty selection).
export function selectionStatePatch(
  selection: ReadonlySet<UID>,
  currentShowDetails: 'variable' | 'errors' | undefined,
): {
  selection: ReadonlySet<UID>;
  showDetails?: 'variable' | 'errors' | undefined;
  variableDetailsActiveTab?: number;
} {
  if (selection.size === 0) {
    // Close the variable panel only if it is the one showing; leave an open
    // errors panel (or an already-closed panel) untouched. Reset the tab
    // regardless so the next variable panel opens on its first tab.
    if (currentShowDetails === 'variable') {
      return { selection, showDetails: undefined, variableDetailsActiveTab: 0 };
    }
    return { selection, variableDetailsActiveTab: 0 };
  }
  return { selection };
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
  // Gates the SpeedDial's module-CREATION tool (the only entry point to the
  // module-creation flow). Module wiring/details editing and drill-in
  // navigation are unaffected. Defaults to enabled when omitted; hosts disable
  // it where the still-maturing creation feature should be hidden (e.g. the app
  // turns it off for production builds while keeping it on in development).
  moduleCreationEnabled?: boolean;
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
  // Whether the model-properties drawer shows its "Exit" link to "/" (the
  // project list in the app and simlin-serve). Defaults to true. Hosts that
  // embed the Editor in a page they own (a notebook cell) pass false: there is
  // no route to go to, and the link would pushState on the host page. No
  // router is required to mount the Editor either way.
  showHomeLink?: boolean;
  // Where the Editor's overlay surfaces (the model-properties drawer, dialogs,
  // menus, the autocomplete listbox) render. Default: document.body, where
  // they are viewport-level (position: fixed) -- right for hosts that own the
  // page. A host that gives the Editor one box on a page it does not own (a
  // notebook cell) passes that box: the surfaces render inside it and
  // position against it (drawer from its left edge, dialogs centred in it),
  // so the host's tokens / data-theme / shortcut-scoping attributes on the box
  // reach them and a transformed page ancestor cannot displace them. The
  // element must be positioned (it is the surfaces' containing block). See
  // components/portal-container.ts.
  portalContainer?: HTMLElement;
  // Open the root model's first view at THIS viewport (pan offset + size and
  // zoom) instead of the one stored in the project. Applied like a pan the user
  // just made -- shown from the first frame, round-tripped to the engine as a
  // view-only update, no undo entry, no save -- so the next saved edit
  // persists it. For a host that remounts the Editor on new project bytes
  // while the user is looking at it (the notebook widget on a kernel push): a
  // pan or zoom is never persisted by itself (only the next edit's save
  // carries it), so a remount on the stored bytes would silently reset the
  // user's viewport; the host reads the live one through `onViewportChange`
  // and hands it back here. Hosts that mount once per project (the app,
  // simlin-serve) leave it unset. Ignored when the view is absent or the
  // viewport is unusable (a non-finite coordinate, a non-positive zoom).
  initialViewport?: Viewport;
  // Fires from a post-commit effect with the model name and the COMMITTED
  // viewport of the model being viewed whenever that viewport changes by value:
  // once when the project first renders (the stored viewport, or
  // `initialViewport`, or the mount-time fit), then on each settled pan/zoom/
  // pinch/momentum coast, an idle resize, and module navigation (the child
  // model's viewport, with its name). Never per gesture frame: the canvas owns
  // the live viewport during a gesture and commits it once on settle. A
  // content-equal republished view fires nothing.
  onViewportChange?: (modelName: string, viewport: Viewport) => void;
  // Called by the Reload action of the notice shown once the engine is lost
  // and cannot be reopened (ProjectSnapshot.engineUnavailable). Default: reload
  // the page. A host whose page reload would not reload the project (or would
  // discard more than the Editor) supplies its own.
  onReload?: () => void;
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
  // True between a pointer press inside the editor and its release, wherever
  // the release lands (see the mount effect's window listeners). Undo/redo is
  // refused while it is set: a gesture planned on the current view could not
  // commit once the undo replaced that view.
  gestureLive: boolean;
  // The open details panel's draft commit, registered by the panel (see
  // registerDraftFlush). A canvas press calls it before the gesture starts; it
  // returns true when it submitted a changed draft.
  draftFlush: (() => boolean) | undefined;
  // The latest pending model-only submission per target (model, label,
  // variable), until it settles. A panel's flush on a canvas press and its blur
  // afterwards submit the same draft; a submission identical to the LATEST
  // pending one is not enqueued again. Any other is: a draft changed and changed
  // back (A, B, A) must still land as A, which comparing against every pending
  // submission would drop.
  pendingModelEdits: Map<string, { readonly content: string; readonly landed: Promise<boolean> }>;
  // The latest pending details-panel submission per element (model, uid), by
  // field, until each field's edit settles: a panel reopened meanwhile seeds
  // from it rather than from committed text the submission is replacing.
  pendingPanelSubmissions: Map<string, PendingSubmission>;
  // The details panel's last key and what it was held against (see getDetails).
  heldPanelKey: { readonly base: string; readonly key: string } | undefined;
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
      initialViewport: p.initialViewport,
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
      gestureLive: false,
      draftFlush: undefined,
      pendingModelEdits: new Map<string, { readonly content: string; readonly landed: Promise<boolean> }>(),
      pendingPanelSubmissions: new Map<string, PendingSubmission>(),
      heldPanelKey: undefined,
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
    panelHasDraft: false,
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

  // The Editor's outermost element: the keyboard-scoping identity of this
  // instance (see editor-key-scope.ts).
  const rootRef = React.useRef<HTMLDivElement>(null);

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

    // NOTE: the read-only indication is deliberately NOT appended here. It
    // used to be a toast latched once at mount, which went stale whenever
    // readOnlyMode changed after mount -- an owner whose identity resolves a
    // beat later saw a "read-only" toast over an editable project, and a
    // mid-session flip to read-only showed nothing (issue #935). The
    // indication is now the persistent "View only" pill in getSearchBar(),
    // derived from the CURRENT prop on every render.

    document.addEventListener('keydown', handleKeyDown);
    // A press inside the editor can be released anywhere -- over the host page,
    // outside the browser window (the window loses focus) -- so the release is
    // observed on the window, in the capture phase, where no handler can stop
    // it first.
    window.addEventListener('pointerup', handleGestureRelease, true);
    window.addEventListener('pointercancel', handleGestureRelease, true);
    // A context menu opened by the press can swallow its pointerup.
    window.addEventListener('contextmenu', handleGestureRelease, true);
    window.addEventListener('blur', handleGestureRelease);
    // Captured here (not read in the cleanup): React detaches refs before
    // passive-effect cleanups run, so rootRef.current is null by then.
    const root = rootRef.current;

    // Open the engine. The open item requests the first sim run, error refresh
    // and connector check itself, and the controller guards its own
    // dispose-races (see ProjectController.dispose), so no Editor-side timer or
    // unmounted flag is needed here.
    void controller.openInitialProject();

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
      window.removeEventListener('pointerup', handleGestureRelease, true);
      window.removeEventListener('pointercancel', handleGestureRelease, true);
      window.removeEventListener('contextmenu', handleGestureRelease, true);
      window.removeEventListener('blur', handleGestureRelease);
      if (root) {
        // A key on <body> must never resolve to an unmounted instance.
        releaseEditorRoot(root);
      }

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

  // ---- Post-commit effect: onViewportChange --------------------------------
  // Report the committed viewport of the viewed model whenever it changes by
  // VALUE (offset, size or zoom), including the first render that has a view.
  // Keyed on the controller snapshot: every viewport commit -- a settled
  // gesture's setViewport, the mount-time fit, an idle resize, module
  // navigation's viewport restore -- publishes a new snapshot, and the
  // prev-value ref keeps content-equal republishes (a content edit, a save
  // acknowledgment) silent. The Canvas holds a gesture's live viewport locally
  // and commits once on settle, so this never fires per frame.
  // The last reported value, held as a flat record of its own (never the object
  // handed to the host, which the host may keep or mutate).
  const prevViewportRef = React.useRef<
    { modelName: string; x: number; y: number; width: number; height: number; zoom: number } | undefined
  >(undefined);
  React.useEffect(() => {
    const snapshot = state.controllerSnapshot;
    const view = snapshot.project?.models.get(snapshot.modelName)?.views[0];
    if (!view) {
      return;
    }
    const cur = {
      modelName: snapshot.modelName,
      x: view.viewBox.x,
      y: view.viewBox.y,
      width: view.viewBox.width,
      height: view.viewBox.height,
      zoom: view.zoom,
    };
    const prev = prevViewportRef.current;
    if (
      prev &&
      prev.modelName === cur.modelName &&
      prev.x === cur.x &&
      prev.y === cur.y &&
      prev.width === cur.width &&
      prev.height === cur.height &&
      prev.zoom === cur.zoom
    ) {
      return;
    }
    prevViewportRef.current = cur;
    latest.current.props.onViewportChange?.(cur.modelName, {
      viewBox: { x: cur.x, y: cur.y, width: cur.width, height: cur.height },
      zoom: cur.zoom,
    });
  }, [state.controllerSnapshot]);

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
      // An undo-driven navigation reset (the viewed model vanished) is a
      // full reset, not an element mutation: close EVERYTHING explicitly rather
      // than routing through selectionStatePatch (which would preserve an open
      // errors panel).
      setState({
        selection: new Set<UID>(),
        showDetails: undefined,
        selectedTool: undefined,
      });
    }
  }, [navResetSeq]);

  // ---- Post-commit effect: readOnlyMode flips ------------------------------
  // readOnlyMode can change while the Editor is mounted, in both directions:
  // the app derives it from the resolved user identity, so an owner deep-link
  // starts read-only and flips editable a beat later, and a session change can
  // flip the other way. Most consequences are render-derived from the current
  // prop (the pill, hidden toolbars, noop canvas handlers, read-only panels),
  // but the armed creation tool and the flow-creation staging are STATE, so a
  // flip to read-only must clear them explicitly -- otherwise flipping back to
  // editable would silently resurrect a tool armed before the flip, and the
  // suppressed details panel would stay suppressed. Selection is deliberately
  // preserved (it is a read capability). Guarded by a prev-value ref so this
  // fires on change, not on mount (docs/dev/typescript.md).
  const readOnlyModeNow = !!props.readOnlyMode;
  const prevReadOnlyModeRef = React.useRef(readOnlyModeNow);
  React.useEffect(() => {
    if (prevReadOnlyModeRef.current === readOnlyModeNow) {
      return;
    }
    prevReadOnlyModeRef.current = readOnlyModeNow;
    if (readOnlyModeNow) {
      setState({ selectedTool: undefined, dialOpen: false, flowStillBeingCreated: false });
    }
  }, [readOnlyModeNow]);

  // ---- Handlers (formerly bound class methods) ----------------------------
  // Each is wrapped in useCallback([]) so its identity is stable across
  // renders -- exactly as the class's bound methods were -- which preserves the
  // memoization of the React.memo'd children they are passed to (Canvas,
  // Status, UndoRedoBar, ZoomBar). They read CURRENT props/state through
  // `latest`/`r`, never a stale render closure, so empty deps is correct.

  const handleKeyDown = React.useCallback((e: KeyboardEvent): void => {
    const { props: p, state } = latest.current;
    // Don't handle shortcuts in embedded mode or editable fields (text
    // inputs, the equation editor, and the canvas's inline name editor are
    // all contenteditable/inputs, so typing there never triggers these).
    if (p.embedded || isEditableElement(e.target)) {
      return;
    }
    // Several Editors can share one document (a notebook with an Editor per
    // cell). This listener is document-level, so it acts only when the event
    // belongs to THIS instance: its target is inside our root, or focus is
    // nowhere (<body>) and we are the instance that most recently saw pointer
    // or focus activity. See editor-key-scope.ts for the full decision.
    const root = rootRef.current;
    if (!root || !editorOwnsKeyEvent(root, e.composedPath(), activeEditorRoot())) {
      return;
    }

    const action = detectUndoRedo(e);
    if (action) {
      // Undo/redo MUTATE the project (they move the history cursor and reopen
      // the engine from a snapshot), so a read-only viewer gets neither; the
      // UndoRedoBar is hidden alongside (getMetaActionsBar). Project-scoped:
      // gated on readOnlyMode, not isReadOnly, so undoing a parent-model edit
      // while VIEWING a stdlib model stays available (see isReadOnly's doc).
      const isEnabled = !p.readOnlyMode && (action === 'undo' ? isUndoEnabled() : isRedoEnabled());
      if (isEnabled) {
        e.preventDefault();
        handleUndoRedo(action);
      }
      return;
    }

    // Escape: disarm the active creation tool first; with no tool armed,
    // clear the selection (which also closes the details panel).
    if (e.key === 'Escape') {
      if (state.selectedTool !== undefined) {
        setState({ selectedTool: undefined });
      } else if (state.selection.size > 0) {
        handleSelection(new Set<UID>());
      }
      return;
    }

    // Delete/Backspace: delete the current selection. This is the only
    // delete affordance for unnamed elements (clouds) and for elements whose
    // details panel cannot open, so it must not depend on the panel.
    if (e.key === 'Delete' || e.key === 'Backspace') {
      if (!isReadOnly() && state.selection.size > 0) {
        e.preventDefault();
        void handleSelectionDelete();
      }
      return;
    }
  }, []);

  // Pointer presses and focus entering this instance (including its portaled
  // drawer/dialog/menus, which React routes through this tree) make it the
  // target of the next key event that lands on <body>. Capture-phase handlers
  // so a child that stops propagation (the Canvas does, on pointerdown) cannot
  // hide the activity.
  const handleActivity = React.useCallback((): void => {
    const root = rootRef.current;
    if (root) {
      markActiveEditorRoot(root);
    }
  }, []);

  // A pointer press anywhere inside the editor moves focus INTO it. Capture
  // phase runs before the browser's own mousedown default action, so a press
  // on a focusable target (a text field, a button) still ends with focus on
  // that target; a press whose default is prevented (the Canvas prevents it
  // on every pointerdown, so element clicks and drags never focus anything)
  // leaves focus on the root -- inside the editor, never on the host page or
  // <body>. Without this, clicking a variable in a notebook cell left focus on
  // the notebook cell and the notebook's own shortcuts (`d d` deletes the
  // CELL) fired instead of the Editor's; the mechanism is host-agnostic
  // (see the keyboard scoping contract in CLAUDE.md). Only when focus is not
  // already inside the editor: a press inside an editable while it is focused
  // must not blur it (its blur commits), and moving focus root-ward for
  // nothing would drop the caret.
  const handlePointerDownCapture = React.useCallback((e: React.PointerEvent<HTMLDivElement>): void => {
    handleActivity();
    r.gestureLive = true;
    // A press outside the details panel does not blur the panel's editors when
    // its default is prevented (the Canvas prevents every press), so flush the
    // draft now: its edit is enqueued ahead of whatever the press starts.
    // Capture phase runs before the Canvas's own handler. The undo/redo controls
    // are exempt: a draft flushed on their press would queue an edit, and undo
    // refuses while one is queued, so the click would do nothing. handleUndoRedo
    // flushes the draft itself and queues the undo behind it.
    const target = e.target as Element | null;
    const exempt =
      typeof target?.closest === 'function' &&
      target.closest(`[${DETAILS_SLOT_ATTRIBUTE}], [${UNDO_REDO_BAR_ATTRIBUTE}]`) !== null;
    if (!exempt) {
      r.draftFlush?.();
    }
    const root = rootRef.current;
    if (!root) {
      return;
    }
    const active = document.activeElement;
    // Portaled surfaces (drawer, dialog, menu, listbox) are not DOM
    // descendants of the root, so root.contains() says no for a press inside
    // one; React routes the press through this tree, and the DOM path of the
    // press tells whether the focused element is the pressed element or one
    // of its ancestors -- the drawer panel is focused and the user presses a
    // field inside it -- in which case focus is left where it is (the panel's
    // own focus management owns it). <body>/<html> are on every path and
    // count as focus nowhere.
    const path = e.nativeEvent.composedPath();
    const focusInsideEditor = active !== null && root.contains(active);
    const focusIsPressedOrAncestor =
      active !== null && active !== document.body && active !== document.documentElement && path.includes(active);
    if (!focusInsideEditor && !focusIsPressedOrAncestor) {
      root.focus({ preventScroll: true });
    }
  }, []);

  const handleGestureRelease = React.useCallback((): void => {
    r.gestureLive = false;
  }, []);

  // The panel registers its draft commit; the unregistration only clears a
  // registration that is still its own, so a remounting panel's cleanup cannot
  // drop the new mount's registration.
  const registerDraftFlush = React.useCallback((flush: () => boolean): (() => void) => {
    r.draftFlush = flush;
    return () => {
      if (r.draftFlush === flush) {
        r.draftFlush = undefined;
      }
    };
  }, []);

  // The controller's CURRENT snapshot. Handlers plan edits on it rather than on
  // the mirror in React state, which lags a render behind an enqueue: two
  // handlers in one tick would otherwise both plan on the view before the
  // first, and the second full-replacement view would drop the first edit.
  const currentSnapshot = (): ProjectSnapshot => {
    return r.controller?.getSnapshot() ?? latest.current.state.controllerSnapshot;
  };

  const isUndoEnabled = (): boolean => {
    return currentSnapshot().canUndo;
  };

  const isRedoEnabled = (): boolean => {
    return currentSnapshot().canRedo;
  };

  // The rendered data-model Project (committed content plus pending edits).
  // Named getProject (the class method was project()) to avoid colliding with
  // the many `const project = ...` locals.
  const getProject = (): Project | undefined => {
    return currentSnapshot().project;
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

  // The active model name lives in the controller snapshot. Edits target it so
  // operations work at any module nesting depth.
  const modelName = (): string => {
    return currentSnapshot().modelName;
  };

  // The active MODEL cannot be edited and its details panels must not open:
  // stdlib models are immutable library code, and embedded mode renders an
  // inert diagram with no chrome. Distinct from isReadOnly below because a
  // read-only VIEWER (readOnlyMode) keeps inspection -- opening the details
  // panels to READ stays available -- so panel-open affordances gate on THIS
  // predicate while mutation affordances gate on isReadOnly.
  const isModelLocked = (): boolean => {
    return !!latest.current.props.embedded || isStdlibModel(modelName());
  };

  // THE editor-wide mutation gate (issue #935): readOnlyMode (viewing someone
  // else's project -- persistence is a host-side no-op, so local edits would
  // silently evaporate) OR a locked model. Every handler and affordance that
  // can change project content consults this: the canvas handlers and
  // creation tools, keyboard delete, rename, the details-panel edit callbacks,
  // and module wiring. The op-building handlers ALSO check it internally as
  // defense in depth, so no future wiring mistake can reopen the gap.
  // Deliberately preserved as capabilities: selection, pan/zoom, drill-in
  // navigation, opening details to read, snapshot/download, and sim runs
  // (running a simulation is analysis -- results live outside the serialized
  // project). Undo/redo and sim-specs are PROJECT-scoped and gate on
  // readOnlyMode alone (see handleUndoRedo/applySimSpecChange): while viewing
  // a stdlib model in your own project they remain legitimately available.
  const isReadOnly = (): boolean => {
    return !!latest.current.props.readOnlyMode || isModelLocked();
  };

  // The gate of every handler that enqueues a view edit: the mutation gate, plus
  // a queued undo/redo. An edit planned now would be planned on the view the
  // undo replaces, so the controller refuses it; the handler refuses first, so
  // its UI side effects (a cleared selection, an un-suppressed panel) do not
  // happen either.
  const viewEditsRefused = (): boolean => {
    return isReadOnly() || viewEditRefusal() !== undefined;
  };

  // Why a view edit cannot be made now, for the handlers whose refusal must be
  // visible: a create or rename commits a typed name, and returning a message
  // keeps the Canvas's name editor open, where returning nothing would close it
  // and drop the name.
  const viewEditRefusal = (): string | undefined => {
    const snapshot = currentSnapshot();
    if (snapshot.engineUnavailable) {
      return 'The project cannot be edited until it is reloaded';
    }
    if (snapshot.undoRedoQueued) {
      return 'Wait for the undo or redo to finish';
    }
    return undefined;
  };

  // A diagram edit: `nextView` renders at once and the controller later applies
  // the model ops implied by (rendered view -> nextView) plus the view,
  // atomically; a failure rolls the diagram back. No-op without a controller.
  const enqueueViewEdit = (label: string, nextView: StockFlowView): void => {
    void r.controller?.enqueueViewEdit({ label, nextView });
  };

  // The uid of the rendered element naming `ident` in the active model.
  const renderedElementUid = (ident: string): UID | undefined => {
    return getView()?.elements.find((el) => isNamedViewElement(el) && el.ident === ident)?.uid;
  };

  // The committed variable an edit enqueued for rendered element `uid` (named
  // `ident` when enqueued) targets at dequeue: the variable the element names on
  // the COMMITTED view. A rename may have landed since the edit was enqueued, or
  // be pending then and rolled back since, so the ident as enqueued can be stale
  // either way; the element's uid is not. An element absent from the committed
  // view (its create was rolled back, or it was deleted) resolves to nothing.
  // `ident` is the lookup only when no rendered element named it.
  const committedVariable = (
    committed: Project,
    mName: string,
    uid: UID | undefined,
    ident: string,
  ): Variable | undefined => {
    const model = committed.models.get(mName);
    if (uid === undefined) {
      return model?.variables.get(ident);
    }
    const element = model?.views[0]?.elements.find((el) => el.uid === uid);
    return element !== undefined && isNamedViewElement(element)
      ? model?.variables.get(canonicalize(element.name))
      : undefined;
  };

  // A model-only edit to variable `ident` of the active model. The payload is
  // built at dequeue from the COMMITTED variable (see committedVariable), so
  // echoed fields (a stock's inflows, a module's references) are never stale; a
  // variable that no longer exists fails the item. An identical edit still
  // pending is not enqueued twice (a panel's canvas-press flush and its later
  // blur submit the same draft).
  // Resolves whether the edit landed; a submission identical to the latest
  // pending one resolves as that one does.
  const enqueueVariableEdit = (
    label: string,
    ident: string,
    content: unknown,
    build: (variable: Variable, committed: Project, mName: string) => JsonProjectPatch,
  ): Promise<boolean> => {
    const controller = r.controller;
    if (!controller) {
      return Promise.resolve(false);
    }
    const mName = modelName();
    const target = JSON.stringify([mName, label, ident]);
    const pending = r.pendingModelEdits.get(target);
    const contentKey = JSON.stringify(content);
    if (pending?.content === contentKey) {
      return pending.landed;
    }
    const uid = renderedElementUid(ident);
    const submission: { content: string; landed: Promise<boolean> } = {
      content: contentKey,
      landed: Promise.resolve(false),
    };
    submission.landed = controller
      .enqueueModelEdit({
        label,
        buildPatch: (committed) => {
          const variable = committedVariable(committed, mName, uid, ident);
          if (variable === undefined) {
            throw new EditorError(`${label} failed: '${ident}' no longer exists`);
          }
          return build(variable, committed, mName);
        },
      })
      .finally(() => {
        if (r.pendingModelEdits.get(target) === submission) {
          r.pendingModelEdits.delete(target);
        }
      });
    r.pendingModelEdits.set(target, submission);
    return submission.landed;
  };

  const handleDialClick = React.useCallback((_event: React.MouseEvent<HTMLButtonElement>): void => {
    // Toggle the palette open/closed. Closing it deselects the active tool so the
    // canvas returns to plain selection mode -- matching the load-time state where
    // no tool is selected (the clearing was dropped in 5191a9b6 and is restored
    // here). Opening leaves any selected tool untouched.
    setState((prev) => ({
      dialOpen: !prev.dialOpen,
      selectedTool: prev.dialOpen ? undefined : prev.selectedTool,
    }));
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

  const handleRename = React.useCallback((oldName: string, newName: string): string | undefined => {
    // Defense in depth (issue #935): the canvas wiring already substitutes
    // no-ops when read-only, but every op-building handler re-checks so a
    // stale-closure commit (e.g. a blur that lands after a flip to read-only)
    // can never mutate the project. Same guard on every mutation handler below.
    if (isReadOnly() || oldName === newName) {
      return undefined;
    }
    const pausedRefusal = viewEditRefusal();
    if (pausedRefusal !== undefined) {
      return pausedRefusal;
    }
    const controller = r.controller;
    const view = getView();
    if (!controller || !view) {
      return undefined;
    }
    // Refuse a name another variable (or a pending create) already has: the
    // inline editor stays open with the message and nothing is enqueued.
    const refusal = controller.nameError(encodeNameNewlines(newName), canonicalize(encodeNameNewlines(oldName)));
    if (refusal !== undefined) {
      return refusal;
    }
    // RenameVariable never renames view elements, so a rename is an edit WITH a
    // next view: the rendered view with the element relabeled. The controller
    // derives renameVariable from the relabeled element (the typed name raw,
    // issue #906).
    enqueueViewEdit('rename', relabelVariable(view, oldName, newName));
    // The details panel for a just-named flow un-suppresses now, with the
    // optimistic rename, not once the edit lands.
    setState({
      flowStillBeingCreated: false,
    });
    return undefined;
  }, []);

  const handleSelection = React.useCallback((selection: ReadonlySet<UID>): void => {
    setState({
      selection,
      flowStillBeingCreated: false,
      variableDetailsActiveTab: 0,
    });
    // An empty selection here is a deliberate dismiss gesture (clicking empty
    // canvas, Escape, clearing the search box), so close EVERYTHING -- the
    // variable panel AND the model-level errors panel. This is intentionally
    // broader than selectionStatePatch (which the element-mutation paths use so
    // an emptied selection preserves an open errors panel); the two behaviors
    // are deliberately distinct, so this path does NOT route through the helper.
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
    const controller = r.controller;
    if (!controller) {
      return undefined;
    }
    const mName = modelName();
    // Through the executor, so the query never runs against an engine an undo is
    // swapping out.
    const latex = await controller.query(async (engine) => {
      const model = (await engine.getModel(mName)) as unknown as {
        getLatexEquation(ident: string): Promise<string | null | undefined>;
      };
      return (await model.getLatexEquation(ident)) ?? undefined;
    });
    return latex ?? undefined;
  }, []);

  const handleSelectionDelete = React.useCallback((): void => {
    if (viewEditsRefused()) {
      return;
    }
    const selection = latest.current.state.selection;
    const view = getView();
    if (!r.controller || !view || selection.size === 0) {
      return;
    }
    // planDelete removes the selection, the links and aliases touching it, and
    // the clouds of deleted flows, and turns endpoints on deleted stocks into
    // clouds; the controller derives deleteVariable and the stock list ops from
    // the view difference.
    const nextView = planDelete(view, selection);
    // Clear the selection in the same synchronous block as the optimistic view,
    // so React batches them into a single render: no consumer should ever
    // observe a selection that references an element the view no longer
    // contains. selectionStatePatch also closes the variable panel (but not an
    // open errors panel) and resets the tab.
    setState(selectionStatePatch(new Set<number>(), latest.current.state.showDetails));
    enqueueViewEdit('delete', nextView);
  }, []);

  // A canvas gesture's commit (docs/design-plans/2026-09-10-diagram-editing-core.md,
  // E2): the planner's elements are the next view exactly as the preview showed
  // them, and the controller derives every model op from the view difference at
  // dequeue. The token is the one the gesture was pressed under, so a truncation
  // or undo that landed meanwhile drops the edit rather than applying it to a
  // view it was not planned on.
  const handleCommitGesture = React.useCallback((commit: GestureCommit): void => {
    if (viewEditsRefused()) {
      return;
    }
    const view = getView();
    if (!r.controller || !view || !sameGeometry(commit.baseView, view)) {
      // A commit planned on a view another edit has since replaced is dropped
      // quietly, as the Canvas drops a gesture whose view changed under it (E5).
      return;
    }
    void r.controller.enqueueViewEdit({
      label: commit.label,
      nextView: { ...view, nextUid: commit.nextUid, elements: [...commit.elements] },
      token: commit.token,
    });
    // A drawn flow hands off to its name editor; flowStillBeingCreated keeps its
    // details panel closed until it is named (handleRename clears it). If the
    // edit fails, the rolled-back view no longer holds the selected flow, which
    // the Canvas tolerates (the name editor resolves nothing and closes).
    setState({
      ...selectionStatePatch(commit.selection, latest.current.state.showDetails),
      flowStillBeingCreated: commit.editName !== undefined,
    });
  }, []);

  const handleCreateVariable = React.useCallback((element: ViewElement): string | undefined => {
    if (isReadOnly()) {
      return undefined;
    }
    const pausedRefusal = viewEditRefusal();
    if (pausedRefusal !== undefined) {
      return pausedRefusal;
    }
    const controller = r.controller;
    const view = getView();
    if (!controller || !view || !isNamedViewElement(element)) {
      return undefined;
    }
    // A typed name that another variable or a pending create already has keeps
    // the name editor open with the message: the engine's upsert would silently
    // replace that variable, whatever its kind.
    const refusal = controller.nameError(element.name, undefined);
    if (refusal !== undefined) {
      return refusal;
    }
    // The created element names a variable that does not exist yet; the
    // controller derives its upsert from the view difference. The edit targets
    // the active model (AC5.2), so modules created while drilled in land there.
    // The Canvas stages the element under its default name's ident; the element
    // takes the typed name's, so every rendered element's ident is its name's.
    enqueueViewEdit('variable creation', {
      ...view,
      nextUid: view.nextUid + 1,
      elements: [...view.elements, { ...element, uid: view.nextUid, ident: canonicalize(element.name) }],
    });
    setState(selectionStatePatch(new Set<number>(), latest.current.state.showDetails));
    return undefined;
  }, []);

  const handleDrawerToggle = React.useCallback((isOpen: boolean): void => {
    setState({
      drawerOpen: isOpen,
    });
  }, []);

  const applySimSpecChange = (updates: Partial<JsonSimSpecs>): void => {
    // Sim specs are PROJECT content: read-only viewers cannot change them
    // (the drawer also renders its fields disabled). Gated on readOnlyMode,
    // not isReadOnly -- editing your own project's sim specs while viewing a
    // stdlib model remains legitimate.
    if (latest.current.props.readOnlyMode) {
      return;
    }
    void r.controller?.enqueueModelEdit({
      label: 'sim specs',
      // setSimSpecs replaces every field, so the untouched ones are echoed from
      // the COMMITTED specs at dequeue, never from specs read before an earlier
      // commit landed.
      buildPatch: (committed) => {
        const simSpec = committed.simSpecs;
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
        return { projectOps: [{ type: 'setSimSpecs', payload: { simSpecs } }] };
      },
    });
  };

  // The drawer holds a draft while a sim-specs field is focused and calls this
  // exactly once when the field settles (blur/Enter) with a validated, changed
  // value -- so a single edit is one engine patch and one undo entry, instead
  // of one per keystroke (issue #55). The drawer already rejected garbage/empty
  // input, so we only route the field to its `applySimSpecChange` update.
  const handleSimSpecCommit = React.useCallback((field: SimSpecField, value: number | string): void => {
    switch (field) {
      case 'startTime':
        void applySimSpecChange({ startTime: value as number });
        break;
      case 'stopTime':
        void applySimSpecChange({ endTime: value as number });
        break;
      case 'dt':
        void applySimSpecChange({ dt: `${value as number}` });
        break;
      case 'timeUnits':
        void applySimSpecChange({ timeUnits: value as string });
        break;
    }
  }, []);

  const handleDownloadXmile = React.useCallback(async (): Promise<void> => {
    const controller = r.controller;
    if (!controller) {
      return;
    }
    const result = await controller.query(async (engine): Promise<{ xmile: string } | { error: unknown }> => {
      try {
        return { xmile: await (engine as unknown as EngineProject).toXmileString() };
      } catch (error: unknown) {
        return { error };
      }
    });
    if (result === undefined) {
      return;
    }
    try {
      if ('error' in result) {
        throw result.error;
      }
      const xmile = result.xmile;
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
      // Stamp the filename with the server-acknowledged version: the
      // fractional projectVersion is a render-cache key whose integer part
      // drifts with unsaved local edits (#958).
      a.download = `${latest.current.props.name}-${latest.current.state.controllerSnapshot.serverVersion}.stmx`;
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
        onSimSpecCommit={handleSimSpecCommit}
        onDownloadXmile={handleDownloadXmile}
        onDelete={onDelete}
        // Sim specs are project content; the download stays available (it is
        // a read). Project-scoped like undo/redo: readOnlyMode, not isReadOnly.
        readOnly={!!latest.current.props.readOnlyMode}
        showHomeLink={latest.current.props.showHomeLink}
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

  // Viewport changes (a settled pan/zoom, a resize, the mount fit, centering)
  // render at once and persist through a controller viewport item: no undo
  // entry, no save.
  const handleViewBoxChange = React.useCallback((viewBox: Rect, zoom: number): void => {
    r.controller?.setViewport(modelName(), { viewBox, zoom });
  }, []);

  const centerVariable = (element: ViewElement): void => {
    const view = getView();
    if (!view) {
      return;
    }
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

    handleViewBoxChange(viewBox, zoom);
  };

  const handleNewVariableName = React.useCallback((base: string): string => {
    return r.controller?.newVariableName(base) ?? base;
  }, []);

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

    // The unified mutation gate (issue #935): read-only viewers, embeds, and
    // stdlib models all get no-op mutation handlers while keeping selection,
    // viewbox, and drill-in navigation active. Opening the details panel is
    // gated on isModelLocked, NOT isReadOnly: a read-only viewer may still
    // open it for inspection (the panel itself renders read-only).
    const readOnly = isReadOnly();
    const onRenameVariable = !readOnly ? handleRename : noopRename;
    const onSetSelection = !embedded ? handleSelection : noopSetSelection;
    const onCommitGesture = !readOnly ? handleCommitGesture : noopCommitGesture;
    const onCreateVariable = !readOnly ? handleCreateVariable : noopCreateVariable;
    const onClearSelectedTool = !readOnly ? handleClearSelectedTool : noop;
    const onDeleteSelection = !readOnly ? handleSelectionDelete : noop;
    const onShowVariableDetails = !isModelLocked() ? handleShowVariableDetails : noop;
    const onViewBoxChange = !embedded ? handleViewBoxChange : noopViewBoxChange;
    const onDrillIntoModule = !embedded ? handleDrillIntoModule : noopDrillIntoModule;

    return (
      <Canvas
        embedded={!!embedded}
        readOnly={readOnly}
        // A carried viewport (a remounting host handing back the user's live
        // framing) is not the offscreen-recovery case: never yank it back.
        recenterOffscreenOnMount={latest.current.props.initialViewport === undefined}
        project={project}
        model={model}
        view={view}
        token={latest.current.state.controllerSnapshot.token}
        selectedTool={readOnly ? undefined : latest.current.state.selectedTool}
        selection={latest.current.state.selection}
        onRenameVariable={onRenameVariable}
        onSetSelection={onSetSelection}
        onCommitGesture={onCommitGesture}
        onCreateVariable={onCreateVariable}
        onClearSelectedTool={onClearSelectedTool}
        onDeleteSelection={onDeleteSelection}
        onShowVariableDetails={onShowVariableDetails}
        onViewBoxChange={onViewBoxChange}
        onDrillIntoModule={onDrillIntoModule}
        newVariableName={handleNewVariableName}
        pressesDisabled={latest.current.state.controllerSnapshot.undoRedoQueued}
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
    // Navigation is a full context switch: close EVERYTHING (variable AND
    // errors) explicitly rather than routing through selectionStatePatch.
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
    // Navigation is a full context switch: close EVERYTHING explicitly (the
    // restored parent selection may be non-empty), not via selectionStatePatch.
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
    // Same as navigateBack: a full context switch that closes EVERYTHING
    // explicitly, not via selectionStatePatch.
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
      if (element) {
        // Locked models (stdlib, embedded) never open the details panel; the
        // Canvas-level guard handles double-click, but search bypasses it.
        // A read-only VIEWER (readOnlyMode) deliberately still opens it: the
        // panel renders read-only, and inspection is a preserved capability.
        //
        // Only open the panel when the search resolved to an element. When it
        // did not, handleSelection already cleared the selection (and, via its
        // own empty-selection close-all, showDetails); setting showDetails here
        // would strand a 'variable' panel over an empty selection.
        setState({
          showDetails: isModelLocked() ? undefined : 'variable',
        });
        centerVariable(element);
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

    // The persistent read-only indication (issue #935). Derived from the
    // CURRENT readOnlyMode prop each render -- never latched -- so it appears
    // and disappears exactly when the mode flips (the old mount-latched toast
    // went stale for owners whose identity resolved after mount). It lives
    // IN-FLOW inside the search bar's flex row, so it cannot collide with the
    // save-failure banner or the shared-model banner (both sit BELOW the bar
    // at --shared-model-banner-top) or the details panels; the search box
    // simply flexes narrower to make room. role="status" (a polite live
    // region) means a mid-session flip to read-only is announced by assistive
    // tech when the pill appears; the aria-label carries the full explanation
    // so it is not tooltip-only, and the title stays as a pointer-hover
    // supplement for sighted users.
    const viewOnlyPill = latest.current.props.readOnlyMode ? (
      <span
        className={styles.viewOnlyPill}
        role="status"
        aria-label="View only: you are viewing someone else's project, and changes are disabled."
        title="You are viewing someone else's project. Changes are disabled."
      >
        View only
      </span>
    ) : undefined;

    return (
      <div className={styles.searchBar}>
        <BreadcrumbBar
          modelStack={latest.current.state.controllerSnapshot.modelStack}
          modelName={modelName()}
          onBack={handleNavigateBack}
          onNavigateToLevel={handleNavigateToLevel}
          onShowDrawer={handleShowDrawer}
        />
        {viewOnlyPill}
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
      // Use the shared serializer so per-element graphical functions, per-element
      // ACTIVE INITIAL equations, and the EXCEPT default round-trip through the
      // upsert (a hand-rolled mapping here previously dropped them).
      return { arrayedEquation: arrayedEquationToJson(eq) };
    }
    return { equation: '' };
  };

  const handleEquationChange = React.useCallback(
    (
      ident: string,
      newEquation: string | undefined,
      newUnits: string | undefined,
      newDocs: string | undefined,
    ): Promise<boolean> => {
      if (isReadOnly()) {
        return Promise.resolve(false);
      }
      const landed = enqueueVariableEdit(
        'equation update',
        ident,
        [newEquation, newUnits, newDocs],
        (variable, _committed, mName) => ({
          models: [{ name: mName, ops: [equationChangeOp(variable, newEquation, newUnits, newDocs)] }],
        }),
      );
      recordPanelSubmission(ident, { equation: newEquation, units: newUnits, docs: newDocs }, landed);
      return landed;
    },
    [],
  );

  // Record a details-panel submission as the element's pending one (see
  // EditorRefs.pendingPanelSubmissions). Each field's entry is removed once its
  // edit settles, unless a newer submission for that field replaced it.
  const recordPanelSubmission = (
    ident: string,
    fields: Partial<Record<'equation' | 'units' | 'docs', string | undefined>>,
    landed: Promise<boolean>,
  ): void => {
    const uid = renderedElementUid(ident);
    if (uid === undefined) {
      return;
    }
    const key = JSON.stringify([modelName(), uid]);
    const entries: PendingSubmission = { ...r.pendingPanelSubmissions.get(key) };
    const recorded: Array<'equation' | 'units' | 'docs'> = [];
    for (const field of ['equation', 'units', 'docs'] as const) {
      const text = fields[field];
      if (text !== undefined) {
        entries[field] = { text, landed };
        recorded.push(field);
      }
    }
    r.pendingPanelSubmissions.set(key, entries);
    void landed.finally(() => {
      const current = r.pendingPanelSubmissions.get(key);
      if (current === undefined) {
        return;
      }
      const next: PendingSubmission = { ...current };
      for (const field of recorded) {
        if (next[field]?.landed === landed && next[field]?.text === fields[field]) {
          delete next[field];
        }
      }
      if (Object.keys(next).length === 0) {
        r.pendingPanelSubmissions.delete(key);
      } else {
        r.pendingPanelSubmissions.set(key, next);
      }
    });
  };

  // The full upsert for an equation/units/docs change of `variable` (the
  // committed variable, at dequeue). The *ToJson serializers preserve every
  // field (compat flags included); the edited fields override.
  const equationChangeOp = (
    variable: Variable,
    newEquation: string | undefined,
    newUnits: string | undefined,
    newDocs: string | undefined,
  ): JsonModelOperation => {
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
      // Modules have no equations or graphical functions -- only units and docs.
      // Use moduleToJson to preserve all fields (including compat flags
      // canBeModuleInput, isPublic, dataSource), then override edited fields.
      const base = moduleToJson(variable);
      op = {
        type: 'upsertModule',
        payload: {
          module: {
            ...base,
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
    return op;
  };

  const handleTableChange = React.useCallback((ident: string, newTable: GraphicalFunction | null): void => {
    if (isReadOnly()) {
      return;
    }
    enqueueVariableEdit('table update', ident, newTable, (variable, _committed, mName) => {
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
      return { models: [{ name: mName, ops: [op] }] };
    });
  }, []);

  // The module variable an edit builder reads; a variable that changed kind
  // since the panel opened fails the item.
  const asModule = (variable: Variable, label: string) => {
    if (variable.type !== 'module') {
      throw new EditorError(`${label} failed: '${variable.ident}' is no longer a module`);
    }
    return variable;
  };

  // Updates the model reference for a module variable.
  const handleModuleModelReferenceChange = React.useCallback((ident: string, newModelName: string): void => {
    if (isReadOnly()) {
      return;
    }
    enqueueVariableEdit('model reference update', ident, newModelName, (variable, _committed, mName) => ({
      models: [
        {
          name: mName,
          // Preserve all fields (including compat) via moduleToJson; override the model ref.
          ops: [
            {
              type: 'upsertModule',
              payload: {
                module: { ...moduleToJson(asModule(variable, 'model reference update')), modelName: newModelName },
              },
            },
          ],
        },
      ],
    }));
  }, []);

  // Updates units and/or documentation for a module variable.
  const handleModuleUnitsDocsChange = React.useCallback(
    (ident: string, newUnits: string | undefined, newDocs: string | undefined): Promise<boolean> => {
      if (isReadOnly()) {
        return Promise.resolve(false);
      }
      const landed = enqueueVariableEdit('module update', ident, [newUnits, newDocs], (variable, _committed, mName) => {
        const module = asModule(variable, 'module update');
        // Preserve all fields (including compat) via moduleToJson; override units/docs.
        const op: JsonModelOperation = {
          type: 'upsertModule',
          payload: {
            module: {
              ...moduleToJson(module),
              units: newUnits ?? module.units ?? undefined,
              documentation: newDocs ?? module.documentation ?? undefined,
            },
          },
        };
        return { models: [{ name: mName, ops: [op] }] };
      });
      recordPanelSubmission(ident, { units: newUnits, docs: newDocs }, landed);
      return landed;
    },
    [],
  );

  // Updates the input references array for a module variable via upsertModule.
  // The engine does full variable replacement (not merge), so we send the
  // complete module with the new references array.
  const handleModuleReferencesChange = React.useCallback(
    (ident: string, newReferences: ReadonlyArray<ModuleReference>): void => {
      if (isReadOnly()) {
        return;
      }
      const references = newReferences.map((ref) => ({ src: ref.src, dst: ref.dst }));
      enqueueVariableEdit('references update', ident, references, (variable, _committed, mName) => ({
        models: [
          {
            name: mName,
            // Preserve all fields (including compat) via moduleToJson; override references.
            ops: [
              {
                type: 'upsertModule',
                payload: { module: { ...moduleToJson(asModule(variable, 'references update')), references } },
              },
            ],
          },
        ],
      }));
    },
    [],
  );

  // Creates a new empty model and sets it as the module's reference.
  // The engine processes projectOps before model ops (see patch.rs),
  // so AddModel creates the model before upsertModule references it.
  const handleCreateModelForModule = React.useCallback((moduleIdent: string): void => {
    if (isReadOnly()) {
      return;
    }
    const mName = modelName();
    const uid = renderedElementUid(moduleIdent);
    void r.controller?.enqueueModelEdit({
      label: 'model creation',
      buildPatch: (committed) => {
        // Generate a unique model name to avoid collisions when the module
        // ident already matches an existing model name.
        let newModelName = moduleIdent;
        if (committed.models.has(newModelName)) {
          newModelName = getUniqueDuplicateName(moduleIdent, committed);
        }
        // Look up the committed module to preserve metadata (including compat)
        // through the model reference change; the shared helper carries every
        // field forward and keeps this in lockstep with the duplicate-model path.
        const existingModule = committedVariable(committed, mName, uid, moduleIdent);
        const modulePayload = buildModuleReferencePayload(
          existingModule,
          existingModule?.ident ?? moduleIdent,
          newModelName,
        );
        return {
          projectOps: [{ type: 'addModel', payload: { name: newModelName } }],
          models: [
            // Seed a default empty view so getCanvas() works after drilling in
            {
              name: newModelName,
              ops: [{ type: 'upsertView', payload: { index: 0, view: { elements: [] } } }],
            },
            {
              name: mName,
              ops: [{ type: 'upsertModule', payload: { module: modulePayload } }],
            },
          ],
        };
      },
    });
  }, []);

  // Duplicates the source model and sets the copy as the module's reference.
  // Copies all variables and the primary view from the source model.
  const handleDuplicateModelForModule = React.useCallback((moduleIdent: string, sourceModelName: string): void => {
    if (isReadOnly()) {
      return;
    }
    const mName = modelName();
    const uid = renderedElementUid(moduleIdent);
    void r.controller?.enqueueModelEdit({
      label: 'model duplication',
      buildPatch: (committed) => {
        const sourceModel = committed.models.get(sourceModelName);
        if (!sourceModel) {
          throw new EditorError(`model duplication failed: model '${sourceModelName}' no longer exists`);
        }
        const existingModule = committedVariable(committed, mName, uid, moduleIdent);
        return duplicateModelPatch(committed, sourceModel, existingModule, existingModule?.ident ?? moduleIdent, mName);
      },
    });
  }, []);

  const duplicateModelPatch = (
    project: Project,
    sourceModel: Model,
    existingModule: Variable | undefined,
    moduleIdent: string,
    mName: string,
  ): JsonProjectPatch => {
    const newModelName = getUniqueDuplicateName(sourceModel.name, project);

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

    // Preserve ALL existing module fields (incl. compat: canBeModuleInput /
    // isPublic / dataSource) through the model reference change; the shared
    // helper keeps this in lockstep with the create-model path, so a
    // full-replacement upsert never drops a compat flag.
    const dupModulePayload = buildModuleReferencePayload(existingModule, moduleIdent, newModelName);

    // Combined patch: create model, copy contents, update module reference.
    // Engine processes projectOps before model ops (patch.rs).
    return {
      projectOps: [{ type: 'addModel', payload: { name: newModelName } }],
      models: [
        { name: newModelName, ops: variableOps },
        {
          name: mName,
          ops: [
            {
              type: 'upsertModule',
              payload: { module: dupModulePayload },
            },
          ],
        },
      ],
    };
  };

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
      <div className={varDetailsClassName} {...{ [DETAILS_SLOT_ATTRIBUTE]: '' }}>
        <ErrorDetails
          status={latest.current.state.controllerSnapshot.status}
          simError={cachedErrors.simError}
          modelErrors={cachedErrors.modelErrors}
          varErrors={cachedErrors.varErrors}
          varUnitErrors={cachedErrors.unitErrors}
          varWarnings={cachedErrors.varWarnings}
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
    // A view element whose variable is missing means the model and view have
    // diverged (corrupted data, or a transient during multi-step updates).
    // Throwing here took the WHOLE editor down via the ErrorBoundary during
    // render -- and since this panel is also the delete affordance, the
    // corrupt element became unremovable. Degrade to no panel instead; the
    // element stays selectable and keyboard-deletable.
    const variable = model.variables.get(ident);
    if (variable === undefined) {
      console.warn(`variable details unavailable: no variable '${ident}' in model '${modelName()}'`);
      return;
    }

    // The panel key (detailsPanelKey) is the selected element plus its
    // variable's committed editable content plus the read-only flag: the panels
    // seed their Slate editors once per mount (see "Details panels are keyed by
    // the selected variable's committed content" in diagram/CLAUDE.md), so a
    // landed edit to this variable, or a mid-session readOnlyMode flip, REMOUNTS
    // an open panel rather than toggling it in place. A flip is deterministic:
    // any in-flight typed text is discarded and the panel re-seeds from the
    // committed state.
    const readOnly = isReadOnly();
    const restoreSeq = latest.current.state.controllerSnapshot.restoreSeq;
    // While the panel holds a draft its key is held: a landed edit (to this
    // variable, or one that rewrote it, as a rename of a name its equation
    // references does) must not remount the panel over text the user typed.
    // The key advances once the panel reports no draft -- its draft landed, or
    // was cancelled. Selecting another element, an undo/redo landing
    // (restoreSeq) and a read-only flip still remount at once.
    const mName = modelName();
    const freshKey = detailsPanelKey(mName, namedElement.uid, variable, restoreSeq, readOnly);
    const base = JSON.stringify([mName, namedElement.uid, restoreSeq, readOnly]);
    const held = r.heldPanelKey;
    const key = latest.current.state.panelHasDraft && held?.base === base ? held.key : freshKey;
    // Idempotent for a given state, so a StrictMode double render is harmless.
    r.heldPanelKey = { base, key };
    // While an undo/redo is queued the panel renders read-only, as the Canvas
    // ignores presses then: the undo's landing remounts the panel (restoreSeq),
    // which would discard text typed meanwhile. Not part of the key, so a draft
    // already submitted keeps its panel until then.
    const panelReadOnly = readOnly || latest.current.state.controllerSnapshot.undoRedoQueued === true;
    const pendingSubmission = r.pendingPanelSubmissions.get(JSON.stringify([mName, namedElement.uid]));

    if (variable.type === 'module') {
      return (
        <div className={varDetailsClassName} {...{ [DETAILS_SLOT_ATTRIBUTE]: '' }}>
          <ModuleDetails
            key={`md-${key}`}
            variable={variable}
            viewElement={namedElement}
            project={defined(getProject())}
            currentModelName={modelName()}
            readOnly={panelReadOnly}
            registerDraftFlush={registerDraftFlush}
            onDraftStateChange={handleDraftStateChange}
            pendingSubmission={pendingSubmission}
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
      <div className={varDetailsClassName} {...{ [DETAILS_SLOT_ATTRIBUTE]: '' }}>
        <VariableDetails
          key={`vd-${key}`}
          variable={variable}
          viewElement={namedElement}
          getLatexEquation={getLatexEquation}
          activeTab={activeTab}
          readOnly={panelReadOnly}
          registerDraftFlush={registerDraftFlush}
          onDraftStateChange={handleDraftStateChange}
          pendingSubmission={pendingSubmission}
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

  // Clicking a tool selects it; clicking the already-active tool deselects it
  // (toggle), returning to plain selection mode -- the load-time state.
  const handleSelectStock = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState((prev) => ({ selectedTool: prev.selectedTool === 'stock' ? undefined : 'stock' }));
  }, []);

  const handleSelectFlow = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState((prev) => ({ selectedTool: prev.selectedTool === 'flow' ? undefined : 'flow' }));
  }, []);

  const handleSelectAux = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState((prev) => ({ selectedTool: prev.selectedTool === 'aux' ? undefined : 'aux' }));
  }, []);

  const handleSelectLink = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState((prev) => ({ selectedTool: prev.selectedTool === 'link' ? undefined : 'link' }));
  }, []);

  const handleSelectModule = React.useCallback((e: React.MouseEvent<HTMLButtonElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    setState((prev) => ({ selectedTool: prev.selectedTool === 'module' ? undefined : 'module' }));
  }, []);

  // Undo/redo is owned by the controller: an edit-class item that reopens the
  // engine from the restored snapshot when it runs, bumps the token, and --
  // when the restored project no longer contains the viewed model -- resets
  // navigation to 'main' and bumps navResetSeq, which the navReset effect
  // observes to clear the Editor's selection/details/tool UI state. The
  // controller refuses it while an edit is pending (snapshot canUndo/canRedo).
  const handleUndoRedo = React.useCallback((kind: 'undo' | 'redo'): void => {
    // Undo/redo rewrite project content; a read-only viewer gets neither
    // (issue #935). This is the single choke point covering the UndoRedoBar
    // buttons and the keyboard shortcut alike. Project-scoped gate: see the
    // keyboard handler's comment for why stdlib views keep undo. A live gesture
    // blocks it too: the gesture's commit could not apply to the restored view.
    if (latest.current.props.readOnlyMode || r.gestureLive) {
      return;
    }
    // An open panel's draft is the user's latest change. Undo submits it first
    // and queues the undo behind its edit, so the undo takes the draft back (and
    // a redo restores it) rather than undoing an older edit under text the panel
    // would then discard. Redo goes the other way: the redo is queued first and
    // the draft submitted behind it -- the draft's payload derives from
    // committed state at dequeue, so it applies on top of the redone project and
    // neither is lost (submitting first would record the draft and discard the
    // redo branch).
    if (kind === 'undo') {
      const submittedDraft = r.draftFlush?.() ?? false;
      r.controller?.undoRedo(kind, { afterQueuedEdits: submittedDraft });
    } else {
      r.controller?.undoRedo(kind);
      r.draftFlush?.();
    }
  }, []);

  const handleDraftStateChange = React.useCallback((hasDraft: boolean): void => {
    setStateRaw((prev) => (prev.panelHasDraft === hasDraft ? prev : { ...prev, panelHasDraft: hasDraft }));
  }, []);

  const handleReload = React.useCallback((): void => {
    const onReload = latest.current.props.onReload;
    if (onReload) {
      onReload();
    } else {
      window.location.reload();
    }
  }, []);

  // The one persistent notice for a lost engine (ProjectSnapshot.engineUnavailable):
  // every edit is refused quietly from then on, so this is the only report.
  const getEngineUnavailableNotice = (): React.ReactElement | undefined => {
    if (props.embedded || !latest.current.state.controllerSnapshot.engineUnavailable) {
      return undefined;
    }
    return (
      <div className={styles.engineUnavailableNotice} role="alert">
        <p className={styles.engineUnavailableTitle}>The model engine stopped working</p>
        <p className={styles.engineUnavailableBody}>
          Changes can no longer be saved, and changes since the last save may be lost. Reload to continue from the last
          saved version.
        </p>
        <div className={styles.engineUnavailableActions}>
          <Button size="small" color="primary" onClick={handleReload}>
            Reload
          </Button>
        </div>
      </div>
    );
  };

  const handleZoomChange = React.useCallback((newZoom: number): void => {
    const view = getView();
    if (!view) {
      return;
    }
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
    handleViewBoxChange(newViewBox, newZoom);
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
    const { embedded, readOnlyMode } = props;
    if (embedded) {
      return undefined;
    }

    const zoom = getView()?.zoom || 1;

    // Undo/redo mutates project content, so the bar is HIDDEN (not merely
    // disabled) for read-only viewers -- matching the hidden SpeedDial, and
    // keeping the chrome quiet rather than showing permanently-dead buttons.
    // The keyboard path and handleUndoRedo are gated alongside. Zoom is a
    // view capability and stays.
    return (
      <div className={styles.undoRedoBar}>
        {!readOnlyMode && (
          <UndoRedoBar undoEnabled={isUndoEnabled()} redoEnabled={isRedoEnabled()} onUndoRedo={handleUndoRedo} />
        )}
        {/*<Snapshotter onSnapshot={handleSnapshot} />*/}
        <ZoomBar zoom={zoom} onChangeZoom={handleZoomChange} />
      </div>
    );
  };

  const getEditorControls = (): React.ReactElement | undefined => {
    const { dialOpen, dialVisible, selectedTool } = state;

    // The creation toolbar is pure mutation affordance: hidden for read-only
    // viewers, embeds, and stdlib models alike (the unified gate).
    if (isReadOnly()) {
      return undefined;
    }

    // Module creation defaults on; hosts opt out (e.g. production app builds).
    const moduleCreationEnabled = props.moduleCreationEnabled ?? true;

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
        {moduleCreationEnabled && (
          <SpeedDialAction
            icon={<ModuleIcon />}
            title="Module"
            onClick={handleSelectModule}
            selected={selectedTool === 'module'}
          />
        )}
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

  // tabIndex={-1}: a click on non-focusable chrome inside the editor then
  // settles focus on this root (the nearest focusable ancestor) rather than on
  // <body>, so the key event that follows carries the root in its path. Not in
  // the tab order; the outline is suppressed in Editor.module.css.
  return (
    <PortalContainerContext.Provider value={props.portalContainer ?? null}>
      <div
        ref={rootRef}
        className={classNames}
        tabIndex={-1}
        {...{ [EDITOR_ROOT_ATTRIBUTE]: '' }}
        onPointerDownCapture={handlePointerDownCapture}
        onFocusCapture={handleActivity}
      >
        {getDrawer()}
        {getDetails(sharedModelBannerInfo.visible)}
        {getSearchBar()}
        {getSharedModelBanner(sharedModelBannerInfo)}
        {getEngineUnavailableNotice()}
        {getCanvas()}
        {getSnackbar()}
        {getEditorControls()}
        {getMetaActionsBar()}
        {getSnapshot()}
      </div>
    </PortalContainerContext.Provider>
  );
});
