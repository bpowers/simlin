// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import { Descendant } from 'slate';
import { defined, exists } from '@simlin/core/common';
import { first, last } from '@simlin/core/collections';
import {
  ViewElement,
  AliasViewElement,
  AuxViewElement,
  CloudViewElement,
  FlowViewElement,
  GroupViewElement,
  LinkViewElement,
  ModuleViewElement,
  StockViewElement,
  NamedViewElement,
  UID,
  StockFlowView,
  Project,
  Model,
  Rect as ViewRect,
  rectDefault as viewRectDefault,
  isNamedViewElement,
  variableHasError,
} from '@simlin/core/datamodel';
import { canonicalize } from '@simlin/core/canonicalize';

import { Alias, aliasBounds, AliasProps } from './Alias';
import { Aux, auxBounds, AuxProps } from './Auxiliary';
import { Cloud, cloudBounds, CloudProps } from './Cloud';
import {
  calcViewBox,
  displayName,
  labelRadii,
  encodeNameNewlines,
  plainDeserialize,
  plainSerialize,
  sanitizeLabelInput,
  Point,
  Rect,
  screenToCanvasPoint,
} from './common';
import { Connector, ConnectorProps } from './Connector';
import { EditableLabel } from './EditableLabel';
import { CanvasRenderContext, EXPORT_LABEL_FILTER_ID, type CanvasRenderContextValue } from './canvas-render-context';
import { fauxTargetUid } from './creation-sentinels';
import { Flow, flowBounds } from './Flow';
import { Group, groupBounds, GroupProps } from './Group';
import { Module, moduleBounds, ModuleProps } from './Module';
import { anyModuleHasModelReference } from '../module-warning';
import { Stock, stockBounds, StockProps } from './Stock';
import {
  VELOCITY_THRESHOLD,
  calculateVelocity as computeVelocity,
  centerOffsetForBounds,
  isDiagramOffscreen,
  isMomentumDone,
  isRenderableZoom,
  momentumOffsetAt,
  pinchOffset,
  pinchZoom,
  resizeViewBox,
  wheelPanOffset,
  wheelZoom,
  zoomAroundPoint,
} from './viewport';
import {
  beyondThreshold,
  classifyPress,
  isLostRelease,
  latchGesture,
  planGesture,
  sameGeometry,
  type GesturePlan,
  type PressGesture,
  type PressHit,
  type PressInput,
  type PressOutcome,
} from '../gesture-planner';

import styles from './Canvas.module.css';

// Pure bounds pass over the displayed elements: every kind but a link folds in
// its drawn box, label included, through the bounds function its renderer's
// module exports. An alias's label shows its target's name, so the target is
// looked up in `elementsByUid`. The engine's `resolve_view` folds the same
// boxes (`tests/svg-rendering.test.ts` pins the two static renderers byte for
// byte). A live gesture's planned elements are what is displayed, so they feed
// the embedded-mode tight viewBox exactly as buildLayers draws them. Undefined
// entries from *Bounds are kept; calcViewBox skips them.
function computeElementBounds(
  displayElements: readonly ViewElement[],
  elementsByUid: ReadonlyMap<UID, ViewElement>,
): Array<Rect | undefined> {
  const bounds: Array<Rect | undefined> = [];
  for (const element of displayElements) {
    switch (element.type) {
      case 'cloud':
        bounds.push(cloudBounds(element));
        break;
      case 'aux':
        bounds.push(auxBounds(element));
        break;
      case 'stock':
        bounds.push(stockBounds(element));
        break;
      case 'module':
        bounds.push(moduleBounds(element));
        break;
      case 'group':
        bounds.push(groupBounds(element));
        break;
      case 'flow':
        bounds.push(flowBounds(element));
        break;
      case 'alias': {
        const aliasOf = elementsByUid.get(element.aliasOfUid) as NamedViewElement | undefined;
        bounds.push(aliasBounds(element, aliasOf));
        break;
      }
      default:
        // link: a connector folds nothing into the bounds
        break;
    }
  }
  return bounds;
}

const ZMax = 6;

// A client point with no pointer behind it, for presses classified from events
// that carry none (a module's double-click).
const NO_POINTER = { clientX: 0, clientY: 0, shiftKey: false, ctrlKey: false, metaKey: false, pointerType: 'mouse' };

// Momentum physics, zoom limits, and the wheel/pinch math live in `viewport.ts`
// (the pure functional core); this shell resolves screen->canvas points and the
// rAF/timer lifecycle, then calls those pure transforms.

// How long an "orphaned" live viewport waits before being committed to the
// controller. Two cases produce one with no natural settle event of its own: a
// wheel/trackpad gesture (a stream of discrete events, no end event -- coalesce
// the burst), and a momentum coast interrupted by a press that does not become a
// viewport gesture. The deferred commit is guarded so a pan/pinch/momentum that
// DID take over commits instead (see scheduleDeferredCommit).
const DEFERRED_COMMIT_DELAY_MS = 200;

// Tracked pointer for multi-touch pinch detection
interface TrackedPointer {
  id: number;
  x: number;
  y: number;
  timestamp: number;
}

// Velocity tracking for momentum
interface VelocityTracker {
  positions: Array<{ x: number; y: number; timestamp: number }>;
}

// The result of the single render-phase derivation step (deriveRenderState).
// Every derived value the render path needs is produced there once, at the top
// of render; the element-rendering helpers only read it, and event handlers read
// it after render returns (connector ends, the name editor's element).
interface RenderDerivation {
  // What is drawn: the live gesture's plan while one is in flight, else the view
  // (plus a draft element while its name is being edited).
  displayElements: readonly ViewElement[];
  // UID -> element lookup over displayElements.
  elementsByUid: Map<UID, ViewElement>;
  // The live gesture's plan, while one is in flight and still valid (E5).
  plan: GesturePlan | undefined;
  // The selection drawn: a committing plan's, so the last preview frame draws
  // what the release commits (E2); otherwise the host's.
  selection: ReadonlySet<UID>;
  // AC1.6: whether any module in the model has a model reference, used to
  // suppress warning dots while a model is being sketched.
  hasAnyModuleReference: boolean;
}

/**
 * What a gesture's release commits: the next view's elements and uid counter,
 * exactly as the last preview frame drew them, and the selection it applies.
 */
export interface GestureCommit {
  readonly label: string;
  readonly elements: readonly ViewElement[];
  readonly nextUid: number;
  readonly selection: ReadonlySet<UID>;
  /** The controller state token the gesture was pressed under. */
  readonly token: number | undefined;
  /**
   * The view the release planned on. The host refuses the commit when its
   * rendered view no longer agrees with it (E5): the elements replace the whole
   * view, so an edit that landed in between would otherwise be reverted.
   */
  readonly baseView: StockFlowView;
  /** An element the edit creates whose name editor opens next (a drawn flow). */
  readonly editName?: UID;
}

export interface CanvasProps {
  embedded: boolean;
  // The host says the displayed model must not be mutated (read-only viewer,
  // stdlib model, embed). The Editor also hands a read-only Canvas a no-op
  // commit callback and no selectedTool; with this flag the gesture planner
  // previews and commits no edit, and the inline label editor a label
  // double-click opens never opens -- it would LOOK editable while the eventual
  // onRenameVariable commit silently no-ops (issue #935).
  readOnly?: boolean;
  // Whether the mount-time offscreen re-center (issue #52) may run for this
  // mount. Default true. A host that opened the view at a viewport it carried
  // from a previous mount (the Editor's `initialViewport`: the user's own
  // pan, or the fit the previous mount already applied) passes false: that
  // framing is what the user is looking at, so a diagram they panned
  // offscreen must not be yanked back by the remount. A viewport that came
  // from data keeps the safety net.
  recenterOffscreenOnMount?: boolean;
  project: Project;
  model: Model;
  view: StockFlowView;
  // The host's state token (ProjectSnapshot.token): a live gesture pressed under
  // another token aborts (E5), since the view it planned on was replaced.
  token?: number;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;
  selection: ReadonlySet<UID>;
  // Returns an error message when the host refuses the name (it names another
  // variable or a pending create); the inline name editor then stays open and
  // shows it, and nothing is committed.
  onRenameVariable: (oldName: string, newName: string) => string | undefined | void;
  onSetSelection: (selected: ReadonlySet<UID>) => void;
  // A gesture released with an edit to commit (see GestureCommit).
  onCommitGesture: (commit: GestureCommit) => void;
  // Returns an error message when the host refuses the name, as onRenameVariable.
  onCreateVariable: (element: ViewElement) => string | undefined | void;
  onClearSelectedTool: () => void;
  // Deletes the selection; the Canvas calls it when a drawn flow's first name
  // edit is cancelled (the flow is then the selection).
  onDeleteSelection: () => void;
  onShowVariableDetails: () => void;
  onViewBoxChange: (viewBox: ViewRect, zoom: number) => void;
  onDrillIntoModule: (moduleIdent: string, targetModelName: string) => void;
  // Allocates the default name of a new element ("New Variable", "New
  // Variable 1", ...). The host allocates against everything that exists once
  // its pending edits land; `props.model` lacks pending creates, so two quick
  // creates allocating from it both got the same name. Absent (static and test
  // hosts), the Canvas allocates against `props.model`.
  newVariableName?: (base: string) => string;
  // Presses start no gesture: the host has an undo or redo queued, and a
  // gesture planned on the view it is about to replace could not commit.
  pressesDisabled?: boolean;
}

// A gesture in flight: what the press started, where (model coordinates), the
// pointer now, and what it was pressed on. Every frame is planned afresh from
// these, never from a previous frame.
interface ActiveGesture {
  readonly gesture: PressGesture;
  readonly pointerId: number;
  readonly pointerType: string;
  readonly press: Point;
  readonly current: Point;
  // The view and token at press: a republish that changes either aborts (E5).
  readonly baseView: StockFlowView;
  readonly token: number | undefined;
  // The selection in effect after the press, and the one a click settles on.
  readonly selection: ReadonlySet<UID>;
  readonly clickSelection: ReadonlySet<UID> | undefined;
}

// An open inline name editor: the element being named, and for a creation
// tool's draft the element itself (it is not in the view until the name is
// done). A drawn flow's first name edit deletes the flow when cancelled.
interface NameEdit {
  readonly uid: UID;
  readonly draft: ViewElement | undefined;
  readonly creatingFlow: boolean;
}

// A two-finger pinch's fixed reference, captured when the second finger lands.
interface PinchState {
  readonly initialDistance: number;
  readonly initialZoom: number;
  readonly modelPoint: Point;
}

// The mutable instance state read by event handlers, native listeners, the
// momentum rAF loop and the ResizeObserver after render returns, collected in
// one ref so every event-time reader shares one "current" view.
interface CanvasRefs {
  svgObserver: ResizeObserver | undefined;
  prevSelectedTool: CanvasProps['selectedTool'];

  // The live gesture, pinch and name editor. Handlers read these refs, so two
  // events that arrive between renders see each other's writes; the gesture and
  // name editor setters mirror into state to re-render (see setGesture).
  gesture: ActiveGesture | undefined;
  pinch: PinchState | undefined;
  nameEdit: NameEdit | undefined;

  // A pan's press in canvas coordinates: the pan physics anchor.
  mouseDownPoint: Point | undefined;

  // The displayed elements the lookup map was built from; the map is rebuilt
  // only when they change identity. Owned by deriveRenderState().
  cachedElements: readonly ViewElement[] | undefined;
  elements: Map<UID, ViewElement>;

  // The most recent render derivation. Written only by deriveRenderState();
  // read by the element-rendering helpers during render and by handlers.
  derived: RenderDerivation;

  // Multi-touch tracking for pinch gestures
  activePointers: Map<number, TrackedPointer>;

  // The canvas offset captured when a drag-pan begins. handleMovingCanvas
  // anchors each move against this rather than props.view.viewBox, so a pan that
  // interrupts an in-flight momentum coast (whose offset has not been committed
  // back to props.view) starts from the on-screen position instead of jumping
  // back to the last committed viewBox.
  panBaseOffset: Point | undefined;

  // Trailing-debounce timer that commits an "orphaned" live viewport -- one left
  // by a wheel/trackpad gesture (no native end event) or by a momentum coast that
  // a non-viewport press interrupted. Re-armed per wheel event / on interruption;
  // its callback is guarded so an active pan/pinch/coast commits instead. Cleared
  // on unmount and by an external-view override.
  deferredCommitTimer: ReturnType<typeof setTimeout> | undefined;

  // The props.view offset/zoom VALUE observed while no gesture was live. The
  // external-override effect compares props.view against this to detect a
  // non-gesture view change (centerVariable, navigation, undo) mid-gesture.
  // Compared by value (not identity) so a content-equal republished snapshot does
  // not look like an external change.
  viewBaseline: { x: number; y: number; zoom: number } | undefined;

  // Momentum/inertia animation
  velocityTracker: VelocityTracker;
  momentumAnimationId: number | undefined;
  momentumStartTime: number | undefined;
  momentumInitialVelocity: Point | undefined;
  momentumStartOffset: Point | undefined;

  // One-shot latch for the mount-time offscreen re-center (issue #52). Set the
  // first time we can evaluate the check (real svgSize, idle, non-embedded) so
  // it runs at most once per Canvas instance and never re-triggers on later
  // prop churn. Because the Editor renders Canvas without a `key`, module
  // navigation reuses the same instance and the latch persists across drill-in
  // -- intentional: navigation restores/seeds its own viewport.
  offscreenChecked: boolean;
}

// The local "live viewport" the canvas owns DURING a gesture (pan, momentum,
// wheel, pinch, resize). While set, the render transform and all gesture math
// read offset+zoom from here instead of `props.view`, so a multi-event gesture
// stays fully local and only notifies the controller once, on settle. `undefined`
// means "no gesture in flight -- read from props.view". It carries zoom as well
// as offset because pinch/wheel-zoom change zoom mid-gesture and there is
// otherwise no local home for it.
interface LiveViewport {
  x: number;
  y: number;
  zoom: number;
}

// The snapshot of props + continuous state that event-time readers (native
// wheel/gesture listeners, the momentum rAF loop, the ResizeObserver, the
// deferred tool-change commit) must see CURRENT, not as captured by a stale
// render closure. Refreshed synchronously on every render.
interface LatestState {
  props: CanvasProps;
  editingName: Array<Descendant>;
  liveViewport: LiveViewport | undefined;
  svgSize: Readonly<{ width: number; height: number }> | undefined;
}

// Main canvas + rendering engine (the imperative shell). Converted from a
// React.PureComponent to a React.memo function component: React.memo replaces
// PureComponent's shallow-prop gate (state changes always re-render in both
// worlds). Per-field useState mirrors the class's setState merge semantics --
// React 18 batches multiple setter calls in one handler into a single
// re-render carrying the net transition, exactly as setState batching did.
// Former instance fields become refs (see CanvasRefs); former this.state.*
// reads from escaped callbacks go through the `latest` ref (see LatestState).
export const Canvas = React.memo(function Canvas(props: CanvasProps): React.ReactElement {
  const svgRef = React.useRef<HTMLDivElement | null>(null);

  // The label-halo filter id (see canvas-render-context.ts). Interactive
  // canvases each get their own: the halo's flood colour is a theme token that
  // resolves in the filter's OWN ancestor chain, so a shared id would paint one
  // Editor's halos in another Editor's theme when two sit on one page (two
  // notebook cells with different `theme` traits). The export path keeps the
  // fixed id the Rust renderer emits. NOT React.useId: that is unique only
  // within one React root, and the notebook widget mounts one root -- from its
  // own copy of React -- per cell, so two cells both get `_r_1_`; `url(#id)`
  // resolves document-wide to the first match, which is exactly the cross-theme
  // leak this id exists to prevent. A random suffix drawn once per mount is
  // unique across roots and React copies alike (interactive canvases are never
  // server-rendered, so there is nothing to hydrate against).
  const [labelHaloId] = React.useState(() => `label-halo-${Math.random().toString(36).slice(2, 10)}`);
  const renderContext = React.useMemo<CanvasRenderContextValue>(
    () =>
      props.embedded
        ? { embedded: true, labelFilterId: EXPORT_LABEL_FILTER_ID }
        : { embedded: false, labelFilterId: labelHaloId },
    [props.embedded, labelHaloId],
  );

  // ---- State ---------------------------------------------------------------
  const [gesture, setGestureState] = React.useState<ActiveGesture | undefined>(undefined);
  const [nameEdit, setNameEditState] = React.useState<NameEdit | undefined>(undefined);
  const [editingName, setEditingName] = React.useState<Array<Descendant>>([]);
  const [liveViewport, setLiveViewport] = React.useState<LiveViewport | undefined>(undefined);
  const [initialBounds, setInitialBounds] = React.useState<ViewRect>(viewRectDefault);
  const [svgSize, setSvgSize] = React.useState<Readonly<{ width: number; height: number }> | undefined>(undefined);
  // The host's refusal of the name the inline editor tried to commit; shown in
  // the editor, cleared by typing or by the editor closing.
  const [nameError, setNameError] = React.useState<string | undefined>(undefined);

  // initialBounds is written in the mount effect and only read there; keep the
  // setter referenced to avoid an unused-var lint while preserving the field.
  void initialBounds;

  // ---- Instance fields as refs ---------------------------------------------
  const refs = React.useRef<CanvasRefs>(undefined as unknown as CanvasRefs);
  if (refs.current === undefined) {
    const elements = new Map<UID, ViewElement>();
    refs.current = {
      svgObserver: undefined,
      prevSelectedTool: undefined,
      gesture: undefined,
      pinch: undefined,
      nameEdit: undefined,
      mouseDownPoint: undefined,
      cachedElements: undefined,
      elements,
      derived: {
        displayElements: [],
        elementsByUid: elements,
        plan: undefined,
        selection: new Set(),
        hasAnyModuleReference: false,
      },
      activePointers: new Map<number, TrackedPointer>(),
      panBaseOffset: undefined,
      deferredCommitTimer: undefined,
      viewBaseline: undefined,
      velocityTracker: { positions: [] },
      momentumAnimationId: undefined,
      momentumStartTime: undefined,
      momentumInitialVelocity: undefined,
      momentumStartOffset: undefined,
      offscreenChecked: false,
    };
  }
  const r = refs.current;

  // ---- Latest props/state snapshot for escaped callbacks ------------------
  // Updated synchronously below on every render. Event handlers, native
  // listeners, the momentum loop, and the ResizeObserver all read through this
  // so they see CURRENT values. Writing during render is safe: it is the same
  // data the JSX below renders, just exposed to non-render-scope callers.
  const latest = React.useRef<LatestState>(undefined as unknown as LatestState);
  latest.current = { props, editingName, liveViewport, svgSize };

  const setGesture = (next: ActiveGesture | undefined): void => {
    r.gesture = next;
    setGestureState(next);
  };

  const setNameEdit = (next: NameEdit | undefined): void => {
    r.nameEdit = next;
    setNameEditState(next);
  };

  // Offset/zoom resolve from the live viewport while a gesture is in flight,
  // else from props.view. Every gesture-math and render read goes through these
  // so a live gesture never has to round-trip through the controller to see its
  // own in-progress viewport.
  const getCanvasOffset = (): Readonly<Point> => latest.current.liveViewport ?? latest.current.props.view.viewBox;

  // The STORED zoom, healed: a value outside the renderable range (unset 0, or
  // a file that recorded a percentage where a factor belongs) is read as 1 so
  // that no gesture threshold, commit, or transform ever consumes it raw. The
  // mount-time fit persists the healed value; until the host reflects it,
  // every read here still sees a sane number.
  const getViewZoom = (): number => {
    const zoom = latest.current.props.view.zoom;
    return isRenderableZoom(zoom) ? zoom : 1;
  };

  const getCanvasZoom = (): number => latest.current.liveViewport?.zoom ?? getViewZoom();

  // Push the live viewport to the controller exactly once and clear it. This is
  // the single settle-time commit shared by every gesture tail (pan release with
  // no momentum, momentum end, wheel debounce, pinch exit). Clearing the live
  // state in the same synchronous stretch as onViewBoxChange -- whose controller
  // path applies the optimistic view synchronously -- keeps props.view and the
  // cleared live state consistent in one React commit, so the diagram does not
  // snap back. A no-op when nothing is live.
  const commitLiveViewport = (): void => {
    const live = latest.current.liveViewport;
    if (!live) {
      return;
    }
    // Source the viewBox width/height from the live measured size, not
    // props.view.viewBox: a resize that fired during this gesture updated
    // `svgSize` but (by design) did not commit, so props.view still holds the
    // pre-resize dimensions. viewBox width/height are pixel dimensions == the
    // measured canvas size, so this settles the gesture with the current size.
    const size = latest.current.svgSize ?? latest.current.props.view.viewBox;
    const newViewBox = {
      ...latest.current.props.view.viewBox,
      x: live.x,
      y: live.y,
      width: size.width,
      height: size.height,
    };
    latest.current.props.onViewBoxChange(newViewBox, live.zoom);
    setLiveViewport(undefined);
  };

  // (Re)arm the deferred commit for an orphaned live viewport (a wheel/trackpad
  // gesture, or a momentum coast a press just interrupted). The commit fires once
  // things have been idle for DEFERRED_COMMIT_DELAY_MS.
  const scheduleDeferredCommit = (): void => {
    if (r.deferredCommitTimer !== undefined) {
      clearTimeout(r.deferredCommitTimer);
    }
    r.deferredCommitTimer = setTimeout(() => {
      r.deferredCommitTimer = undefined;
      // If a viewport gesture (drag-pan, pinch, or a momentum coast) is now in
      // flight, it owns the live viewport (which it inherited) and will commit on
      // its own settle -- don't double-commit. Otherwise commit now, so a plain
      // click/selection that interrupted a wheel scroll or a coast still persists
      // the viewport rather than stranding it in local state.
      const viewportGestureActive =
        r.momentumAnimationId !== undefined || r.gesture?.gesture.kind === 'pan' || r.pinch !== undefined;
      if (!viewportGestureActive) {
        commitLiveViewport();
      }
    }, DEFERRED_COMMIT_DELAY_MS);
  };

  // Cancel a pending deferred commit WITHOUT committing -- used on unmount and by
  // an external-view override that supersedes the abandoned gesture.
  const cancelDeferredCommit = (): void => {
    if (r.deferredCommitTimer !== undefined) {
      clearTimeout(r.deferredCommitTimer);
      r.deferredCommitTimer = undefined;
    }
  };

  // Stop an in-flight momentum coast that something is interrupting, and -- only
  // if a coast was actually running -- arm a deferred commit so the now-orphaned
  // live viewport still has a settle path. Whatever interrupted then either
  // re-arms (a wheel/pan that moves), makes the deferred callback skip (it becomes
  // a pan/pinch/coast), or lets the timer fire (a click, or a wheel that was a
  // clamped no-op). Every caller that stops a coast must go through this so the
  // coasted pan is never silently dropped.
  const interruptCoast = (): void => {
    const wasCoasting = r.momentumAnimationId !== undefined;
    stopMomentumAnimation();
    if (wasCoasting) {
      scheduleDeferredCommit();
    }
  };
  // Non-throwing element lookup over what is drawn (a live plan's elements, or
  // the view). A uid can transiently resolve to nothing -- a name editor naming a
  // flow whose create was refused or rolled back -- and callers skip it rather
  // than crash.
  const tryGetElementByUid = (uid: UID): ViewElement | undefined => r.elements.get(uid);

  const getCanvasPoint = (x: number, y: number): Point => {
    if (svgRef.current) {
      const bounds = svgRef.current.getBoundingClientRect();
      x -= bounds.x;
      y -= bounds.y;
    }
    return screenToCanvasPoint(x, y, getCanvasZoom());
  };

  // Helper to get canvas point with a specific zoom level
  const getCanvasPointWithZoom = (x: number, y: number, zoom: number): Point => {
    if (svgRef.current) {
      const bounds = svgRef.current.getBoundingClientRect();
      x -= bounds.x;
      y -= bounds.y;
    }
    return screenToCanvasPoint(x, y, zoom);
  };

  // Move focus onto the canvas after a click. An <svg> can't take focus, so
  // the focus target is the container div that svgRef points at (tabindex=-1;
  // Canvas.module.css suppresses its focus ring). Focus must land INSIDE the
  // editor, never merely leave the previous element: a text field the user
  // was typing in still blurs (its blur commits), and the key events that
  // follow carry the editor in their path, so the Editor's keyboard scoping
  // resolves them to this instance directly and hosts that gate their own
  // shortcuts on the event target (JupyterLab's data-lm-suppress-shortcuts,
  // walked up from the focused element) see them land in the editor's
  // subtree; focus left
  // on <body> would instead route the key by the last-active instance.
  // preventScroll: a host page (notebook) may scroll; focusing must not jump it.
  // No fallback for a missing container: every caller -- a gesture's release
  // and the name editor closing, which is reached from the keyboard too -- runs
  // on a rendered canvas, where svgRef is always attached.
  const focusCanvas = (): void => {
    svgRef.current?.focus({ preventScroll: true });
  };

  const getNewVariableName = (base: string): string => {
    const allocate = latest.current.props.newVariableName;
    if (allocate !== undefined) {
      return allocate(base);
    }
    const variables = latest.current.props.model.variables;
    if (!variables.has(canonicalize(base))) {
      return base;
    }
    for (let i = 1; i < 1024; i++) {
      const newName = `${base} ${i}`;
      if (!variables.has(canonicalize(newName))) {
        return newName;
      }
    }
    // give up
    return base;
  };

  // ---- The live gesture's plan ---------------------------------------------

  // Model coordinates of a client point: the canvas point less the live offset.
  // Gesture presses and pointers are kept in model coordinates, so a wheel that
  // pans or zooms mid-drag does not move what the drag plans.
  const modelPoint = (clientX: number, clientY: number): Point => {
    const p = getCanvasPoint(clientX, clientY);
    const offset = getCanvasOffset();
    return { x: p.x - offset.x, y: p.y - offset.y };
  };

  // A live gesture survives a republish that changes nothing it reads (E5): the
  // same controller token, and the same geometry as the view it was pressed on.
  // A pan reads nothing of the view.
  const gestureIsValid = (g: ActiveGesture, p: CanvasProps): boolean =>
    g.gesture.kind === 'pan' || (g.token === p.token && sameGeometry(g.baseView, p.view));

  // The gesture's plan with the pointer at `current`. The preview renders it and
  // a release commits it at the release point: preview and commit are one
  // function evaluated at one point (E2).
  const planAt = (g: ActiveGesture, current: Point): GesturePlan => {
    const p = latest.current.props;
    return planGesture({
      view: p.view,
      variables: p.model.variables,
      selection: g.selection,
      gesture: g.gesture,
      press: g.press,
      current,
      zoom: getCanvasZoom(),
      pointerType: g.pointerType,
      readOnly: !!p.readOnly,
      names: getNewVariableName,
      clickSelection: g.clickSelection,
    });
  };

  // The single render-phase derivation step. Invoked once at the top of the
  // render body (and the mount effect); it is the ONLY code permitted to write
  // the render caches (r.elements, r.cachedElements, r.derived).
  const deriveRenderState = (g: ActiveGesture | undefined, edit: NameEdit | undefined): RenderDerivation => {
    const p = latest.current.props;
    const plan = g !== undefined && g.gesture.kind !== 'pan' && gestureIsValid(g, p) ? planAt(g, g.current) : undefined;
    let displayElements: readonly ViewElement[] = plan?.elements ?? p.view.elements;
    if (plan === undefined && edit?.draft !== undefined) {
      displayElements = [...displayElements, edit.draft];
    }
    if (displayElements !== r.cachedElements) {
      r.elements = new Map(displayElements.map((el) => [el.uid, el]));
      r.cachedElements = displayElements;
    }
    // A committing plan draws the selection its release applies, and a rubber
    // band past the click threshold draws its membership, so the last preview
    // frame is the committed frame.
    const drawsPlanSelection =
      plan !== undefined &&
      g !== undefined &&
      (plan.commit === 'edit' ||
        (g.gesture.kind === 'rubberBand' && beyondThreshold(g.press, g.current, getCanvasZoom())));
    const derived: RenderDerivation = {
      displayElements,
      elementsByUid: r.elements,
      plan,
      selection: drawsPlanSelection ? plan.selection : p.selection,
      hasAnyModuleReference: anyModuleHasModelReference(p.model.variables),
    };
    r.derived = derived;
    return derived;
  };

  // ---- Momentum / velocity physics (shell-internal, escapes render) -------

  // Estimate release velocity from the tracked pointer samples. The decision
  // logic (too-few-samples / stationary-stop / recent-average) lives in the pure
  // `computeVelocity`; this shell only supplies the samples and the clock.
  const calculateVelocity = (): Point => computeVelocity(r.velocityTracker.positions, window.performance.now());

  const stopMomentumAnimation = (): void => {
    if (r.momentumAnimationId !== undefined) {
      window.cancelAnimationFrame(r.momentumAnimationId);
      r.momentumAnimationId = undefined;
    }
    r.momentumStartTime = undefined;
    r.momentumInitialVelocity = undefined;
    r.momentumStartOffset = undefined;
  };

  // Animation frame callback for momentum scrolling
  const animateMomentum = (timestamp: number): void => {
    if (
      r.momentumStartTime === undefined ||
      r.momentumInitialVelocity === undefined ||
      r.momentumStartOffset === undefined
    ) {
      return;
    }

    const elapsed = (timestamp - r.momentumStartTime) / 1000; // seconds
    const v0 = r.momentumInitialVelocity;

    // Natural end: the decayed speed dropped below threshold. This is the single
    // commit point for a coasted pan -- push the final live viewport once, then
    // stop. (An interruption, by contrast, stops without committing and lets the
    // interrupting gesture inherit the live viewport.)
    if (isMomentumDone(v0, elapsed)) {
      commitLiveViewport();
      stopMomentumAnimation();
      return;
    }

    // Note: the friction displacement is ADDED because a higher offset moves the
    // view in the positive direction, while velocity is in screen coordinates
    // where dragging right should move the view left. The coasted offset is held
    // in the live viewport (immediate render) -- no per-frame controller
    // round-trip; that is the whole point of issue #707.
    const newOffset = momentumOffsetAt(r.momentumStartOffset, v0, elapsed);
    setLiveViewport({ x: newOffset.x, y: newOffset.y, zoom: getCanvasZoom() });

    // Continue animation
    r.momentumAnimationId = window.requestAnimationFrame(animateMomentum);
  };

  // Start a momentum coast after pan release. Returns whether a coast actually
  // started: the caller commits the pan immediately when it did NOT (a stationary
  // release), and defers the single commit to the coast's natural end when it
  // did. The two are mutually exclusive, so a gesture commits exactly once.
  const startMomentumAnimation = (): boolean => {
    // Cancel any existing momentum animation first (defensive)
    stopMomentumAnimation();

    const velocity = calculateVelocity();
    const speed = Math.hypot(velocity.x, velocity.y);

    // Don't start animation if velocity is at or below threshold
    if (speed <= VELOCITY_THRESHOLD) {
      return false;
    }

    r.momentumInitialVelocity = velocity;
    r.momentumStartOffset = { ...getCanvasOffset() };
    r.momentumStartTime = window.performance.now();

    r.momentumAnimationId = window.requestAnimationFrame(animateMomentum);
    return true;
  };

  // Track position for velocity calculation during pan
  const trackPosition = (x: number, y: number): void => {
    const now = window.performance.now();
    r.velocityTracker.positions.push({ x, y, timestamp: now });

    // Keep only last 200ms of positions to avoid memory bloat
    // Only reallocate array if there's actually something to remove
    const cutoff = now - 200;
    const positions = r.velocityTracker.positions;
    if (positions.length > 0 && positions[0].timestamp <= cutoff) {
      r.velocityTracker.positions = positions.filter((p) => p.timestamp > cutoff);
    }
  };

  // ---- Pinch helpers ------------------------------------------------------

  // Calculate distance between two pointers for pinch gesture
  const getPinchDistance = (): number => {
    const pointers = Array.from(r.activePointers.values());
    if (pointers.length < 2) {
      return 0;
    }
    const dx = pointers[1].x - pointers[0].x;
    const dy = pointers[1].y - pointers[0].y;
    return Math.sqrt(dx * dx + dy * dy);
  };

  // Get the center point between two pointers
  const getPinchCenter = (): Point => {
    const pointers = Array.from(r.activePointers.values());
    if (pointers.length < 2) {
      return { x: 0, y: 0 };
    }
    return {
      x: (pointers[0].x + pointers[1].x) / 2,
      y: (pointers[0].y + pointers[1].y) / 2,
    };
  };

  // Handle pinch-to-zoom gesture movement
  const handlePinchMove = (): void => {
    const interactionNow = r.pinch;
    if (interactionNow === undefined) {
      return;
    }

    const currentDistance = getPinchDistance();
    if (currentDistance === 0 || interactionNow.initialDistance === 0) {
      return;
    }

    // Scale the starting zoom by the finger-distance ratio (clamped).
    const scale = currentDistance / interactionNow.initialDistance;
    const newZoom = pinchZoom(interactionNow.initialZoom, scale);

    // Get the current pinch center in screen coordinates, then convert to canvas
    // coordinates at the NEW zoom level. The fixed model point (under the fingers
    // when the pinch began) is re-anchored under that center.
    const currentCenter = getPinchCenter();
    const currentCenterCanvas = getCanvasPointWithZoom(currentCenter.x, currentCenter.y, newZoom);
    const newOffset = pinchOffset(currentCenterCanvas, interactionNow.modelPoint);

    // Update the live viewport (immediate render); the single commit happens on
    // pinch exit, not per move.
    setLiveViewport({ x: newOffset.x, y: newOffset.y, zoom: newZoom });
  };

  // ---- Native wheel / Safari-gesture listeners (registered at mount) ------

  const handleWheelPan = (e: WheelEvent): void => {
    const zoom = getCanvasZoom();
    const base = getCanvasOffset();
    const viewBox = latest.current.props.view.viewBox;

    // Page deltas (deltaMode 2) scroll a full viewport; measure it from the DOM
    // since the stored viewBox size may be stale during a resize transition.
    const viewportPx = {
      width: svgRef.current?.clientWidth ?? viewBox.width,
      height: svgRef.current?.clientHeight ?? viewBox.height,
    };
    const newOffset = wheelPanOffset(base, { x: e.deltaX, y: e.deltaY, mode: e.deltaMode }, zoom, viewportPx);

    // Update the live viewport and (re)arm the trailing commit; do NOT round-trip
    // to the controller per event.
    setLiveViewport({ x: newOffset.x, y: newOffset.y, zoom });
    scheduleDeferredCommit();
  };

  // Native wheel zoom handler using exponential scaling for natural macOS feel.
  // Exponential scaling ensures symmetric behavior: zoom in 2x then out 2x returns to original.
  const handleNativeWheelZoom = (e: WheelEvent): void => {
    const zoom = getCanvasZoom();

    // Exponential scaling (negative deltaY = pinch out = zoom in), clamped, with
    // an epsilon no-op at the zoom limits.
    const { zoom: newZoom, changed } = wheelZoom(zoom, e.deltaY);
    if (!changed) {
      return;
    }

    // Keep the model point under the cursor fixed across the zoom change: map the
    // same screen pixel into canvas space at both the old (current live) and new
    // zoom. getCanvasPoint reads the live zoom, so the old mapping is correct
    // even mid-gesture.
    const cursorCanvas = getCanvasPoint(e.clientX, e.clientY);
    const base = getCanvasOffset();
    const newCursorCanvas = getCanvasPointWithZoom(e.clientX, e.clientY, newZoom);
    const newOffset = zoomAroundPoint(base, cursorCanvas, newCursorCanvas);

    setLiveViewport({ x: newOffset.x, y: newOffset.y, zoom: newZoom });
    scheduleDeferredCommit();
  };

  // Native wheel event handler with { passive: false } to ensure preventDefault works.
  // React's synthetic onWheel handler is passive by default, so we must use native events.
  const handleNativeWheel = (e: WheelEvent): void => {
    if (latest.current.props.embedded) {
      return;
    }

    // Always prevent default to stop browser zoom, even at zoom limits
    e.preventDefault();

    // Stop any momentum coast this wheel interrupts, arming a deferred commit so
    // its offset still settles even if this wheel event turns out to be a no-op
    // (a zoom already clamped at MIN/MAX returns early below without committing).
    interruptCoast();

    // On Mac trackpads, pinch-to-zoom is reported as wheel events with ctrlKey
    if (e.ctrlKey || e.metaKey) {
      handleNativeWheelZoom(e);
    } else {
      handleWheelPan(e);
    }
  };

  // Safari-specific gesture events for pinch-to-zoom prevention.
  // Safari triggers these events alongside wheel events for trackpad pinch gestures.
  const handleGestureStart = (e: Event): void => {
    if (latest.current.props.embedded) {
      return;
    }
    e.preventDefault();
  };

  const handleGestureChange = (e: Event): void => {
    if (latest.current.props.embedded) {
      return;
    }
    e.preventDefault();
  };

  const handleGestureEnd = (e: Event): void => {
    if (latest.current.props.embedded) {
      return;
    }
    e.preventDefault();
  };

  // ---- ResizeObserver handler ---------------------------------------------

  const handleSvgResize = (contentRect: { width: number; height: number }): void => {
    const newSvgSize = {
      width: contentRect.width,
      height: contentRect.height,
    };
    const oldSize = latest.current.svgSize;
    // Re-center + commit only when idle. Embedded mode draws to tight element
    // bounds and ignores viewBox. While a viewport gesture owns the live viewport,
    // the gesture keeps full control of the offset -- a resize must not shift it
    // (that would fight the user / coast, and the shift would be discarded by the
    // next move/frame anyway). Only `svgSize` updates here; the gesture's settle
    // commit reads the new size from it (see commitLiveViewport).
    if (oldSize && !latest.current.props.embedded && !latest.current.liveViewport) {
      const dWidth = contentRect.width - oldSize.width;
      const dHeight = contentRect.height - oldSize.height;
      const newViewBox = resizeViewBox(getCanvasOffset(), dWidth, dHeight, contentRect.width, contentRect.height);
      latest.current.props.onViewBoxChange(newViewBox, getCanvasZoom());
    }

    setSvgSize(newSvgSize);
  };

  // ---- Pointer handlers ---------------------------------------------------

  const trackPointer = (e: { pointerId: number; clientX: number; clientY: number }): void => {
    r.activePointers.set(e.pointerId, {
      id: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      timestamp: window.performance.now(),
    });
  };

  // Drop the live gesture, and a pan's physics anchors with it. Its release (if
  // one still comes) finds no gesture and commits nothing.
  const endGesture = (): void => {
    r.mouseDownPoint = undefined;
    r.panBaseOffset = undefined;
    setGesture(undefined);
  };

  // A pan's release: start the momentum coast; if it does not start (a
  // stationary release), commit the pan now. Exactly one commit either way.
  const settlePan = (): void => {
    if (latest.current.liveViewport && !startMomentumAnimation()) {
      commitLiveViewport();
    }
  };

  // A gesture ended with no release to commit -- a pointercancel or a lost
  // release -- commits nothing (E5). A pan still settles the viewport it moved:
  // a viewport is presentation, not an edit.
  const cancelGesture = (): void => {
    if (r.gesture?.gesture.kind === 'pan') {
      settlePan();
    }
    endGesture();
    focusCanvas();
  };

  // A second finger: whatever the first finger started is dropped (E5), and the
  // pinch anchors against the live viewport (a prior pan's offset if one was in
  // flight, else props.view), so a pinch that follows a pan keeps its place.
  const startPinch = (): void => {
    endGesture();
    r.velocityTracker.positions = [];
    const center = getPinchCenter();
    const centerCanvas = getCanvasPoint(center.x, center.y);
    const base = getCanvasOffset();
    // The MODEL point under the pinch center stays under the fingers throughout.
    r.pinch = {
      initialDistance: getPinchDistance(),
      initialZoom: getCanvasZoom(),
      modelPoint: { x: centerCanvas.x - base.x, y: centerCanvas.y - base.y },
    };
  };

  // Commit the pinched viewport once, on exit, and drop every pointer:
  // continuing with a single finger after a pinch leads to confusing UX.
  const endPinch = (): void => {
    commitLiveViewport();
    r.pinch = undefined;
    r.activePointers.clear();
    r.mouseDownPoint = undefined;
  };

  const handleMovingCanvas = (e: React.PointerEvent<SVGElement>): void => {
    if (!r.mouseDownPoint) {
      return;
    }
    // Anchor against the offset captured at pan start (see refs.panBaseOffset),
    // not props.view.viewBox, so an interrupted-momentum -> pan does not jump.
    const base = r.panBaseOffset ?? latest.current.props.view.viewBox;
    const curr = getCanvasPoint(e.clientX, e.clientY);
    const newOffset = {
      x: base.x + (curr.x - r.mouseDownPoint.x),
      y: base.y + (curr.y - r.mouseDownPoint.y),
    };
    trackPosition(newOffset.x, newOffset.y);
    // A pan does not change zoom, so the live viewport keeps the current zoom.
    setLiveViewport({ x: newOffset.x, y: newOffset.y, zoom: getCanvasZoom() });
  };

  const pressInput = (
    hit: PressHit,
    e: {
      clientX: number;
      clientY: number;
      shiftKey: boolean;
      ctrlKey: boolean;
      metaKey: boolean;
      pointerType?: string;
    },
    pointers: number,
  ): PressInput => {
    const p = latest.current.props;
    return {
      view: p.view,
      selection: p.selection,
      tool: p.selectedTool,
      hit,
      point: modelPoint(e.clientX, e.clientY),
      shiftKey: e.shiftKey,
      toggleKey: e.ctrlKey || e.metaKey,
      pointerType: e.pointerType || 'mouse',
      readOnly: !!p.readOnly,
      pressesDisabled: !!p.pressesDisabled,
      pointers,
      gestureLive: r.gesture !== undefined,
    };
  };

  const beginNameEdit = (
    uid: UID,
    draft: ViewElement | undefined,
    creatingFlow: boolean,
    named: ViewElement | undefined,
  ): void => {
    setNameEdit({ uid, draft, creatingFlow });
    const name = named !== undefined && isNamedViewElement(named) ? named.name : '';
    setEditingName(plainDeserialize('label', displayName(name)));
    setNameError(undefined);
  };

  // Close the name editor. Settling a name clears the selection, and focus lands
  // on the canvas so the key events that follow belong to this editor.
  const endNameEdit = (): void => {
    setNameEdit(undefined);
    setNameError(undefined);
    latest.current.props.onSetSelection(new Set());
    focusCanvas();
  };

  // Carry out what classifyPress decided a press does.
  const applyPress = (outcome: PressOutcome, e: React.MouseEvent<Element>, capture: boolean): void => {
    const p = latest.current.props;
    switch (outcome.kind) {
      case 'ignore':
      case 'drill':
        return;
      case 'pinch':
        startPinch();
        return;
      case 'abort':
        endGesture();
        return;
      case 'commitName':
        handleEditingNameDone(false);
        return;
      case 'select':
        if (outcome.clearTool) {
          p.onClearSelectedTool();
        }
        p.onSetSelection(outcome.selection);
        return;
      case 'editName':
        if (outcome.clearTool) {
          p.onClearSelectedTool();
        }
        p.onSetSelection(outcome.selection);
        beginNameEdit(outcome.uid, undefined, false, tryGetElementByUid(outcome.uid));
        return;
      case 'start': {
        const pe = e as React.PointerEvent<Element>;
        if (outcome.clearTool) {
          p.onClearSelectedTool();
        }
        if (outcome.selection !== undefined) {
          p.onSetSelection(outcome.selection);
        }
        if (outcome.gesture.kind === 'pan') {
          r.mouseDownPoint = getCanvasPoint(pe.clientX, pe.clientY);
          r.velocityTracker.positions = [];
          const offset = getCanvasOffset();
          r.panBaseOffset = { x: offset.x, y: offset.y };
          trackPosition(offset.x, offset.y);
        } else if (capture) {
          // Capture on the svg root, never the pressed node: a plan can remove the
          // pressed element from the preview (a valid drop deletes the dragged
          // cloud), which releases a capture it held, and a release over chrome
          // would then be lost.
          svgRef.current?.querySelector('svg')?.setPointerCapture(pe.pointerId);
        }
        const at = modelPoint(pe.clientX, pe.clientY);
        setGesture({
          gesture: outcome.gesture,
          pointerId: pe.pointerId,
          pointerType: pe.pointerType || 'mouse',
          press: at,
          current: at,
          baseView: p.view,
          token: p.token,
          selection: outcome.selection ?? p.selection,
          clickSelection: outcome.clickSelection,
        });
        return;
      }
    }
  };

  // A pointer press on the empty canvas or an element. A press that starts
  // anything interrupts an in-flight momentum coast; the live viewport is
  // preserved, so a pan or pinch this press starts inherits it and commits the
  // combined result, while any other press lets interruptCoast's deferred
  // commit persist it.
  const pressPointer = (hit: PressHit, e: React.PointerEvent<SVGElement>): void => {
    const pointers = r.activePointers.size + (r.activePointers.has(e.pointerId) ? 0 : 1);
    const outcome = classifyPress(pressInput(hit, e, pointers));
    if (outcome.kind === 'ignore') {
      return;
    }
    interruptCoast();
    trackPointer(e);
    applyPress(outcome, e, true);
  };

  const moveGesture = (g: ActiveGesture, e: React.PointerEvent<SVGElement>): void => {
    const current = modelPoint(e.clientX, e.clientY);
    const latched = latchGesture(g.gesture, {
      view: latest.current.props.view,
      press: g.press,
      current,
      zoom: getCanvasZoom(),
    });
    setGesture({ ...g, gesture: latched, current });
  };

  // Commit a released gesture: its plan at the release point, which is the frame
  // the preview drew there (E2), unless a republish invalidated it (E5).
  const finishGesture = (g: ActiveGesture, current: Point): void => {
    const p = latest.current.props;
    if (!gestureIsValid(g, p)) {
      focusCanvas();
      return;
    }
    const released: ActiveGesture = {
      ...g,
      current,
      gesture: latchGesture(g.gesture, { view: p.view, press: g.press, current, zoom: getCanvasZoom() }),
    };
    const plan = planAt(released, current);
    if (plan.commit === 'edit') {
      p.onCommitGesture({
        label: plan.label,
        elements: plan.elements,
        nextUid: plan.nextUid,
        selection: plan.selection,
        token: g.token,
        baseView: p.view,
        editName: plan.handoff?.editName,
      });
    } else if (plan.commit === 'select') {
      p.onSetSelection(plan.selection);
    }
    if (plan.handoff !== undefined) {
      const uid = plan.handoff.editName;
      const named = plan.draft ?? plan.elements.find((el) => el.uid === uid);
      beginNameEdit(uid, plan.draft, released.gesture.kind === 'createFlow', named);
      return;
    }
    if (plan.details) {
      p.onShowVariableDetails();
    }
    focusCanvas();
  };

  const handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    pressPointer({ kind: 'canvas' }, e);
  };

  const handlePointerMove = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }
    if (r.activePointers.has(e.pointerId)) {
      trackPointer(e);
    }
    if (r.pinch !== undefined) {
      if (r.activePointers.size >= 2) {
        handlePinchMove();
      }
      return;
    }
    const g = r.gesture;
    if (g === undefined || g.pointerId !== e.pointerId) {
      return;
    }
    if (isLostRelease(e.pointerType, e.buttons)) {
      // The release never came, so forget the pointer too: a later press would
      // count it, and a single touch would start a pinch.
      r.activePointers.delete(e.pointerId);
      cancelGesture();
      return;
    }
    if (g.gesture.kind === 'pan') {
      handleMovingCanvas(e);
      return;
    }
    moveGesture(g, e);
  };

  const handlePointerUp = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    r.activePointers.delete(e.pointerId);
    if (r.pinch !== undefined) {
      endPinch();
      return;
    }
    const g = r.gesture;
    if (g === undefined || g.pointerId !== e.pointerId) {
      return;
    }
    // The gesture ends whatever its commit does: a host callback that throws
    // must not leave it live for the next press to inherit.
    try {
      if (g.gesture.kind === 'pan') {
        settlePan();
        focusCanvas();
      } else {
        finishGesture(g, modelPoint(e.clientX, e.clientY));
      }
    } finally {
      endGesture();
    }
  };

  const handlePointerCancel = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    r.activePointers.delete(e.pointerId);
    if (r.pinch !== undefined) {
      endPinch();
      return;
    }
    if (r.gesture === undefined || r.gesture.pointerId !== e.pointerId) {
      return;
    }
    cancelGesture();
  };

  // A label dragged past the label component's own click threshold starts a
  // label gesture on that first move. Its later moves and its release bubble to
  // the svg's handlers, which update and finish the gesture like any other.
  // The label holds its own pointer capture (Label.tsx), so a label gesture
  // captures nothing more.
  const labelDragImpl = (uid: number, e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded || r.gesture !== undefined) {
      return;
    }
    applyPress(classifyPress(pressInput({ kind: 'labelDrag', uid }, e, 1)), e, false);
  };

  const handleEditingEnd = (e: React.PointerEvent<HTMLDivElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    applyPress(classifyPress(pressInput({ kind: 'nameEditor' }, e, 1)), e, false);
  };

  const editConnectorImpl = (element: ViewElement, e: React.PointerEvent<SVGElement>, isArrowhead: boolean): void => {
    setSelectionImpl(element, e, false, isArrowhead);
  };

  // Called from the element components' press handlers: a body, arrowhead or
  // source-grip press with a pointer, or a label's double-click (isText), which
  // carries no pointer and starts no drag.
  const setSelectionImpl = (
    element: ViewElement,
    e: React.PointerEvent<SVGElement>,
    isText?: boolean,
    isArrowhead?: boolean,
    isSource?: boolean,
  ): void => {
    if (latest.current.props.embedded) {
      return;
    }
    if (isText) {
      applyPress(classifyPress(pressInput({ kind: 'labelDoubleClick', uid: element.uid }, e, 1)), e, false);
      return;
    }
    const part = isArrowhead ? 'arrowhead' : isSource ? 'source' : 'body';
    pressPointer({ kind: 'element', uid: element.uid, part }, e);
  };

  const handleEditingNameChange = (value: Descendant[]): void => {
    setEditingName(value);
    setNameError(undefined);
  };

  const handleEditingNameDone = (isCancel: boolean): void => {
    const edit = r.nameEdit;
    if (edit === undefined) {
      return;
    }
    if (isCancel) {
      // Cancelling a drawn flow's first name edit deletes the flow (its commit
      // made it the selection). The latch lives in this edit only, so a later
      // rename's cancel can never delete anything.
      if (edit.creatingFlow) {
        latest.current.props.onDeleteSelection();
      }
      endNameEdit();
      return;
    }

    // A commit whose sanitized name is empty (all whitespace/blank lines) is a
    // cancel, not a rename to "": for a drawn flow it deletes the flow, matching
    // Escape.
    const newName = sanitizeLabelInput(plainSerialize(defined(latest.current.editingName)));
    if (newName === '') {
      handleEditingNameDone(true);
      return;
    }

    // The element resolves through the non-throwing lookup: a refused or
    // rolled-back create leaves the editor naming an element the view does not
    // hold (issue #820), and there is nothing to commit then.
    const element = edit.draft ?? tryGetElementByUid(edit.uid);
    if (element === undefined || !isNamedViewElement(element)) {
      endNameEdit();
      return;
    }

    // Names persist line breaks as literal backslash-n (see displayName); the
    // rename path encodes in rename-ops.ts (relabelVariable), the create path
    // here. A refused name keeps the editor open with the host's message, so
    // the user can pick another name without losing the element.
    const refusal =
      edit.draft !== undefined
        ? latest.current.props.onCreateVariable({ ...element, name: encodeNameNewlines(newName) } as ViewElement)
        : latest.current.props.onRenameVariable(displayName(element.name), newName);
    if (typeof refusal === 'string') {
      setNameError(refusal);
      return;
    }
    endNameEdit();
  };

  const moduleDoubleClickImpl = (element: ModuleViewElement): void => {
    if (classifyPress(pressInput({ kind: 'moduleDoubleClick', uid: element.uid }, NO_POINTER, 1)).kind !== 'drill') {
      return;
    }
    const variable = latest.current.props.model.variables.get(element.ident);
    if (variable?.type !== 'module' || !variable.modelName) {
      return;
    }
    latest.current.props.onDrillIntoModule(element.ident, variable.modelName);
  };

  // The element components are memo'd, so the callbacks handed to them keep one
  // identity for the Canvas's life and dispatch to this render's implementation.
  // Otherwise every drag frame would re-render every element on the canvas.
  const impls = { setSelectionImpl, labelDragImpl, editConnectorImpl, moduleDoubleClickImpl };
  const handlers = React.useRef(impls);
  handlers.current = impls;
  const handleSetSelection = React.useCallback(
    (
      element: ViewElement,
      e: React.PointerEvent<SVGElement>,
      isText?: boolean,
      isArrowhead?: boolean,
      isSource?: boolean,
    ): void => handlers.current.setSelectionImpl(element, e, isText, isArrowhead, isSource),
    [],
  );
  const handleLabelDrag = React.useCallback(
    (uid: number, e: React.PointerEvent<SVGElement>): void => handlers.current.labelDragImpl(uid, e),
    [],
  );
  const handleEditConnector = React.useCallback(
    (element: ViewElement, e: React.PointerEvent<SVGElement>, isArrowhead: boolean): void =>
      handlers.current.editConnectorImpl(element, e, isArrowhead),
    [],
  );
  const handleModuleDoubleClick = React.useCallback(
    (element: ModuleViewElement): void => handlers.current.moduleDoubleClickImpl(element),
    [],
  );

  // ---- Element-rendering helpers (read r.derived; never mutate caches) -----

  // Drawn as selected: the derived selection, a creation tool's draft while it
  // is dragged, and the element whose name is being edited.
  const isSelected = (uid: UID): boolean =>
    r.derived.selection.has(uid) || r.derived.plan?.draft?.uid === uid || nameEdit?.uid === uid;

  // The drop target a live gesture's pointer is over: green when valid, red when
  // not, undefined for every other element.
  const targetState = (uid: UID): boolean | undefined => {
    const target = r.derived.plan?.target;
    return target !== undefined && target.uid === uid ? target.valid : undefined;
  };

  const alias = (element: AliasViewElement): React.ReactElement => {
    const aliasOf = r.elements.get(element.aliasOfUid) as NamedViewElement | undefined;
    const aliasProps: AliasProps = {
      isSelected: isSelected(element.uid),
      isValidTarget: aliasOf ? targetState(aliasOf.uid) : undefined,
      series: aliasOf ? props.model.variables.get(defined(aliasOf.ident))?.data : undefined,
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      element,
      aliasOf,
    };
    return <Alias key={element.uid} {...aliasProps} />;
  };

  const cloud = (element: CloudViewElement): React.ReactElement | undefined => {
    // A cloud whose flow is not drawn is corrupt or transient data: skip it.
    if (tryGetElementByUid(element.flowUid) === undefined) {
      return undefined;
    }
    const cloudProps: CloudProps = {
      element,
      isSelected: isSelected(element.uid),
      onSelection: handleSetSelection,
    };
    return <Cloud key={element.uid} {...cloudProps} />;
  };

  const aux = (element: AuxViewElement, editing: boolean): React.ReactElement => {
    const variable = props.model.variables.get(element.ident);
    const selected = isSelected(element.uid);
    const auxProps: AuxProps = {
      element,
      series: variable?.data,
      isSelected: selected,
      isEditingName: selected && editing && nameEdit?.uid === element.uid,
      isValidTarget: targetState(element.uid),
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      hasWarning: variable ? variableHasError(variable) : false,
    };
    return <Aux key={element.uid} {...auxProps} />;
  };

  const stock = (element: StockViewElement, editing: boolean): React.ReactElement => {
    const variable = props.model.variables.get(element.ident);
    const selected = isSelected(element.uid);
    const stockProps: StockProps = {
      element,
      series: variable?.data,
      isSelected: selected,
      isEditingName: selected && editing && nameEdit?.uid === element.uid,
      isValidTarget: targetState(element.uid),
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      hasWarning: variable ? variableHasError(variable) : false,
    };
    return <Stock key={element.uid} {...stockProps} />;
  };

  const module = (element: ModuleViewElement, editing: boolean): React.ReactElement => {
    const variable = props.model.variables.get(element.ident);
    const hasEngineError = variable ? variableHasError(variable) : false;
    const selected = isSelected(element.uid);
    const moduleProps: ModuleProps = {
      element,
      isSelected: selected,
      isEditingName: selected && editing && nameEdit?.uid === element.uid,
      isValidTarget: targetState(element.uid),
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      onDoubleClick: handleModuleDoubleClick,
      // AC1.6: suppress warning when no module in the model has a model reference
      // yet (new model scenario where user is rapidly sketching structure).
      hasWarning: hasEngineError && r.derived.hasAnyModuleReference,
    };
    return <Module key={element.uid} {...moduleProps} />;
  };

  const group = (element: GroupViewElement): React.ReactElement => {
    const groupProps: GroupProps = { element, isSelected: isSelected(element.uid) };
    return <Group key={element.uid} {...groupProps} />;
  };

  const connector = (element: LinkViewElement): React.ReactElement | undefined => {
    // A dangling from/to reference (uid not in the view) is corrupt data: skip
    // the broken link rather than throwing out of render (#812, #817). A link a
    // gesture is dragging points at the plan's stand-in target at the pointer.
    const from = tryGetElementByUid(element.fromUid);
    const to = tryGetElementByUid(element.toUid);
    if (!from || !to) {
      return undefined;
    }
    const connectorProps: ConnectorProps = {
      element,
      from,
      to,
      isSelected: isSelected(element.uid),
      isDashed: to.type === 'stock',
      onSelection: handleEditConnector,
    };
    return <Connector key={element.uid} {...connectorProps} />;
  };

  const flow = (element: FlowViewElement, editing: boolean): React.ReactElement | undefined => {
    const variable = props.model.variables.get(element.ident);
    const selected = isSelected(element.uid);

    if (element.points.length < 2) {
      return undefined;
    }
    // A dangling endpoint reference (source/sink uid not in the view) is corrupt
    // data -- transient during an undo rebuild (#817) or persisted (#812). Skip
    // rendering the broken flow rather than throwing out of render and taking the
    // whole editor down via the ErrorBoundary.
    const sourceId = first(element.points).attachedToUid;
    const source = sourceId === undefined ? undefined : tryGetElementByUid(sourceId);
    if (!source || (source.type !== 'stock' && source.type !== 'cloud')) {
      return undefined;
    }
    const sinkId = last(element.points).attachedToUid;
    const sink = sinkId === undefined ? undefined : tryGetElementByUid(sinkId);
    if (!sink || (sink.type !== 'stock' && sink.type !== 'cloud')) {
      return undefined;
    }

    // A drag draws the flow exactly as it will be committed -- its sink cloud at
    // the endpoint -- so nothing is drawn differently while moving (E2).
    return (
      <Flow
        key={element.uid}
        element={element}
        series={variable?.data}
        source={source}
        sink={sink}
        embedded={props.embedded}
        isSelected={selected}
        hasWarning={variable ? variableHasError(variable) : false}
        isEditingName={selected && editing && nameEdit?.uid === element.uid}
        isValidTarget={targetState(element.uid)}
        onSelection={handleSetSelection}
        onLabelDrag={handleLabelDrag}
      />
    );
  };

  // One layer per z-order so the element kinds compose: groups behind
  // everything, then links, flows, stocks/clouds/modules, and auxes/aliases.
  const buildLayers = (displayElements: readonly ViewElement[], editing: boolean): React.ReactElement[][] => {
    const zLayers = new Array(ZMax) as React.ReactElement[][];
    for (let i = 0; i < ZMax; i++) {
      zLayers[i] = [];
    }

    for (const element of displayElements) {
      // A link preview's stand-in target at the pointer is never drawn.
      if (element.uid === fauxTargetUid) {
        continue;
      }
      let zOrder = 0;
      let component: React.ReactElement | undefined;
      if (element.type === 'aux') {
        component = aux(element, editing);
        zOrder = 5;
      } else if (element.type === 'link') {
        component = connector(element);
        zOrder = 2;
      } else if (element.type === 'stock') {
        component = stock(element, editing);
        zOrder = 4;
      } else if (element.type === 'flow') {
        component = flow(element, editing);
        zOrder = 3;
      } else if (element.type === 'cloud') {
        component = cloud(element);
        zOrder = 4;
      } else if (element.type === 'alias') {
        component = alias(element);
        zOrder = 5;
      } else if (element.type === 'module') {
        component = module(element, editing);
        zOrder = 4;
      } else if (element.type === 'group') {
        component = group(element);
        zOrder = 0;
      }

      if (!component) {
        continue;
      }

      zLayers[zOrder].push(component);
    }

    return zLayers;
  };

  // ---- External-view override (issue #707) --------------------------------
  // While a gesture owns the live viewport, props.view is expected to stay put
  // (a gesture does not commit mid-flight). If props.view's offset/zoom VALUE
  // nonetheless changes, some other source moved the view -- centerVariable,
  // module navigation, or an undo that restored a different viewport -- and that
  // external view must win: drop the live viewport and cancel any pending
  // momentum/wheel commit, with no stray commit of the abandoned gesture. A
  // self-commit clears the live viewport in the same React commit as its
  // optimistic props.view update, so it is never observed here as still-live.
  // Comparison is by value against a baseline tracked while idle, so a
  // content-equal republished snapshot (new identity, same viewport) is ignored.
  React.useEffect(() => {
    const pv = props.view;
    const current = { x: pv.viewBox.x, y: pv.viewBox.y, zoom: pv.zoom };
    if (liveViewport) {
      const baseline = r.viewBaseline;
      if (baseline && (baseline.x !== current.x || baseline.y !== current.y || baseline.zoom !== current.zoom)) {
        stopMomentumAnimation();
        cancelDeferredCommit();
        setLiveViewport(undefined);
        r.viewBaseline = current;
        // If a pointer-driven viewport gesture (drag-pan or pinch) is still
        // physically in progress, abandon it too. Clearing only liveViewport is
        // not enough: a continued pointer move would recreate it from the now
        // stale press-time anchor (panBaseOffset) / pinch reference and the
        // pointer-up could then commit that abandoned gesture back over the
        // external view. Dropping the pan gesture, the pinch and the pointer
        // anchors makes handleMovingCanvas/handlePinchMove no-op and the
        // release a clean no-commit. (Non-viewport gestures don't touch
        // liveViewport, so they're left alone.)
        if (r.gesture?.gesture.kind === 'pan' || r.pinch !== undefined) {
          endGesture();
          r.pinch = undefined;
          r.activePointers.clear();
        }
      }
    } else {
      // Idle: track props.view as the baseline for the next gesture.
      r.viewBaseline = current;
    }
    // Triggers: props.view (the external change) and liveViewport (gesture
    // start/end, which moves the baseline). The handler functions are stable
    // shell closures read directly and are intentionally not deps. (The repo lint
    // config does not enable react-hooks/exhaustive-deps, so no disable directive
    // is needed.)
  }, [props.view, liveViewport]);

  // ---- Mount / unmount effect ---------------------------------------------
  // componentDidMount -> mount effect; componentWillUnmount -> the cleanup.
  // Runs once (empty deps); reads the latest props/state through `latest`.
  // Cleanup is symmetric so a StrictMode mount/unmount/mount cycle is safe.
  React.useEffect(() => {
    const derived = deriveRenderState(r.gesture, r.nameEdit);

    // Compute initial diagram bounds via the explicit pure pass.
    const elementBounds = computeElementBounds(derived.displayElements, derived.elementsByUid);

    let computedInitialBounds: ViewRect | undefined;
    const bounds = calcViewBox(elementBounds);
    if (bounds) {
      const left = Math.floor(bounds.left) - 10;
      const top = Math.floor(bounds.top) - 10;
      const width = Math.ceil(bounds.right - left) + 10;
      const height = Math.ceil(bounds.bottom - top) + 10;
      computedInitialBounds = { x: left, y: top, width, height };
      setInitialBounds(computedInitialBounds);
    }

    const svgElement = exists(svgRef.current);
    r.svgObserver?.disconnect();
    r.svgObserver = new ResizeObserver((entries: ResizeObserverEntry[]) => {
      const entry = defined(entries[0]);
      const target = entry.target as HTMLDivElement;
      handleSvgResize({
        width: target.clientWidth,
        height: target.clientHeight,
      });
    });

    r.svgObserver.observe(svgElement);

    // Register native event listeners with { passive: false } to ensure preventDefault() works.
    // React's synthetic event handlers are passive by default for wheel events, which means
    // preventDefault() is ignored and the browser still performs its native pinch-to-zoom.
    const svg = svgElement.querySelector('svg');
    if (svg) {
      svg.addEventListener('wheel', handleNativeWheel, { passive: false });
      // Safari-specific gesture events for pinch-to-zoom prevention
      svg.addEventListener('gesturestart', handleGestureStart, { passive: false });
      svg.addEventListener('gesturechange', handleGestureChange, { passive: false });
      svg.addEventListener('gestureend', handleGestureEnd, { passive: false });
    }

    // Escape abandons a live gesture: the preview returns to the published view
    // and the release, when it comes, finds nothing to commit.
    const handleEscape = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && r.gesture !== undefined) {
        cancelGesture();
      }
    };
    window.addEventListener('keydown', handleEscape);

    const svgWidth = svgElement.clientWidth;
    const svgHeight = svgElement.clientHeight;

    const viewBox = latest.current.props.view.viewBox;
    // getViewZoom already heals an out-of-range stored zoom to 1; the raw value
    // is only consulted to decide that the healed one must be PERSISTED
    // (through onViewBoxChange below), so the model is fixed on first open.
    const storedZoom = latest.current.props.view.zoom;
    const zoom = getViewZoom();

    let shouldUpdate = false;
    const prevBounds = viewBox;
    if (viewBox.width === 0 || viewBox.height === 0) {
      shouldUpdate = true;
    } else if (
      viewBox.width !== svgWidth ||
      viewBox.height !== svgHeight ||
      !isFinite(viewBox.x) ||
      !isFinite(viewBox.y) ||
      !isRenderableZoom(storedZoom)
    ) {
      shouldUpdate = true;
    }

    if (shouldUpdate) {
      let x = 0;
      let y = 0;

      // on a new diagram we won't have an initial bounds, but we should
      // still set the width/height
      if (computedInitialBounds) {
        const currWidth = svgWidth / zoom;
        const currHeight = svgHeight / zoom;

        // convert diagram bounds to cx,cy
        computedInitialBounds = defined(computedInitialBounds);
        const diagramCx = computedInitialBounds.x + computedInitialBounds.width / 2;
        const diagramCy = computedInitialBounds.y + computedInitialBounds.height / 2;

        if (prevBounds.width && prevBounds.height) {
          const prevWidth = prevBounds.width / zoom;
          const prevHeight = prevBounds.height / zoom;
          const prevX = isFinite(prevBounds.x) ? prevBounds.x : 0;
          const prevY = isFinite(prevBounds.y) ? prevBounds.y : 0;
          // find where cx/cy was as % of prev viewport  (e.g. .2,.3)
          const prevCx = prevX + diagramCx;
          const prevCy = prevY + diagramCy;
          // find proportional cx/cy on curr viewport  (.2 * curr.w...)
          const fractionX = prevCx / prevWidth;
          const fractionY = prevCy / prevHeight;

          // go from cx/cy on current viewport to zoom-adjusted offset
          x = fractionX * currWidth - diagramCx;
          y = fractionY * currHeight - diagramCy;
        } else {
          const viewCx = currWidth / 2;
          const viewCy = currHeight / 2;

          x = viewCx - diagramCx;
          y = viewCy - diagramCy;
        }
      }

      const newViewBox: ViewRect = { x, y, width: svgWidth, height: svgHeight };

      latest.current.props.onViewBoxChange(newViewBox, zoom);

      setSvgSize({
        width: svgWidth,
        height: svgHeight,
      });
    }

    return () => {
      // componentWillUnmount: disconnect the observer, remove native listeners,
      // stop momentum, and clear velocity/pointer state. Symmetric with the
      // setup above so a StrictMode mount/unmount/mount cycle leaves no stuck
      // listeners or running rAF.
      if (r.svgObserver) {
        r.svgObserver.disconnect();
        r.svgObserver = undefined;
      }
      window.removeEventListener('keydown', handleEscape);
      const teardownSvg = svgRef.current?.querySelector('svg');
      if (teardownSvg) {
        teardownSvg.removeEventListener('wheel', handleNativeWheel);
        teardownSvg.removeEventListener('gesturestart', handleGestureStart);
        teardownSvg.removeEventListener('gesturechange', handleGestureChange);
        teardownSvg.removeEventListener('gestureend', handleGestureEnd);
      }
      // Cancel any running momentum animation and clear all momentum state
      stopMomentumAnimation();
      // Cancel a pending deferred commit WITHOUT firing it: committing during
      // teardown would call onViewBoxChange (-> a setState on the unmounting
      // host). The dropped commit is harmless -- viewBox is presentational and
      // re-persisted on the next interaction.
      cancelDeferredCommit();
      // Clear velocity tracking and pointer data
      r.velocityTracker.positions = [];
      r.activePointers.clear();
      // Clear the pan anchor and any pinch reference
      r.mouseDownPoint = undefined;
      r.pinch = undefined;
    };
    // Intentionally empty deps: this effect mirrors componentDidMount/Unmount.
    // All props/state it reads go through `latest`, and the native listeners /
    // observer / momentum callbacks likewise read `latest`, so nothing here
    // closes over stale values. (The repo lint config does not enable
    // react-hooks/exhaustive-deps, so no disable directive is needed.)
  }, []);

  // ---- Offscreen re-center on mount (issue #52) ---------------------------
  // A saved viewBox/zoom can leave the whole diagram outside the visible canvas
  // (e.g. it was panned far away, or the window is a very different size than
  // when it was saved and the mount-time proportional refit preserved the
  // offscreen framing). The user then opens the model to a blank surface with
  // no idea where it went. Once we know both the element bounds and the real
  // svg size, check the visible fraction and, if the diagram is (mostly)
  // offscreen, center it at the current zoom.
  //
  // This runs at most once per mount (the `offscreenChecked` latch): it must not
  // fight a later user pan or an external viewport change, and module drill-in
  // (same Canvas instance -- see the latch's comment) manages its own viewport.
  // A host that carried the viewport in from a previous mount opts out
  // (`recenterOffscreenOnMount === false`, see the prop). "Idle" here matches the resize handler's convention (`!liveViewport`): while
  // a gesture owns the live viewport the check waits, so it never yanks the view
  // out from under an in-flight pan/pinch/coast. The commit goes through
  // `onViewBoxChange` directly -- the same view-only, non-undo path the idle
  // resize uses -- so the host persists it once with no history entry. Zoom is
  // deliberately left unchanged (scope kept tight to re-centering).
  React.useEffect(() => {
    if (r.offscreenChecked || props.embedded || props.recenterOffscreenOnMount === false || liveViewport) {
      return;
    }
    if (!svgSize || svgSize.width <= 0 || svgSize.height <= 0) {
      // No real measurement yet: wait for the first ResizeObserver delivery
      // (or the mount effect's initial fit) without burning our one shot.
      return;
    }
    // This is the one measured, idle, non-embedded opportunity.
    r.offscreenChecked = true;

    // Bounds come from the derivation the just-committed render produced (same
    // pure pass the mount effect uses), so this reflects what is actually drawn.
    const derived = r.derived;
    const bounds = calcViewBox(computeElementBounds(derived.displayElements, derived.elementsByUid));
    if (!bounds) {
      // Empty model: nothing to center against.
      return;
    }

    const offset = props.view.viewBox;
    const zoom = getViewZoom();
    if (!isDiagramOffscreen(bounds, offset, zoom, svgSize)) {
      return;
    }

    const centered = centerOffsetForBounds(bounds, zoom, svgSize);
    const newViewBox: ViewRect = {
      x: centered.x,
      y: centered.y,
      width: svgSize.width,
      height: svgSize.height,
    };
    props.onViewBoxChange(newViewBox, zoom);
    // Deps: svgSize (the gating first measurement), props.view (re-evaluate if
    // the view changes before we get a measured shot), and liveViewport (defer
    // while a gesture is live, re-check when it settles). The latch makes the
    // effect a no-op after its single run, so extra runs are harmless.
  }, [svgSize, props.view, liveViewport]);

  // ---- E5: a republish that invalidates the live gesture drops it ---------
  // Planning already ignores an invalid gesture (deriveRenderState renders the
  // published view), and a release re-checks; dropping it here also keeps a
  // later move from planning on the replaced view.
  React.useEffect(() => {
    const g = r.gesture;
    if (g !== undefined && !gestureIsValid(g, props)) {
      endGesture();
    }
  }, [props.view, props.token]);

  // ---- A name editor whose element is gone closes quietly -----------------
  // A refused or rolled-back flow create leaves the editor naming a uid the view
  // no longer holds. It closes without settling a selection, so the overlay does
  // not linger inert and a later tool change has nothing to commit. A draft (an
  // element the view never held) is exempt.
  React.useEffect(() => {
    const edit = r.nameEdit;
    if (edit !== undefined && edit.draft === undefined && !props.view.elements.some((el) => el.uid === edit.uid)) {
      setNameEdit(undefined);
      setNameError(undefined);
    }
  }, [props.view, nameEdit]);

  // ---- Render -------------------------------------------------------------

  const { selectedTool, embedded } = props;

  let isEditingNameNow = nameEdit !== undefined;
  if (isEditingNameNow && selectedTool !== r.prevSelectedTool) {
    // Changing the tool while editing commits the name. The deferred done fires
    // after this render commits and reads the refs, so it observes the latest
    // name edit.
    setTimeout(() => {
      handleEditingNameDone(false);
    });
    isEditingNameNow = false;
  }
  r.prevSelectedTool = selectedTool;

  // phase 1: the single render derivation (displayed elements, lookup, the live
  // plan and the drawn selection). The only place render writes the caches.
  const derived = deriveRenderState(gesture, nameEdit);
  const displayElements = derived.displayElements;

  // phase 2: create React components and add them to the appropriate layer
  const zLayers = buildLayers(displayElements, isEditingNameNow);

  let overlayClass = styles.overlay;
  let nameEditor;

  let dragRect;
  if (gesture?.gesture.kind === 'rubberBand' && beyondThreshold(gesture.press, gesture.current, getCanvasZoom())) {
    const { press, current } = gesture;
    dragRect = (
      <rect
        className={styles.dragRectOverlay}
        x={Math.min(press.x, current.x)}
        y={Math.min(press.y, current.y)}
        width={Math.abs(press.x - current.x)}
        height={Math.abs(press.y - current.y)}
      />
    );
  }

  // The element being named: a draft, or an element of what is drawn. It can be
  // unresolvable for a moment -- a drawn flow whose commit the host refused --
  // and the editor is skipped then rather than crashing.
  const editingElement =
    isEditingNameNow && nameEdit !== undefined
      ? ((nameEdit.draft ?? tryGetElementByUid(nameEdit.uid)) as NamedViewElement | undefined)
      : undefined;
  if (!editingElement) {
    overlayClass += ' ' + styles.noPointerEvents;
  } else {
    const zoom = getCanvasZoom();
    const { rw, rh } = labelRadii(editingElement.type);
    const offset = getCanvasOffset();
    nameEditor = (
      <EditableLabel
        uid={editingElement.uid}
        cx={(editingElement.x + offset.x) * zoom}
        cy={(editingElement.y + offset.y) * zoom}
        side={editingElement.labelSide}
        rw={rw * zoom}
        rh={rh * zoom}
        zoom={zoom}
        value={defined(editingName)}
        error={nameError}
        onChange={handleEditingNameChange}
        onDone={handleEditingNameDone}
      />
    );
  }

  let transform;
  let viewBox: string | undefined;
  if (embedded) {
    // For embedded/export mode, always calculate tight bounds from elements.
    // The stored view.viewBox represents the editor viewport, not diagram bounds.
    const bounds = calcViewBox(computeElementBounds(displayElements, derived.elementsByUid));
    if (bounds) {
      const left = Math.floor(bounds.left) - 10;
      const top = Math.floor(bounds.top) - 10;
      const width = Math.ceil(bounds.right - left) + 10;
      const height = Math.ceil(bounds.bottom - top) + 10;
      viewBox = `${left} ${top} ${width} ${height}`;
    }
  } else {
    // getCanvasZoom heals an out-of-range stored zoom, so nothing is ever drawn
    // at, say, 200x because a file recorded a percentage where a factor belongs.
    const zoom = getCanvasZoom();
    const offset = getCanvasOffset();

    transform = `matrix(${zoom} 0 0 ${zoom} ${offset.x * zoom} ${offset.y * zoom})`;
  }

  const overlay = embedded ? undefined : (
    <div className={overlayClass} onPointerDown={handleEditingEnd}>
      {nameEditor}
    </div>
  );

  // The label halo: each label's glyphs dilated and blurred into a soft
  // backing plate at 85% opacity, so a label stays legible where it crosses a
  // connector or a flow. Two variants of one filter, differing only in where
  // the plate's colour comes from (see canvas-render-context.ts):
  //  - export: a colour matrix that forces every pixel to literal white -- the
  //    standalone SVG has no tokens, and this markup is byte-identical to the
  //    Rust renderer's;
  //  - interactive: an feFlood whose colour is the `--color-white` token (the
  //    same token every canvas primitive uses for its fill: white in light
  //    mode, dark in a dark host), composited `in` the blurred alpha. `in`
  //    multiplies the flood's alpha by the blur's, exactly what the matrix's
  //    alpha row does, so light mode renders pixel-for-pixel as before. Filter
  //    primitives cannot read `var()` from presentation attributes in every
  //    engine, so the colour is set as a CSS property (`style`), which they can.
  const labelHaloFilter = renderContext.embedded ? (
    <filter id={renderContext.labelFilterId} x="-50%" y="-50%" width="200%" height="200%">
      <feMorphology operator="dilate" radius="4" />
      <feGaussianBlur stdDeviation="2" />
      <feColorMatrix
        type="matrix"
        values="0 0 0 0 1
                          0 0 0 0 1
                          0 0 0 0 1
                          0 0 0 0.85 0"
      />
      <feComposite operator="over" in="SourceGraphic" />
    </filter>
  ) : (
    <filter id={renderContext.labelFilterId} x="-50%" y="-50%" width="200%" height="200%">
      <feMorphology operator="dilate" radius="4" />
      <feGaussianBlur stdDeviation="2" result="plate" />
      <feFlood style={{ floodColor: 'var(--color-white)' }} floodOpacity={0.85} />
      <feComposite operator="in" in2="plate" />
      <feComposite operator="over" in="SourceGraphic" />
    </filter>
  );

  return (
    <div
      style={{ height: '100%', width: '100%' }}
      ref={svgRef}
      className={`${styles.canvas} ${styles.canvasContainer} simlin-canvas`}
      tabIndex={-1}
    >
      <svg
        viewBox={viewBox}
        preserveAspectRatio="xMinYMin"
        className={clsx(styles.canvas, styles.simlinCanvas, 'simlin-canvas')}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerCancel={handlePointerCancel}
        onPointerUp={handlePointerUp}
      >
        <defs>{labelHaloFilter}</defs>
        <CanvasRenderContext.Provider value={renderContext}>
          <g transform={transform}>
            {zLayers}
            {dragRect}
          </g>
        </CanvasRenderContext.Provider>
      </svg>
      {overlay}
    </div>
  );
});
