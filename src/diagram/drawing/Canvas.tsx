// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/// <reference types="resize-observer-browser" />

import * as React from 'react';

import clsx from 'clsx';
import { Descendant } from 'slate';
import { defined, exists, mapValues, setsEqual } from '@simlin/core/common';
import { at, first, getOrThrow, last, only } from '@simlin/core/collections';
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
  Point as FlowPoint,
  UID,
  LabelSide,
  StockFlowView,
  Project,
  Model,
  Rect as ViewRect,
  rectDefault as viewRectDefault,
  isNamedViewElement,
  variableHasError,
} from '@simlin/core/datamodel';
import { canonicalize } from '@simlin/core/canonicalize';

import { Alias, AliasProps } from './Alias';
import { Aux, auxBounds, auxContains, AuxProps } from './Auxiliary';
import { Cloud, cloudBounds, cloudContains, CloudProps } from './Cloud';
import { isCloudOnSourceSide, isCloudOnSinkSide } from './cloud-utils';
import {
  calcViewBox,
  displayName,
  labelRadii,
  plainDeserialize,
  plainSerialize,
  Point,
  Rect,
  screenToCanvasPoint,
} from './common';
import { Connector, ConnectorProps, computeLinkCreationArc } from './Connector';
import { EditableLabel } from './EditableLabel';
import { Flow, flowBounds } from './Flow';
import { applyGroupMovement } from '../group-movement';
import { Group, groupBounds, GroupProps } from './Group';
import { Module, moduleBounds, moduleContains, ModuleProps } from './Module';
import { anyModuleHasModelReference } from '../module-warning';
import { CustomElement } from './SlateEditor';
import { Stock, stockBounds, stockContains, StockHeight, StockProps, StockWidth } from './Stock';
import { isDragMovement, shouldShowVariableDetails } from './pointer-utils';
import {
  VELOCITY_THRESHOLD,
  calculateVelocity as computeVelocity,
  isMomentumDone,
  momentumOffsetAt,
  pinchOffset,
  pinchZoom,
  resizeViewBox,
  wheelPanOffset,
  wheelZoom,
  zoomAroundPoint,
} from './viewport';
import { pointerStateReset, resolveSelectionForReattachment } from '../selection-logic';
import {
  computeDragSelection,
  decideMouseDownSelection,
  idleState,
  InteractionContext,
  InteractionEffect,
  InteractionState,
  isDrag,
  labelSideForPointer,
  reduceInteraction,
  resolveDeferredSelection,
} from './canvas-interaction';

import styles from './Canvas.module.css';

export const inCreationUid = -2;
export const fauxTargetUid = -3;
export const inCreationCloudUid = -4;
export const fauxCloudTargetUid = -5;

const fauxTarget: AuxViewElement = {
  type: 'aux',
  name: '$⁚model-internal-faux-target',
  ident: '$⁚model-internal-faux-target',
  uid: fauxTargetUid,
  var: undefined,
  x: 0,
  y: 0,
  labelSide: 'right' as LabelSide,
  isZeroRadius: true,
};

const fauxCloudTarget: CloudViewElement = {
  type: 'cloud',
  uid: fauxCloudTargetUid,
  flowUid: -1,
  x: 0,
  y: 0,
  isZeroRadius: true,
  ident: undefined,
};

// Pure bounds pass over the displayed elements, replacing the side-channel that
// used to populate this.elementBounds while rendering each element. Mirrors the
// per-type bounds calls in the element-rendering methods exactly: only cloud,
// aux, stock, module, group, and flow contribute bounds (links and aliases do
// not). Selection-update substitutions are applied first so drag-preview
// geometry feeds the embedded-mode tight viewBox, matching what buildLayers
// draws. Returns one entry per contributing element (undefined entries from
// *Bounds are kept; calcViewBox skips them).
function computeElementBounds(
  displayElements: readonly ViewElement[],
  selectionUpdates: ReadonlyMap<UID, ViewElement>,
): Array<Rect | undefined> {
  const bounds: Array<Rect | undefined> = [];
  for (let element of displayElements) {
    const updated = selectionUpdates.get(element.uid);
    if (updated !== undefined) {
      element = updated;
    }
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
      default:
        // link, alias: no bounds contribution (matches original render path)
        break;
    }
  }
  return bounds;
}

const ZMax = 6;

// Momentum physics, zoom limits, and the wheel/pinch math live in `viewport.ts`
// (the pure functional core); this shell resolves screen->canvas points and the
// rAF/timer lifecycle, then calls those pure transforms.

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
// Every cached/derived value the render path needs is produced here exactly
// once at the top of render(); the element-rendering helpers (connector(),
// aux(), ...) only *read* these, never recompute or mutate during render. This
// keeps render free of mid-render ref mutation beyond the single
// deriveRenderState writer.
interface RenderDerivation {
  // The elements to draw (props.view.elements plus any in-creation element).
  displayElements: readonly ViewElement[];
  // UID -> element lookup over displayElements plus the faux drag targets.
  // Reused at event-time (getElementByUid, handlers) -- see elementsRef.
  elementsByUid: Map<UID, ViewElement>;
  // Selected elements with live drag/label updates applied (group movement,
  // label-side, single-link arc suppression). Keyed by UID.
  selectionUpdates: Map<UID, ViewElement>;
  // AC1.6: whether any module in the model has a model reference, used to
  // suppress warning dots while a model is being sketched.
  hasAnyModuleReference: boolean;
  // The arc last computed for a single-link arrowhead drag (creation or
  // reattachment), or undefined when not dragging a link / straight line.
  // connector() renders this exact value and pointer-up persists it, so the
  // saved arc always matches the on-screen arc (see "Link drag arc ownership").
  draggedLinkArc: number | undefined;
}

export interface CanvasProps {
  embedded: boolean;
  project: Project;
  model: Model;
  view: StockFlowView;
  version: number;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;
  selection: ReadonlySet<UID>;
  onRenameVariable: (oldName: string, newName: string) => void;
  onSetSelection: (selected: ReadonlySet<UID>) => void;
  onMoveSelection: (position: Point, arcPoint?: Point, segmentIndex?: number) => void;
  onMoveFlow: (
    flow: FlowViewElement,
    targetUid: number,
    moveDelta: Point,
    fauxTargetCenter: Point | undefined,
    inCreation: boolean,
    isSourceAttach?: boolean,
  ) => void;
  onMoveLabel: (uid: UID, side: 'top' | 'left' | 'bottom' | 'right') => void;
  onAttachLink: (link: LinkViewElement, newTarget: string) => void;
  onCreateVariable: (element: ViewElement) => void;
  onClearSelectedTool: () => void;
  onDeleteSelection: () => void;
  onShowVariableDetails: () => void;
  onViewBoxChange: (viewBox: ViewRect, zoom: number) => void;
  onDrillIntoModule: (moduleIdent: string, targetModelName: string) => void;
}

// UID -> element lookup for resolving connector ends. Module-level pure function
// (formerly Canvas.buildSelectionMap, a static method). The skip rationale for
// inCreationUid / missing elements is preserved verbatim below.
export function buildSelectionMap(
  props: CanvasProps,
  elements: ReadonlyMap<UID, ViewElement>,
  inCreation?: ViewElement,
): Map<UID, ViewElement> {
  const selection = new Map<UID, ViewElement>();
  for (const uid of props.selection) {
    if (uid === inCreationUid) {
      if (inCreation) {
        selection.set(uid, inCreation);
      }
      // When inCreation is undefined the async Editor update hasn't
      // finished yet — skip this transient UID; the next render after
      // Editor.setState will carry the real selection.
      continue;
    }
    const e = elements.get(uid);
    if (e === undefined) {
      // The selection can transiently reference an element that has just
      // been removed from the view (e.g. dropping a connector's arrowhead
      // off-canvas deletes it): Editor updates the view and clears the
      // selection in separate setState calls, so there is a render in
      // between where props.view no longer has the element but
      // props.selection still does. Skip it rather than crashing the whole
      // canvas; the next render after the selection-clear lands is
      // consistent. (Same rationale as the inCreationUid case above.)
      continue;
    }
    selection.set(e.uid, e);
  }
  return selection;
}

// The mutable instance state that, in the class component, lived as instance
// fields (this.*) and was read by event handlers / native listeners / the
// momentum rAF loop AFTER render returned. Collected here so the function
// component can keep them in a single ref and the event-time readers share one
// "current" view -- exactly as `this.*` always reflected the latest values.
interface CanvasRefs {
  svgObserver: ResizeObserver | undefined;
  mouseDownPoint: Point | undefined;
  selectionCenterOffset: Point | undefined;
  pointerId: number | undefined;
  prevSelectedTool: CanvasProps['selectedTool'];

  // Cache key for the elements-by-uid lookup map: when props.version is
  // unchanged we reuse the existing map (and the displayElements array) rather
  // than rebuilding it. Owned exclusively by deriveRenderState().
  cachedVersion: number;

  // UID -> element lookup, populated by deriveRenderState() and intentionally
  // NOT cleared at the end of render: event handlers (getElementByUid and the
  // pointer callbacks) read it after render returns. Mirrors derived.elementsByUid.
  elements: Map<UID, ViewElement>;

  // The most recent render derivation. Written only by deriveRenderState();
  // read by the element-rendering helpers during render and by the pointer
  // handlers at event time.
  derived: RenderDerivation;

  // Multi-touch tracking for pinch gestures
  activePointers: Map<number, TrackedPointer>;

  // Momentum/inertia animation
  velocityTracker: VelocityTracker;
  momentumAnimationId: number | undefined;
  momentumStartTime: number | undefined;
  momentumInitialVelocity: Point | undefined;
  momentumStartOffset: Point | undefined;
}

// The snapshot of props + discrete/continuous state that event-time readers
// (native wheel/gesture listeners, the momentum rAF loop, the ResizeObserver,
// the deferred tool-change commit) must see CURRENT, not as captured by a stale
// render closure. Refreshed synchronously on every render so any escaped
// callback reads the same values `this.props` / `this.state` would have.
interface LatestState {
  props: CanvasProps;
  interaction: InteractionState;
  editingName: Array<Descendant>;
  dragSelectionPoint: Point | undefined;
  moveDelta: Point | undefined;
  movingCanvasOffset: Point | undefined;
  svgSize: Readonly<{ width: number; height: number }> | undefined;
  inCreation: ViewElement | undefined;
  inCreationCloud: CloudViewElement | undefined;
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

  // ---- Discrete + continuous state (formerly CanvasState) -----------------
  const [interaction, setInteraction] = React.useState<InteractionState>(idleState);
  const [editingName, setEditingName] = React.useState<Array<Descendant>>([]);
  const [dragSelectionPoint, setDragSelectionPoint] = React.useState<Point | undefined>(undefined);
  const [moveDelta, setMoveDelta] = React.useState<Point | undefined>(undefined);
  const [movingCanvasOffset, setMovingCanvasOffset] = React.useState<Point | undefined>(undefined);
  const [initialBounds, setInitialBounds] = React.useState<ViewRect>(viewRectDefault);
  const [svgSize, setSvgSize] = React.useState<Readonly<{ width: number; height: number }> | undefined>(undefined);
  const [inCreation, setInCreation] = React.useState<ViewElement | undefined>(undefined);
  const [inCreationCloud, setInCreationCloud] = React.useState<CloudViewElement | undefined>(undefined);

  // initialBounds is written in the mount effect and only read there; keep the
  // setter referenced to avoid an unused-var lint while preserving the field.
  void initialBounds;

  // ---- Instance fields (formerly this.*) as refs --------------------------
  const refs = React.useRef<CanvasRefs>(undefined as unknown as CanvasRefs);
  if (refs.current === undefined) {
    refs.current = {
      svgObserver: undefined,
      mouseDownPoint: undefined,
      selectionCenterOffset: undefined,
      pointerId: undefined,
      prevSelectedTool: undefined,
      cachedVersion: -Infinity,
      elements: new Map<UID, ViewElement>(),
      derived: {
        displayElements: [],
        elementsByUid: new Map<UID, ViewElement>(),
        selectionUpdates: new Map<UID, ViewElement>(),
        hasAnyModuleReference: false,
        draggedLinkArc: undefined,
      },
      activePointers: new Map<number, TrackedPointer>(),
      velocityTracker: { positions: [] },
      momentumAnimationId: undefined,
      momentumStartTime: undefined,
      momentumInitialVelocity: undefined,
      momentumStartOffset: undefined,
    };
    // Seed the empty derivation's elementsByUid to the same map instance, as the
    // class constructor did (derived.elementsByUid === this.elements).
    refs.current.derived.elementsByUid = refs.current.elements;
  }
  const r = refs.current;

  // ---- Latest props/state snapshot for escaped callbacks ------------------
  // Updated synchronously below on every render. Event handlers, native
  // listeners, the momentum loop, and the ResizeObserver all read through this
  // so they see CURRENT values (the class read this.props/this.state, which
  // were always current). Writing during render is safe: it is the same data
  // the JSX below renders, just exposed to non-render-scope callers.
  const latest = React.useRef<LatestState>(undefined as unknown as LatestState);
  latest.current = {
    props,
    interaction,
    editingName,
    dragSelectionPoint,
    moveDelta,
    movingCanvasOffset,
    svgSize,
    inCreation,
    inCreationCloud,
  };

  // ---- Discrete-interaction-mode accessors --------------------------------
  // The migration (#65) collapsed the former boolean CanvasState modes onto the
  // tagged-union interaction state. These narrow helpers keep the call sites
  // readable; they are the ONLY places that destructure the union mode, so the
  // render/handler code stays mode-agnostic. They take the interaction value
  // explicitly so render-time callers pass the render value and event-time
  // callers pass latest.current.interaction.

  // Dragging a link/flow arrowhead (sink) endpoint.
  const isDraggingArrowhead = (i: InteractionState): boolean => i.mode === 'movingEndpoint' && i.endpoint === 'arrow';

  // Dragging a flow source endpoint.
  const isDraggingSource = (i: InteractionState): boolean => i.mode === 'movingEndpoint' && i.endpoint === 'source';

  // The inline name editor is showing NOW. This reproduces the OLD boolean
  // `isEditingName` ("the inline editor is visible"), which was distinct from
  // `editNameOnPointerUp` ("enter editing AFTER this creation drag ends"). Both
  // map onto the `editingName` union variant, separated by `onPointerUp`: during
  // an aux/stock/module creation drag the variant is `editingName {onPointerUp:
  // true}` but the editor is NOT yet visible, so this MUST exclude that staging
  // case. Readers that drive the EditableLabel overlay, the label-suppression
  // props, the overlay's pointer-event capture, and the tool-change deferred
  // commit all want this "showing now" semantics -- never the staged handoff.
  // The pointer-up staging read uses `mode === 'editingName' && onPointerUp`
  // directly (the old `editNameOnPointerUp`), not this helper.
  const isShowingNameEditor = (i: InteractionState): boolean => i.mode === 'editingName' && !i.onPointerUp;

  // The pointer type captured at the start of an endpoint drag, or undefined
  // when not dragging an endpoint. Drives the touch-is-always-straight link
  // rule (touch links never get an arc).
  const getDragPointerType = (i: InteractionState): string | undefined =>
    i.mode === 'movingEndpoint' ? i.pointerType : undefined;

  // The flow segment being dragged (undefined = valve / whole element).
  const getDraggingSegmentIndex = (i: InteractionState): number | undefined =>
    i.mode === 'movingSelection' ? i.segmentIndex : undefined;

  // The active label-drag side, or undefined when not dragging a label.
  const getLabelSide = (i: InteractionState): 'right' | 'bottom' | 'left' | 'top' | undefined =>
    i.mode === 'movingLabel' ? i.side : undefined;

  // The read-only environment the pure reducer needs from the shell. Reads the
  // latest selection so a reducer call mid-handler sees current props.
  const interactionContext = (): InteractionContext => ({ selection: latest.current.props.selection });

  // Execute the discrete effects a reducer transition emitted, in order. The
  // reducer only ever emits `capturePointer` today (selection/tool changes are
  // done by the shell directly), so this is the lone arm.
  const runEffects = (effects: readonly InteractionEffect[], target: Element | undefined, pointerId: number): void => {
    for (const effect of effects) {
      switch (effect.kind) {
        case 'capturePointer':
          target?.setPointerCapture(pointerId);
          break;
      }
    }
  };

  // Apply the PointerStateReset bag (formerly `setState(pointerStateReset())`)
  // by calling the per-field setters. React batches them into one render. The
  // former loose instance fields (deferredSingleSelectUid, deferredIsText,
  // dragPointerType) now live inside the interaction union, reset by
  // pointerStateReset()'s `interaction: idle`.
  const applyPointerStateReset = (): void => {
    const reset = pointerStateReset();
    setInteraction(reset.interaction);
    setMoveDelta(reset.moveDelta);
    setDragSelectionPoint(reset.dragSelectionPoint);
    setInCreation(reset.inCreation);
    setInCreationCloud(reset.inCreationCloud);
  };

  const getCanvasOffset = (): Readonly<Point> => latest.current.movingCanvasOffset ?? latest.current.props.view.viewBox;

  const getElementByUid = (uid: UID): ViewElement => {
    let element: ViewElement | undefined;
    if (uid === inCreationUid) {
      element = latest.current.inCreation;
    } else if (uid === inCreationCloudUid) {
      element = latest.current.inCreationCloud;
    } else {
      element = r.elements.get(uid);
    }
    return defined(element);
  };

  const isSelected = (element: ViewElement): boolean => latest.current.props.selection.has(element.uid);

  const getCanvasPoint = (x: number, y: number): Point => {
    if (svgRef.current) {
      const bounds = svgRef.current.getBoundingClientRect();
      x -= bounds.x;
      y -= bounds.y;
    }
    return screenToCanvasPoint(x, y, latest.current.props.view.zoom);
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

  const focusCanvas = (): void => {
    // an SVG element can't actually be focused.  Instead, blur any _other_
    // focused element.
    if (typeof document !== 'undefined' && document && document.activeElement) {
      const activeElement = document.activeElement;
      if ('blur' in activeElement && typeof activeElement.blur === 'function') {
        activeElement.blur();
      }
    }
  };

  const getNewVariableName = (base: string): string => {
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

  // ---- isValidTarget / arc / link-drag helpers ----------------------------
  // These run during render (called by the element-rendering helpers) and at
  // event time (pointer-up resolution). They read the live moveDelta and
  // selectionCenterOffset; during render those reflect the current render, at
  // event time they reflect the latest committed values -- both via `latest`/`r`.

  const isValidTarget = (element: ViewElement): boolean | undefined => {
    const draggingArrowhead = isDraggingArrowhead(latest.current.interaction);
    const draggingSource = isDraggingSource(latest.current.interaction);

    if ((!draggingArrowhead && !draggingSource) || !r.selectionCenterOffset) {
      return undefined;
    }

    const arrowUid = only(latest.current.props.selection);
    const arrow = getElementByUid(arrowUid);

    const off = r.selectionCenterOffset;
    const delta = latest.current.moveDelta || { x: 0, y: 0 };
    const canvasOffset = getCanvasOffset();
    const pointer = {
      x: off.x - delta.x - canvasOffset.x,
      y: off.y - delta.y - canvasOffset.y,
    };

    let isTarget = false;
    if (element.type === 'cloud') {
      isTarget = cloudContains(element, pointer);
    } else if (element.type === 'stock') {
      isTarget = stockContains(element, pointer);
    } else if (element.type === 'module') {
      isTarget = moduleContains(element, pointer);
    } else if (element.type === 'aux') {
      isTarget = auxContains(element, pointer);
    } else if (element.type === 'flow') {
      isTarget = auxContains(element, pointer);
    }
    if (!isTarget) {
      return undefined;
    }

    // don't allow connectors from and to the same element
    if (arrow.type === 'link' && arrow.fromUid === element.uid) {
      return undefined;
    }

    // dont allow duplicate links between the same two elements
    if (arrow.type === 'link') {
      const { view } = latest.current.props;
      for (const e of view.elements) {
        // skip if its not a connector, or if it is the currently selected connector
        if (e.type !== 'link' || e.uid === arrow.uid) {
          continue;
        }

        if (e.fromUid === arrow.fromUid && e.toUid === element.uid) {
          return false;
        }
      }
    }

    if (arrow.type === 'flow') {
      if (element.type !== 'stock') {
        return false;
      }

      if (draggingSource) {
        // For source movement: check if target stock is valid source
        const lastPt = last(arrow.points);
        // Don't allow connecting source and sink to the same stock
        if (lastPt.attachedToUid === element.uid) {
          return false;
        }
        // For multi-segment flows (3+ points), the source needs to align with
        // the adjacent point (second), not the sink point. For 2-point flows,
        // points[1] gives us the last point, which is correct.
        const adjacentToSource = at(arrow.points, 1);
        return (
          Math.abs(adjacentToSource.x - element.x) < StockWidth / 2 ||
          Math.abs(adjacentToSource.y - element.y) < StockHeight / 2
        );
      } else {
        // For arrowhead movement: check if target stock is valid sink
        const firstPt = first(arrow.points);
        // make sure we don't point a flow back at its source
        if (firstPt.attachedToUid === element.uid) {
          return false;
        }
        // For multi-segment flows (3+ points), the arrowhead needs to align with
        // the adjacent point (second-to-last), not the source point. For 2-point
        // flows, points.length - 2 = 0 gives us the first point, which is correct.
        const adjacentToArrowhead = at(arrow.points, arrow.points.length - 2);
        return (
          Math.abs(adjacentToArrowhead.x - element.x) < StockWidth / 2 ||
          Math.abs(adjacentToArrowhead.y - element.y) < StockHeight / 2
        );
      }
    }

    return element.type === 'flow' || element.type === 'aux' || element.type === 'module';
  };

  const getArcPoint = (): FlowPoint | undefined => {
    if (!r.selectionCenterOffset) {
      return undefined;
    }
    const off = defined(r.selectionCenterOffset);
    const delta = latest.current.moveDelta ?? { x: 0, y: 0 };
    const canvasOffset = getCanvasOffset();
    return {
      x: off.x - delta.x - canvasOffset.x,
      y: off.y - delta.y - canvasOffset.y,
      attachedToUid: undefined,
    };
  };

  // The element the dragged single link's arrowhead is currently snapped to (a
  // valid aux/flow/module target under the cursor), or undefined for empty
  // space. A pure read over the displayed elements; shared by connector()
  // (visual `to` endpoint) and deriveDraggedLinkArc (arc computation) so both
  // agree on the snap target within a render.
  const findLinkDragTarget = (): ViewElement | undefined => {
    return r.derived.displayElements.find((e: ViewElement) => {
      if (e.type !== 'aux' && e.type !== 'flow' && e.type !== 'module') {
        return false;
      }
      return isValidTarget(e) || false;
    });
  };

  // Compute the arc for a single-link arrowhead drag exactly as connector()
  // renders it: an arc only when snapped to a valid target with a mouse
  // pointer (touch links are always straight), undefined otherwise. Writes
  // nothing; called once per render from deriveRenderState so connector() and
  // the pointer-up persist path read the identical value.
  const deriveDraggedLinkArc = (selectionUpdates: ReadonlyMap<UID, ViewElement>): number | undefined => {
    if (!isDraggingArrowhead(latest.current.interaction) || !r.selectionCenterOffset) {
      return undefined;
    }
    if (latest.current.props.selection.size !== 1) {
      return undefined;
    }
    const linkUid = only(latest.current.props.selection);
    let link = r.elements.get(linkUid);
    const updated = selectionUpdates.get(linkUid);
    if (updated !== undefined) {
      link = updated;
    }
    if (link === undefined || link.type !== 'link') {
      return undefined;
    }
    if (getDragPointerType(latest.current.interaction) === 'touch') {
      return undefined;
    }
    const validTarget = findLinkDragTarget();
    if (!validTarget) {
      return undefined;
    }
    const from = selectionUpdates.get(link.fromUid) || getElementByUid(link.fromUid);
    const arcPt = getArcPoint();
    return arcPt ? computeLinkCreationArc(from, validTarget, arcPt) : undefined;
  };

  // The single render-phase derivation step. Invoked once at the top of the
  // render body (and the mount effect); it is the ONLY code permitted to write
  // the render caches (r.elements, r.cachedVersion, r.derived). Every
  // element-rendering helper reads r.derived and never recomputes or mutates a
  // cache mid-render.
  const deriveRenderState = (): RenderDerivation => {
    const p = latest.current.props;
    const inCreationNow = latest.current.inCreation;
    const inCreationCloudNow = latest.current.inCreationCloud;
    let displayElements: readonly ViewElement[] = p.view.elements;
    if (inCreationNow) {
      displayElements = [...displayElements, inCreationNow];
    }
    if (inCreationCloudNow) {
      displayElements = [...displayElements, inCreationCloudNow];
    }

    // Rebuild the uid lookup only when the project version changed. r.elements
    // is held across renders because event handlers read it after render returns
    // ("n.b. we don't want to clear r.elements"). The displayElements array
    // identity must track the same key, so cache both together.
    if (p.version !== r.cachedVersion) {
      const elements = new Map<UID, ViewElement>(displayElements.map((el) => [el.uid, el]));
      elements.set(fauxTarget.uid, fauxTarget);
      elements.set(fauxCloudTarget.uid, fauxCloudTarget);
      r.elements = elements;
      r.cachedVersion = p.version;
    }

    let selectionUpdates = buildSelectionMap(p, r.elements, inCreationNow);
    const activeLabelSide = getLabelSide(latest.current.interaction);
    if (activeLabelSide) {
      selectionUpdates = mapValues(selectionUpdates, (el) => {
        return { ...el, labelSide: activeLabelSide } as ViewElement;
      }) as Map<UID, ViewElement>;
    }
    if (latest.current.moveDelta) {
      const moveDeltaValue = defined(latest.current.moveDelta);

      // When dragging a single link arrow (creation or reattachment),
      // suppress arcPoint so processLinks doesn't compute a rotation-based
      // arc.  connector() handles arc computation directly.
      const isDraggingLink = isDraggingArrowhead(latest.current.interaction) && p.selection.size === 1;
      const { updatedElements } = applyGroupMovement({
        elements: r.elements.values(),
        selection: p.selection,
        delta: moveDeltaValue,
        arcPoint: isDraggingLink ? undefined : getArcPoint(),
        segmentIndex: getDraggingSegmentIndex(latest.current.interaction),
      });

      selectionUpdates = new Map([...selectionUpdates, ...updatedElements]);
    }

    const derived: RenderDerivation = {
      displayElements,
      elementsByUid: r.elements,
      selectionUpdates,
      hasAnyModuleReference: anyModuleHasModelReference(p.model.variables),
      draggedLinkArc: undefined,
    };
    // Publish before computing the dragged-link arc: deriveDraggedLinkArc reads
    // r.derived.displayElements (via findLinkDragTarget) and selectionUpdates.
    r.derived = derived;
    derived.draggedLinkArc = deriveDraggedLinkArc(selectionUpdates);

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

    // Stop when the decayed momentum speed drops below threshold.
    if (isMomentumDone(v0, elapsed)) {
      stopMomentumAnimation();
      return;
    }

    // Note: the friction displacement is ADDED because a higher offset moves the
    // view in the positive direction, while velocity is in screen coordinates
    // where dragging right should move the view left.
    const newOffset = momentumOffsetAt(r.momentumStartOffset, v0, elapsed);

    // Update viewBox with new offset
    const newViewBox = {
      ...latest.current.props.view.viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };
    latest.current.props.onViewBoxChange(newViewBox, latest.current.props.view.zoom);

    // Continue animation
    r.momentumAnimationId = window.requestAnimationFrame(animateMomentum);
  };

  // Start momentum animation after pan release
  const startMomentumAnimation = (): void => {
    // Cancel any existing momentum animation first (defensive)
    stopMomentumAnimation();

    const velocity = calculateVelocity();
    const speed = Math.sqrt(velocity.x * velocity.x + velocity.y * velocity.y);

    // Don't start animation if velocity is at or below threshold
    if (speed <= VELOCITY_THRESHOLD) {
      return;
    }

    r.momentumInitialVelocity = velocity;
    r.momentumStartOffset = { ...getCanvasOffset() };
    r.momentumStartTime = window.performance.now();

    r.momentumAnimationId = window.requestAnimationFrame(animateMomentum);
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
    const interactionNow = latest.current.interaction;
    if (interactionNow.mode !== 'pinching') {
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

    const newViewBox = {
      ...latest.current.props.view.viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };

    latest.current.props.onViewBoxChange(newViewBox, newZoom);
  };

  // ---- Native wheel / Safari-gesture listeners (registered at mount) ------

  const handleWheelPan = (e: WheelEvent): void => {
    const zoom = latest.current.props.view.zoom;
    const viewBox = latest.current.props.view.viewBox;

    // Page deltas (deltaMode 2) scroll a full viewport; measure it from the DOM
    // since the stored viewBox size may be stale during a resize transition.
    const viewportPx = {
      width: svgRef.current?.clientWidth ?? viewBox.width,
      height: svgRef.current?.clientHeight ?? viewBox.height,
    };
    const newOffset = wheelPanOffset(viewBox, { x: e.deltaX, y: e.deltaY, mode: e.deltaMode }, zoom, viewportPx);

    const newViewBox = {
      ...viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };

    latest.current.props.onViewBoxChange(newViewBox, zoom);
  };

  // Native wheel zoom handler using exponential scaling for natural macOS feel.
  // Exponential scaling ensures symmetric behavior: zoom in 2x then out 2x returns to original.
  const handleNativeWheelZoom = (e: WheelEvent): void => {
    const zoom = latest.current.props.view.zoom;

    // Exponential scaling (negative deltaY = pinch out = zoom in), clamped, with
    // an epsilon no-op at the zoom limits.
    const { zoom: newZoom, changed } = wheelZoom(zoom, e.deltaY);
    if (!changed) {
      return;
    }

    // Keep the model point under the cursor fixed across the zoom change: map the
    // same screen pixel into canvas space at both the old and new zoom.
    const cursorCanvas = getCanvasPoint(e.clientX, e.clientY);
    const viewBox = latest.current.props.view.viewBox;
    const newCursorCanvas = getCanvasPointWithZoom(e.clientX, e.clientY, newZoom);
    const newOffset = zoomAroundPoint(viewBox, cursorCanvas, newCursorCanvas);

    const newViewBox = {
      ...viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };

    latest.current.props.onViewBoxChange(newViewBox, newZoom);
  };

  // Native wheel event handler with { passive: false } to ensure preventDefault works.
  // React's synthetic onWheel handler is passive by default, so we must use native events.
  const handleNativeWheel = (e: WheelEvent): void => {
    if (latest.current.props.embedded) {
      return;
    }

    // Always prevent default to stop browser zoom, even at zoom limits
    e.preventDefault();

    // Stop any momentum animation when user starts interacting
    stopMomentumAnimation();

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
    if (oldSize) {
      const dWidth = contentRect.width - oldSize.width;
      const dHeight = contentRect.height - oldSize.height;
      const newViewBox = resizeViewBox(getCanvasOffset(), dWidth, dHeight, contentRect.width, contentRect.height);

      latest.current.props.onViewBoxChange(newViewBox, latest.current.props.view.zoom);
    }

    setSvgSize(newSvgSize);
  };

  // ---- Pointer handlers ---------------------------------------------------

  const clearPointerState = (clearSelection = true): void => {
    r.pointerId = undefined;
    r.mouseDownPoint = undefined;
    r.selectionCenterOffset = undefined;

    applyPointerStateReset();

    if (clearSelection) {
      latest.current.props.onSetSelection(new Set());
    }

    focusCanvas();
  };

  const handlePointerCancel = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    // Remove this pointer from tracking
    r.activePointers.delete(e.pointerId);

    // Handle end of pinch gesture
    if (latest.current.interaction.mode === 'pinching') {
      // When exiting pinch mode, clear all gesture state for a clean restart.
      // Continuing with a single finger after pinch leads to confusing UX.
      const { state: nextInteraction } = reduceInteraction(
        latest.current.interaction,
        { kind: 'pinchEnd' },
        interactionContext(),
      );
      setInteraction(nextInteraction);
      r.activePointers.clear();
      r.pointerId = undefined;
      r.mouseDownPoint = undefined;
      return;
    }

    if (r.pointerId === undefined || r.pointerId !== e.pointerId) {
      return;
    }

    const showDetails = shouldShowVariableDetails(
      r.selectionCenterOffset !== undefined,
      latest.current.moveDelta,
      latest.current.props.view.zoom,
      isDraggingArrowhead(latest.current.interaction),
      isDraggingSource(latest.current.interaction),
      latest.current.interaction.mode === 'movingLabel',
    );

    r.pointerId = undefined;

    // Resolve deferred selection: if user clicked an already-selected element
    // without modifier, we deferred the selection change to allow group drag.
    // Now on mouseUp, if no drag occurred, collapse to the single element. The
    // deferred fields now live in the movingSelection union variant.
    const interactionNow = latest.current.interaction;
    if (interactionNow.mode === 'movingSelection' && interactionNow.deferredSingleSelectUid !== undefined) {
      const didDrag = isDrag(latest.current.moveDelta, latest.current.props.view.zoom);
      const newSel = resolveDeferredSelection(interactionNow.deferredSingleSelectUid, didDrag);
      const wasDeferredText = interactionNow.deferredIsText;
      // Drop the deferred fields by collapsing the movingSelection variant to a
      // plain one (segmentIndex stays so a drag still moves the right segment).
      if (newSel) {
        latest.current.props.onSetSelection(newSel);
        if (wasDeferredText && newSel.size === 1) {
          const uid = only(newSel);
          const el = getElementByUid(uid);
          if (!isNamedViewElement(el)) {
            // Clouds and other non-named elements can't enter text editing
            r.selectionCenterOffset = undefined;
            applyPointerStateReset();
            return;
          }
          const nextEditingName = plainDeserialize('label', displayName(defined((el as NamedViewElement).name)));
          setInteraction({ mode: 'editingName', onPointerUp: false, creatingFlow: false });
          setEditingName(nextEditingName);
          setMoveDelta(undefined);
          r.selectionCenterOffset = undefined;
          return;
        }
      }
    }

    if (interactionNow.mode === 'movingLabel') {
      const selected = only(latest.current.props.selection);
      latest.current.props.onMoveLabel(selected, interactionNow.side);
      clearPointerState(false);
      return;
    }

    if (r.selectionCenterOffset) {
      if (latest.current.moveDelta) {
        const arcPoint = getArcPoint();
        const delta = latest.current.moveDelta;
        // The mode after committing the move: idle, unless we hand off into name
        // editing (creation tool, or a just-created flow). Computed once because
        // every boolean that used to be cleared piecemeal now lives in the union.
        let nextInteraction: InteractionState = idleState;

        if (interactionNow.mode === 'editingName' && interactionNow.onPointerUp) {
          let inCreationLocal = latest.current.inCreation;
          if (
            inCreationLocal !== undefined &&
            (inCreationLocal.type === 'stock' || inCreationLocal.type === 'aux' || inCreationLocal.type === 'module')
          ) {
            inCreationLocal = {
              ...inCreationLocal,
              x: inCreationLocal.x - delta.x,
              y: inCreationLocal.y - delta.y,
            };
          } else {
            throw new Error('invariant broken');
          }

          const nextEditingName = plainDeserialize(
            'label',
            displayName(defined((inCreationLocal as NamedViewElement).name)),
          );
          setInteraction({ mode: 'editingName', onPointerUp: false, creatingFlow: false });
          setEditingName(nextEditingName);
          setInCreation(inCreationLocal);
          setMoveDelta(undefined);
          r.selectionCenterOffset = undefined;
          // we do weird one off things in this codepath, so exit early
          return;
        } else if (!isDraggingArrowhead(interactionNow) && !isDraggingSource(interactionNow)) {
          // A sub-threshold pointer wobble during a click is not a drag: don't
          // nudge the element. shouldShowVariableDetails (which applies the
          // same threshold) will open the details panel for it instead.
          if (isDragMovement(delta, latest.current.props.view.zoom)) {
            latest.current.props.onMoveSelection(delta, arcPoint, getDraggingSegmentIndex(interactionNow));
          }
        } else {
          const element = getElementByUid(only(latest.current.props.selection));
          let foundInvalidTarget = false;
          const validTarget = r.derived.displayElements.find((el: ViewElement) => {
            const isValid = isValidTarget(el);
            foundInvalidTarget = foundInvalidTarget || isValid === false;
            return isValid || false;
          });
          if (element.type === 'link' && validTarget) {
            // Use the arc that was last rendered — computed once per render in
            // deriveRenderState (derived.draggedLinkArc) and drawn by connector()
            // — so the saved link matches the visual exactly. Works for both
            // new-link creation and existing-link reattachment.
            const linkToAttach = { ...element, arc: r.derived.draggedLinkArc };
            latest.current.props.onAttachLink(linkToAttach, defined(validTarget.ident));
          } else if (element.type === 'flow') {
            // don't create a flow stacked on top of 2 clouds due to a misclick
            // (a click that wobbled a pixel is still a misclick, not a drag)
            if (
              !isDragMovement(latest.current.moveDelta, latest.current.props.view.zoom) &&
              latest.current.inCreation
            ) {
              clearPointerState();
              return;
            }
            const inCreationFlag = !!latest.current.inCreation;
            const isSourceAttach = isDraggingSource(interactionNow);
            let fauxTargetCenter: Point | undefined;
            if (element.points[1]?.attachedToUid === fauxCloudTargetUid) {
              const canvasOffset = getCanvasOffset();
              fauxTargetCenter = {
                x: r.selectionCenterOffset.x - canvasOffset.x,
                y: r.selectionCenterOffset.y - canvasOffset.y,
              };
            }
            // For source movement when not snapped to a valid target, compute the faux source center
            if (isSourceAttach && !validTarget) {
              const canvasOffset = getCanvasOffset();
              fauxTargetCenter = {
                x: r.selectionCenterOffset.x - canvasOffset.x,
                y: r.selectionCenterOffset.y - canvasOffset.y,
              };
            }
            latest.current.props.onMoveFlow(
              element,
              validTarget ? validTarget.uid : 0,
              delta,
              fauxTargetCenter,
              inCreationFlag,
              isSourceAttach,
            );
            if (inCreationFlag) {
              // Hand off into editing the just-created flow's name. creatingFlow
              // (formerly flowStillBeingCreated) makes a later name-cancel delete
              // the flow. The editingName Slate value is carried alongside.
              nextInteraction = { mode: 'editingName', onPointerUp: false, creatingFlow: true };
              setEditingName(plainDeserialize('label', displayName(defined(element.name))));
            }
          } else if (!foundInvalidTarget || latest.current.inCreation) {
            latest.current.props.onDeleteSelection();
          }
        }

        // Single coalesced commit: the discrete mode (idle, or the editingName
        // hand-off computed above) plus the continuous companions that travel
        // with a move. Replaces the former piecemeal isMovingArrow / isMovingSource
        // / draggingSegmentIndex clears -- those all collapse into `interaction`.
        // React batches these setters into one render with the net state.
        setInteraction(nextInteraction);
        setMoveDelta(undefined);
        setInCreation(undefined);
        setInCreationCloud(undefined);
      } else if (isDraggingArrowhead(interactionNow) || isDraggingSource(interactionNow)) {
        // User clicked on flow arrowhead/source (or cloud) but didn't move.
        // Clear the movement mode so the cloud reappears.
        setInteraction(idleState);
      }
      r.selectionCenterOffset = undefined;
      if (showDetails) {
        latest.current.props.onShowVariableDetails();
      }
      return;
    }

    if (interactionNow.mode === 'panning' && latest.current.movingCanvasOffset) {
      const newViewBox = {
        ...latest.current.props.view.viewBox,
        x: latest.current.movingCanvasOffset.x,
        y: latest.current.movingCanvasOffset.y,
      };

      latest.current.props.onViewBoxChange(newViewBox, latest.current.props.view.zoom);
      setMovingCanvasOffset(undefined);

      // Start momentum animation for smooth deceleration
      startMomentumAnimation();
    }

    if (!r.mouseDownPoint) {
      return;
    }

    // Handle drag selection
    if (interactionNow.mode === 'dragSelecting' && latest.current.dragSelectionPoint) {
      const pointA = r.mouseDownPoint;
      const pointB = latest.current.dragSelectionPoint;
      const canvasOffset = getCanvasOffset();

      // Calculate selection rectangle bounds
      const left = Math.min(pointA.x, pointB.x) - canvasOffset.x;
      const right = Math.max(pointA.x, pointB.x) - canvasOffset.x;
      const top = Math.min(pointA.y, pointB.y) - canvasOffset.y;
      const bottom = Math.max(pointA.y, pointB.y) - canvasOffset.y;

      // Find all elements within the selection rectangle. Each element type's
      // containment rule lives in canvas-interaction.isInDragSelectRect; auxes
      // additionally count when any rectangle corner falls inside the aux
      // circle (a geometry test the shell owns via auxContains).
      const rect = { left, right, top, bottom };
      const auxCornerHit = (element: ViewElement): boolean =>
        auxContains(element as AuxViewElement, { x: left, y: top }) ||
        auxContains(element as AuxViewElement, { x: right, y: top }) ||
        auxContains(element as AuxViewElement, { x: left, y: bottom }) ||
        auxContains(element as AuxViewElement, { x: right, y: bottom });
      const selectedElements = computeDragSelection(r.derived.displayElements, rect, auxCornerHit);

      // Update selection
      latest.current.props.onSetSelection(selectedElements);
      clearPointerState(false);
      return;
    }

    // A pan must not clear the selection; everything reaching here does. The
    // panning branch above only cleared movingCanvasOffset, so the mode is still
    // 'panning' here (mirrors the former `!this.state.isMovingCanvas`).
    const clearSelection = interactionNow.mode !== 'panning';
    clearPointerState(clearSelection);
  };

  const handleSelectionMove = (e: React.PointerEvent<SVGElement>): void => {
    if (!r.selectionCenterOffset) {
      return;
    }

    const currPt = getCanvasPoint(e.clientX, e.clientY);

    const dx = r.selectionCenterOffset.x - currPt.x;
    const dy = r.selectionCenterOffset.y - currPt.y;

    setMoveDelta({
      x: dx,
      y: dy,
    });
  };

  const handleMovingCanvas = (e: React.PointerEvent<SVGElement>): void => {
    if (!r.mouseDownPoint) {
      return;
    }

    const base = latest.current.props.view.viewBox;
    const curr = getCanvasPoint(e.clientX, e.clientY);

    const newOffset = {
      x: base.x + (curr.x - r.mouseDownPoint.x),
      y: base.y + (curr.y - r.mouseDownPoint.y),
    };

    // Track position for momentum calculation
    trackPosition(newOffset.x, newOffset.y);

    // The panning mode was already entered on pointer-down; re-affirm it (it is
    // the move-guard in handlePointerMove) alongside the continuous offset.
    setInteraction({ mode: 'panning' });
    setMovingCanvasOffset(newOffset);
  };

  const handleDragSelection = (e: React.PointerEvent<SVGElement>): void => {
    if (!r.mouseDownPoint) {
      return;
    }

    const nextDragSelectionPoint = getCanvasPoint(e.clientX, e.clientY);

    setInteraction({ mode: 'dragSelecting' });
    setDragSelectionPoint(nextDragSelectionPoint);
  };

  const handlePointerMove = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }

    // Update tracked pointer position
    if (r.activePointers.has(e.pointerId)) {
      r.activePointers.set(e.pointerId, {
        id: e.pointerId,
        x: e.clientX,
        y: e.clientY,
        timestamp: window.performance.now(),
      });
    }

    // Handle pinch gesture
    if (latest.current.interaction.mode === 'pinching' && r.activePointers.size >= 2) {
      handlePinchMove();
      return;
    }

    if (r.pointerId !== e.pointerId) {
      return;
    } else if (r.pointerId && e.pointerType === 'mouse' && e.buttons === 0) {
      handlePointerCancel(e);
    }

    if (r.selectionCenterOffset) {
      handleSelectionMove(e);
    } else if (latest.current.interaction.mode === 'dragSelecting') {
      handleDragSelection(e);
    } else if (latest.current.interaction.mode === 'panning') {
      handleMovingCanvas(e);
    }
  };

  const handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    if (latest.current.props.embedded) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    // Stop any momentum animation when user starts interacting
    stopMomentumAnimation();

    // Track this pointer for multi-touch detection
    r.activePointers.set(e.pointerId, {
      id: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      timestamp: window.performance.now(),
    });

    // Check for pinch gesture (two touches)
    if (r.activePointers.size === 2 && e.pointerType === 'touch') {
      // Start pinch mode - clear all single-finger gesture state to prevent
      // simultaneous pan+pinch or drag+pinch if user adds second finger mid-gesture
      r.pointerId = undefined;
      r.mouseDownPoint = undefined;
      r.selectionCenterOffset = undefined;
      // Reset velocity tracker since pinch doesn't use momentum
      r.velocityTracker.positions = [];

      const distance = getPinchDistance();
      const center = getPinchCenter();
      const centerCanvas = getCanvasPoint(center.x, center.y);
      const viewBox = latest.current.props.view.viewBox;

      // Calculate the MODEL point under the pinch center. This is the fixed
      // point in model space that should remain under the user's fingers
      // throughout the pinch gesture.
      const pinchModelPoint = {
        x: centerCanvas.x - viewBox.x,
        y: centerCanvas.y - viewBox.y,
      };

      // Entering pinch mode supersedes any single-finger panning/dragSelecting
      // mode; the reducer returns the pinching variant carrying the fixed
      // reference. Clear movingCanvasOffset so exiting pinch can't start momentum.
      const { state: nextInteraction, effects } = reduceInteraction(
        latest.current.interaction,
        {
          kind: 'pinchStart',
          initialDistance: distance,
          initialZoom: latest.current.props.view.zoom,
          modelPoint: pinchModelPoint,
        },
        interactionContext(),
      );
      runEffects(effects, e.target as Element | undefined, e.pointerId);
      setInteraction(nextInteraction);
      setMovingCanvasOffset(undefined);
      return;
    }

    // If already pinching and a third finger comes in, ignore it
    if (latest.current.interaction.mode === 'pinching') {
      return;
    }

    // For non-primary touches when we already have a primary, track for potential pinch
    if (!e.isPrimary && r.pointerId !== undefined) {
      return;
    }

    const client = getCanvasPoint(e.clientX, e.clientY);

    const canvasOffset = getCanvasOffset();
    const { selectedTool } = latest.current.props;
    if (selectedTool === 'aux' || selectedTool === 'stock' || selectedTool === 'module') {
      let inCreationLocal: AuxViewElement | StockViewElement | ModuleViewElement;
      if (selectedTool === 'aux') {
        const name = getNewVariableName('New Variable');
        inCreationLocal = {
          type: 'aux',
          uid: inCreationUid,
          var: undefined,
          x: client.x - canvasOffset.x,
          y: client.y - canvasOffset.y,
          name,
          ident: canonicalize(name),
          labelSide: 'right',
          isZeroRadius: false,
        };
      } else if (selectedTool === 'stock') {
        const name = getNewVariableName('New Stock');
        inCreationLocal = {
          type: 'stock',
          uid: inCreationUid,
          var: undefined,
          x: client.x - canvasOffset.x,
          y: client.y - canvasOffset.y,
          name,
          ident: canonicalize(name),
          labelSide: 'bottom',
          isZeroRadius: false,
          inflows: [],
          outflows: [],
        };
      } else {
        const name = getNewVariableName('New Module');
        inCreationLocal = {
          type: 'module',
          uid: inCreationUid,
          var: undefined,
          x: client.x - canvasOffset.x,
          y: client.y - canvasOffset.y,
          name,
          ident: canonicalize(name),
          labelSide: 'bottom',
          isZeroRadius: false,
        };
      }

      r.pointerId = e.pointerId;
      r.selectionCenterOffset = client;

      // The creation-tool press enters the editing-on-pointer-up handoff and
      // captures the pointer (the capturePointer effect runs setPointerCapture).
      // The staged element + zero moveDelta are the continuous companions the
      // shell owns.
      const { state: nextInteraction, effects } = reduceInteraction(
        latest.current.interaction,
        { kind: 'createToolPointerDown', tool: selectedTool },
        interactionContext(),
      );
      runEffects(effects, e.target as Element | undefined, e.pointerId);
      setInteraction(nextInteraction);
      setInCreation(inCreationLocal);
      setMoveDelta({ x: 0, y: 0 });
      latest.current.props.onSetSelection(new Set([inCreationLocal.uid]));
      return;
    }
    r.pointerId = e.pointerId;

    if (selectedTool === 'flow') {
      const canvasOffsetFlow = getCanvasOffset();
      const x = client.x - canvasOffsetFlow.x;
      const y = client.y - canvasOffsetFlow.y;

      const inCreationCloudLocal: CloudViewElement = {
        type: 'cloud',
        uid: inCreationCloudUid,
        flowUid: inCreationUid,
        x,
        y,
        isZeroRadius: false,
        ident: undefined,
      };

      const name = getNewVariableName('New Flow');
      const inCreationLocal: FlowViewElement = {
        type: 'flow',
        uid: inCreationUid,
        var: undefined,
        name,
        ident: canonicalize(name),
        x,
        y,
        labelSide: 'bottom',
        points: [
          { x, y, attachedToUid: inCreationCloudLocal.uid },
          { x, y, attachedToUid: fauxCloudTarget.uid },
        ],
        isZeroRadius: false,
      };

      r.selectionCenterOffset = client;

      // Flow tool on empty canvas: enter arrowhead-drag of the staged flow so the
      // user drags the sink into place (no pointer capture in this branch, as
      // before). The staged flow + source cloud are the continuous companions.
      const { state: nextInteraction, effects } = reduceInteraction(
        latest.current.interaction,
        { kind: 'flowToolPointerDown', pointerType: e.pointerType },
        interactionContext(),
      );
      runEffects(effects, e.target as Element | undefined, e.pointerId);
      setInteraction(nextInteraction);
      setInCreation(inCreationLocal);
      setInCreationCloud(inCreationCloudLocal);
      setMoveDelta({ x: 0, y: 0 });
      latest.current.props.onSetSelection(new Set([inCreationLocal.uid]));
      return;
    }

    // onclick handlers are weird.  If we mouse down on a circle, move
    // off the circle, and mouse-up on the canvas, the canvas gets an
    // onclick.  Instead, capture where we mouse-down'd, and on mouse up
    // check if its the same.
    r.mouseDownPoint = getCanvasPoint(e.clientX, e.clientY);

    // Discrete decision: touch / shift-drag pans, everything else rubber-band
    // drag-selects. Routed through the pure reducer so the pan-vs-select rule
    // lives in canvas-interaction; the continuous pan offset + momentum stay in
    // the shell.
    const pan = e.pointerType === 'touch' || e.shiftKey;
    const { state: nextInteraction, effects } = reduceInteraction(
      idleState,
      { kind: 'canvasPointerDown', pan },
      interactionContext(),
    );
    runEffects(effects, e.target as Element | undefined, e.pointerId);
    if (nextInteraction.mode === 'panning') {
      // Initialize velocity tracking for momentum
      r.velocityTracker.positions = [];
      const canvasOffsetPan = getCanvasOffset();
      trackPosition(canvasOffsetPan.x, canvasOffsetPan.y);
    }
    // The pan-vs-drag-select mode came from the reducer; the in-creation
    // companions are cleared regardless (an empty-canvas press stages nothing).
    setInteraction(nextInteraction);
    setInCreation(undefined);
    setInCreationCloud(undefined);
  };

  const handleModuleDoubleClick = (element: ModuleViewElement): void => {
    const variable = latest.current.props.model.variables.get(element.ident);
    if (variable?.type !== 'module' || !variable.modelName) {
      return;
    }
    latest.current.props.onDrillIntoModule(element.ident, variable.modelName);
  };

  const handleLabelDrag = (uid: number, e: React.PointerEvent<SVGElement>): void => {
    r.pointerId = e.pointerId;

    const selectionSet = new Set([uid]);
    if (!setsEqual(latest.current.props.selection, selectionSet)) {
      latest.current.props.onSetSelection(selectionSet);
    }

    const element = getElementByUid(uid);
    const delta = getCanvasOffset();
    const client = getCanvasPoint(e.clientX, e.clientY);
    const pointer = {
      x: client.x - delta.x,
      y: client.y - delta.y,
    };

    const side = labelSideForPointer({ x: element.x, y: element.y }, pointer);

    const { state: nextInteraction, effects } = reduceInteraction(
      latest.current.interaction,
      { kind: 'labelDragStart', side },
      interactionContext(),
    );
    runEffects(effects, e.target as Element | undefined, e.pointerId);
    setInteraction(nextInteraction);
  };

  const handleEditingEnd = (e: React.PointerEvent<HTMLDivElement>): void => {
    e.preventDefault();
    e.stopPropagation();

    handleEditingNameDone(false);
  };

  const handleEditConnector = (element: ViewElement, e: React.PointerEvent<SVGElement>, isArrowhead: boolean): void => {
    handleSetSelection(element, e, false, isArrowhead);
  };

  // called from handleMouseDown in elements like Aux
  const handleSetSelection = (
    element: ViewElement,
    e: React.PointerEvent<SVGElement>,
    isText?: boolean,
    isArrowhead?: boolean,
    segmentIndex?: number,
    isSource?: boolean,
  ): void => {
    if (latest.current.props.embedded) {
      return;
    }

    // These locals track the discrete outcome the way the pre-migration code did
    // (mutually-exclusive booleans); they are folded into a single interaction
    // variant at the end. The shell owns the geometry/hit-testing here (cloud
    // reattachment, staged tool elements, Slate name deserialize) and composes
    // the pure selection decisions (decideMouseDownSelection,
    // resolveSelectionForReattachment); the discrete *mode* it lands in is then
    // expressed through the tagged union, not loose flags.
    let isEditingName = !!isText;
    let nextEditingName: Array<CustomElement> = [];
    let draggingArrowEndpoint = !!isArrowhead;
    let draggingSourceEndpoint = !!isSource;

    r.pointerId = e.pointerId;

    // For multi-selection, use the click point as the offset
    // This ensures smooth dragging from where the user clicked
    r.selectionCenterOffset = getCanvasPoint(e.clientX, e.clientY);

    if (!isEditingName) {
      (e.target as Element).setPointerCapture(e.pointerId);
    }

    const { selectedTool } = latest.current.props;
    let inCreationLocal: ViewElement | undefined;

    if (selectedTool === 'link' && isNamedViewElement(element)) {
      isEditingName = false;
      draggingArrowEndpoint = true;
      inCreationLocal = {
        type: 'link',
        uid: inCreationUid,
        fromUid: element.uid,
        toUid: fauxTarget.uid,
        arc: 0.0,
        multiPoint: undefined,
        isStraight: false,
        polarity: undefined,
        x: 0,
        y: 0,
        isZeroRadius: false,
        ident: undefined,
      };
      element = inCreationLocal;
    } else if (selectedTool === 'flow' && element.type === 'stock') {
      isEditingName = false;
      draggingArrowEndpoint = true;
      const startPoint: FlowPoint = {
        x: element.x,
        y: element.y,
        attachedToUid: element.uid,
      };
      const endPoint: FlowPoint = {
        x: element.x,
        y: element.y,
        attachedToUid: fauxCloudTarget.uid,
      };
      const name = getNewVariableName('New Flow');
      inCreationLocal = {
        type: 'flow',
        uid: inCreationUid,
        var: undefined,
        name: name,
        ident: canonicalize(name),
        x: element.x,
        y: element.y,
        labelSide: 'bottom',
        points: [startPoint, endPoint],
        isZeroRadius: false,
      };
      element = inCreationLocal;
    } else {
      // Not a link/flow tool action -- compute selection and handle clouds
      latest.current.props.onClearSelectedTool();

      const isMultiSelect = e.ctrlKey || e.metaKey || e.shiftKey;
      const { newSelection, deferSingleSelect } = decideMouseDownSelection(
        latest.current.props.selection,
        element.uid,
        isMultiSelect,
      );

      if (deferSingleSelect !== undefined) {
        // Element is already in the selection and no modifier -- defer selection
        // change to mouseUp so that group drag works without dissolving selection.
        // The deferred fields ride inside the movingSelection variant now.
        setInteraction({
          mode: 'movingSelection',
          deferredSingleSelectUid: deferSingleSelect,
          deferredIsText: !!isText,
          segmentIndex,
        });
        setEditingName(nextEditingName);
        setInCreation(inCreationLocal);
        setMoveDelta({ x: 0, y: 0 });
        return;
      }

      // Cloud re-attachment only when the cloud will be the sole selection
      const willBeSoleSelection = newSelection !== undefined && newSelection.size === 1;
      if (element.type === 'cloud' && element.flowUid !== undefined && willBeSoleSelection) {
        let flow: FlowViewElement | undefined;
        try {
          const flowElement = getElementByUid(element.flowUid);
          if (flowElement.type === 'flow') {
            flow = flowElement;
          }
        } catch (err) {
          console.warn(`Cloud ${element.uid} references invalid flow ${element.flowUid}:`, err);
        }
        if (flow) {
          if (isCloudOnSourceSide(element, flow)) {
            draggingSourceEndpoint = true;
            element = flow;
          } else if (isCloudOnSinkSide(element, flow)) {
            draggingArrowEndpoint = true;
            element = flow;
          }
        }
      }

      // Only allow editing name if single selection of a named element
      if (isEditingName && newSelection !== undefined && newSelection.size === 1) {
        const uid = only(newSelection);
        const editingElement = getElementByUid(uid) as NamedViewElement;
        nextEditingName = plainDeserialize('label', displayName(defined(editingElement.name)));
      } else {
        isEditingName = false;
      }

      if (newSelection !== undefined) {
        const enteredReattachment = draggingSourceEndpoint || draggingArrowEndpoint;
        latest.current.props.onSetSelection(
          resolveSelectionForReattachment(newSelection, enteredReattachment, element.uid),
        );
      }
    }

    // Fold the mutually-exclusive outcome into one interaction variant:
    //  - an endpoint drag (arrowhead/source, link/flow tool, cloud reattach)
    //  - inline name editing (double-click on a single named element)
    //  - otherwise a (potential) selection move, carrying any flow segmentIndex.
    // pointerType is recorded for every endpoint drag so the touch-is-always-
    // straight link rule (connector()/deriveDraggedLinkArc) has the real value.
    let nextInteraction: InteractionState;
    if (draggingArrowEndpoint || draggingSourceEndpoint) {
      nextInteraction = {
        mode: 'movingEndpoint',
        endpoint: draggingSourceEndpoint ? 'source' : 'arrow',
        pointerType: e.pointerType,
      };
    } else if (isEditingName) {
      nextInteraction = { mode: 'editingName', onPointerUp: false, creatingFlow: false };
    } else {
      nextInteraction = {
        mode: 'movingSelection',
        deferredSingleSelectUid: undefined,
        deferredIsText: false,
        segmentIndex,
      };
    }

    setInteraction(nextInteraction);
    setEditingName(nextEditingName);
    setInCreation(inCreationLocal);
    setMoveDelta({ x: 0, y: 0 });

    if (selectedTool === 'link' || selectedTool === 'flow') {
      latest.current.props.onSetSelection(new Set([element.uid]));
    }
  };

  const handleEditingNameChange = (value: Descendant[]): void => {
    setEditingName(value);
  };

  const handleEditingNameDone = (isCancel: boolean): void => {
    const interactionNow = latest.current.interaction;
    // Old guard was `if (!this.state.isEditingName) return` -- the editor must be
    // SHOWING NOW. The staging variant (`onPointerUp: true`, set during a
    // creation drag before the editor mounts) must NOT run this, so exclude it
    // here too (mirrors the isShowingNameEditor helper while narrowing the union).
    if (interactionNow.mode !== 'editingName' || interactionNow.onPointerUp) {
      return;
    }

    if (isCancel) {
      // Cancelling the initial name edit of a just-created flow deletes the
      // flow; creatingFlow (formerly flowStillBeingCreated) is reset by
      // clearPointerState's `interaction: idle` below, so a later rename-cancel
      // can't re-trigger this.
      if (interactionNow.creatingFlow) {
        latest.current.props.onDeleteSelection();
      }
      clearPointerState();
      return;
    }

    const uid = only(latest.current.props.selection);
    const element = getElementByUid(uid);
    const oldName = displayName(defined((element as NamedViewElement).name));
    const newName = plainSerialize(defined(latest.current.editingName));

    if (uid === inCreationUid) {
      latest.current.props.onCreateVariable({ ...element, name: newName } as ViewElement);
    } else {
      latest.current.props.onRenameVariable(oldName, newName);
    }

    clearPointerState();
  };

  // ---- Element-rendering helpers (read r.derived; never mutate caches) -----

  const alias = (element: AliasViewElement): React.ReactElement => {
    const aliasOf = r.elements.get(element.aliasOfUid) as NamedViewElement | undefined;
    let series;
    let validTarget: boolean | undefined;
    if (aliasOf) {
      series = props.model.variables.get(defined(aliasOf.ident))?.data;
      validTarget = isValidTarget(aliasOf);
    }
    const selected = isSelected(element);
    const aliasProps: AliasProps = {
      isSelected: selected,
      isValidTarget: validTarget,
      series,
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      element,
      aliasOf,
    };
    return <Alias key={element.uid} {...aliasProps} />;
  };

  const cloud = (element: CloudViewElement): React.ReactElement | undefined => {
    const selected = isSelected(element);

    // TODO: fix this -- we apparently can get in the state where a flow doesn't exist but we haven't deleted the cloud
    let flow: FlowViewElement;
    try {
      flow = getElementByUid(defined(element.flowUid)) as FlowViewElement;
    } catch {
      return;
    }

    // When dragging a cloud to attach to a stock, we need to visually hide it
    // but keep it in the DOM to maintain pointer capture.
    let isHidden = false;
    if (isSelected(flow)) {
      try {
        if (isDraggingArrowhead(interaction) && isCloudOnSinkSide(element, flow)) {
          isHidden = true;
        } else if (isDraggingSource(interaction) && isCloudOnSourceSide(element, flow)) {
          isHidden = true;
        }
      } catch (e) {
        console.error('Invalid flow state when checking cloud position:', e);
      }
    }

    const cloudProps: CloudProps = {
      element,
      isSelected: selected,
      isHidden,
      onSelection: handleSetSelection,
    };

    return <Cloud key={element.uid} {...cloudProps} />;
  };

  const aux = (element: AuxViewElement): React.ReactElement => {
    const variable = props.model.variables.get(element.ident);
    const hasWarning = variable ? variableHasError(variable) : false;
    const selected = isSelected(element);
    const series = variable?.data;
    const auxProps: AuxProps = {
      element,
      series,
      isSelected: selected,
      isEditingName: selected && isShowingNameEditor(interaction),
      isValidTarget: isValidTarget(element),
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      hasWarning,
    };

    return <Aux key={element.uid} {...auxProps} />;
  };

  const stock = (element: StockViewElement): React.ReactElement => {
    const variable = props.model.variables.get(element.ident);
    const hasWarning = variable ? variableHasError(variable) : false;
    const selected = isSelected(element);
    const series = variable?.data;
    const stockProps: StockProps = {
      element,
      series,
      isSelected: selected,
      isEditingName: selected && isShowingNameEditor(interaction),
      isValidTarget: isValidTarget(element),
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      hasWarning,
    };

    return <Stock key={element.uid} {...stockProps} />;
  };

  const module = (element: ModuleViewElement): React.ReactElement => {
    const variable = props.model.variables.get(element.ident);
    const hasEngineError = variable ? variableHasError(variable) : false;
    // AC1.6: suppress warning when no module in the model has a model reference
    // yet (new model scenario where user is rapidly sketching structure).
    const hasWarning = hasEngineError && r.derived.hasAnyModuleReference;
    const selected = isSelected(element);
    const moduleProps: ModuleProps = {
      element,
      isSelected: selected,
      isEditingName: selected && isShowingNameEditor(interaction),
      isValidTarget: isValidTarget(element),
      onSelection: handleSetSelection,
      onLabelDrag: handleLabelDrag,
      onDoubleClick: handleModuleDoubleClick,
      hasWarning,
    };

    return <Module key={element.uid} {...moduleProps} />;
  };

  const group = (element: GroupViewElement): React.ReactElement => {
    const selected = isSelected(element);
    const groupProps: GroupProps = {
      element,
      isSelected: selected,
    };

    return <Group key={element.uid} {...groupProps} />;
  };

  const connector = (element: LinkViewElement): React.ReactElement => {
    const draggingArrowhead = isDraggingArrowhead(interaction);
    const selected = props.selection.has(element.uid);

    // Get the updated element from selectionUpdates if available (arc was already adjusted
    // by applyGroupMovement for group selection cases)
    const updatedElement = r.derived.selectionUpdates.get(element.uid);
    if (updatedElement !== undefined && updatedElement.type === 'link') {
      element = updatedElement;
    }

    const from = r.derived.selectionUpdates.get(element.fromUid) || getElementByUid(element.fromUid);
    let to = r.derived.selectionUpdates.get(element.toUid) || getElementByUid(element.toUid);
    let isSticky = false;

    // Dragging this link's arrowhead — covers both new-link creation and
    // reattaching an existing link.  Unified: straight line when not over
    // a target, dynamic arc when snapped to a valid target. The arc itself is
    // computed once in deriveRenderState (derived.draggedLinkArc); we only
    // resolve the visual `to` endpoint here. Reading the derived arc (instead
    // of recomputing-and-caching it during render) keeps render free of
    // mid-render cache mutation while preserving the guarantee that the rendered
    // arc equals the value persisted on pointer-up.
    const isDraggingLink = draggingArrowhead && selected;
    if (isDraggingLink && r.selectionCenterOffset) {
      const validTarget = findLinkDragTarget();
      if (validTarget) {
        isSticky = true;
        to = validTarget;
      } else {
        const off = r.selectionCenterOffset;
        const delta = moveDelta ?? { x: 0, y: 0 };
        const canvasOffset = getCanvasOffset();
        to = {
          ...(to as AuxViewElement),
          x: off.x - delta.x - canvasOffset.x,
          y: off.y - delta.y - canvasOffset.y,
          isZeroRadius: true,
        };
      }

      const isTouch = getDragPointerType(interaction) === 'touch';
      if (isSticky && !isTouch) {
        element = { ...element, arc: r.derived.draggedLinkArc };
      } else {
        element = { ...element, arc: undefined };
      }
    }

    const connectorProps: ConnectorProps = {
      element,
      from,
      to,
      isSelected: selected,
      isDashed: to.type === 'stock',
      onSelection: handleEditConnector,
    };
    // When not dragging: pass arcPoint for existing arc-adjustment interactions
    // (e.g. clicking the arc mid-line to curve it). During link dragging the arc
    // is already computed on the element, so arcPoint would interfere.
    if (selected && !isSticky && !isDraggingLink) {
      connectorProps.arcPoint = getArcPoint();
    }
    return <Connector key={element.uid} {...connectorProps} />;
  };

  const flow = (element: FlowViewElement): React.ReactElement | undefined => {
    const variable = props.model.variables.get(element.ident);
    const hasWarning = variable ? variableHasError(variable) : false;
    const draggingArrowhead = isDraggingArrowhead(interaction);
    const selected = isSelected(element);
    const series = variable?.data;

    if (element.points.length < 2) {
      return;
    }

    const sourceId = first(element.points).attachedToUid;
    if (!sourceId) {
      return;
    }
    const source = getElementByUid(sourceId);
    if (source.type !== 'stock' && source.type !== 'cloud') {
      throw new Error('invariant broken');
    }

    const sinkId = last(element.points).attachedToUid;
    if (!sinkId) {
      return;
    }
    const sink = getElementByUid(sinkId);
    if (sink.type !== 'stock' && sink.type !== 'cloud') {
      throw new Error('invariant broken');
    }

    return (
      <Flow
        key={element.uid}
        element={element}
        series={series}
        source={source}
        sink={sink}
        embedded={props.embedded}
        isSelected={selected}
        hasWarning={hasWarning}
        isMovingArrow={selected && draggingArrowhead}
        isMovingSource={selected && isDraggingSource(interaction)}
        isEditingName={selected && isShowingNameEditor(interaction)}
        isValidTarget={isValidTarget(element)}
        onSelection={handleSetSelection}
        onLabelDrag={handleLabelDrag}
      />
    );
  };

  const buildLayers = (displayElements: readonly ViewElement[]): React.ReactElement[][] => {
    const selectionUpdates = r.derived.selectionUpdates;

    // create different layers for each of the display types so that views compose together nicely
    const zLayers = new Array(ZMax) as React.ReactElement[][];
    for (let i = 0; i < ZMax; i++) {
      zLayers[i] = [];
    }

    for (let element of displayElements) {
      if (selectionUpdates.has(element.uid)) {
        element = getOrThrow(selectionUpdates, element.uid);
      }

      // const ZOrder = Map<'flow' | 'module' | 'stock' | 'aux' | 'link' | 'style' | 'reference' | 'cloud' | 'alias', number>([
      //   ['style', 0],
      //   ['module', 1],
      //   ['link', 2],
      //   ['flow', 3],
      //   ['cloud', 4],
      //   ['stock', 4],
      //   ['aux', 5],
      //   ['reference', 5],
      //   ['alias', 5],
      // ]);

      let zOrder = 0;
      let component: React.ReactElement | undefined;
      if (element.type === 'aux') {
        component = aux(element);
        zOrder = 5;
      } else if (element.type === 'link') {
        component = connector(element);
        zOrder = 2;
      } else if (element.type === 'stock') {
        component = stock(element);
        zOrder = 4;
      } else if (element.type === 'flow') {
        component = flow(element);
        zOrder = 3;
      } else if (element.type === 'cloud') {
        component = cloud(element);
        zOrder = 4;
      } else if (element.type === 'alias') {
        component = alias(element);
        zOrder = 5;
      } else if (element.type === 'module') {
        component = module(element);
        zOrder = 4;
      } else if (element.type === 'group') {
        component = group(element);
        zOrder = 0; // Groups render behind everything else
      }

      if (!component) {
        continue;
      }

      zLayers[zOrder].push(component);
    }

    return zLayers;
  };

  // ---- Mount / unmount effect ---------------------------------------------
  // componentDidMount -> mount effect; componentWillUnmount -> the cleanup.
  // Runs once (empty deps); reads the latest props/state through `latest`.
  // Cleanup is symmetric so a StrictMode mount/unmount/mount cycle is safe.
  React.useEffect(() => {
    const derived = deriveRenderState();

    // Compute initial diagram bounds via the explicit pure pass (no longer a
    // side effect of rendering each element).
    const elementBounds = computeElementBounds(derived.displayElements, derived.selectionUpdates);

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

    const svgWidth = svgElement.clientWidth;
    const svgHeight = svgElement.clientHeight;

    const viewBox = latest.current.props.view.viewBox;
    let zoom = latest.current.props.view.zoom;

    let shouldUpdate = false;
    const prevBounds = viewBox;
    if (viewBox.width === 0 || viewBox.height === 0) {
      shouldUpdate = true;
    } else if (
      viewBox.width !== svgWidth ||
      viewBox.height !== svgHeight ||
      !isFinite(viewBox.x) ||
      !isFinite(viewBox.y) ||
      !isFinite(zoom) ||
      zoom < 0.2
    ) {
      shouldUpdate = true;
    }

    if (shouldUpdate) {
      let x = 0;
      let y = 0;

      if (!isFinite(zoom) || zoom < 0.2) {
        zoom = 1;
      }

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
      const teardownSvg = svgRef.current?.querySelector('svg');
      if (teardownSvg) {
        teardownSvg.removeEventListener('wheel', handleNativeWheel);
        teardownSvg.removeEventListener('gesturestart', handleGestureStart);
        teardownSvg.removeEventListener('gesturechange', handleGestureChange);
        teardownSvg.removeEventListener('gestureend', handleGestureEnd);
      }
      // Cancel any running momentum animation and clear all momentum state
      stopMomentumAnimation();
      // Clear velocity tracking and pointer data
      r.velocityTracker.positions = [];
      r.activePointers.clear();
      // Clear single-pointer gesture state
      r.pointerId = undefined;
      r.mouseDownPoint = undefined;
      r.selectionCenterOffset = undefined;
    };
    // Intentionally empty deps: this effect mirrors componentDidMount/Unmount.
    // All props/state it reads go through `latest`, and the native listeners /
    // observer / momentum callbacks likewise read `latest`, so nothing here
    // closes over stale values. (The repo lint config does not enable
    // react-hooks/exhaustive-deps, so no disable directive is needed.)
  }, []);

  // ---- Render -------------------------------------------------------------

  const { selectedTool, embedded } = props;

  let isEditingNameNow = isShowingNameEditor(interaction);
  if (isEditingNameNow && selectedTool !== r.prevSelectedTool) {
    // The deferred editing-done fires after this render commits; route it
    // through `latest` so it observes the freshest interaction/selection state.
    setTimeout(() => {
      handleEditingNameDone(false);
    });
    isEditingNameNow = false;
  }
  r.prevSelectedTool = selectedTool;

  // phase 1: the single render derivation. Produces displayElements, the uid
  // lookup, selection updates, module-warning flag, and the dragged-link arc.
  // This is the only place render mutates the instance caches (r.elements,
  // r.cachedVersion, r.derived) -- the same writes the class did to this.*,
  // with identical semantics, kept idempotent so a StrictMode double-render is
  // safe (the version cache short-circuits the second pass).
  const derived = deriveRenderState();
  const displayElements = derived.displayElements;

  // phase 2: create React components and add them to the appropriate layer
  const zLayers = buildLayers(displayElements);

  let overlayClass = styles.overlay;
  let nameEditor;

  let dragRect;
  if (interaction.mode === 'dragSelecting' && r.mouseDownPoint && dragSelectionPoint) {
    const pointA = r.mouseDownPoint;
    const pointB = dragSelectionPoint;
    const offset = getCanvasOffset();

    const x = Math.min(pointA.x, pointB.x) - offset.x;
    const y = Math.min(pointA.y, pointB.y) - offset.y;
    const w = Math.abs(pointA.x - pointB.x);
    const h = Math.abs(pointA.y - pointB.y);

    dragRect = <rect className={styles.dragRectOverlay} x={x} y={y} width={w} height={h} />;
  }

  if (!isEditingNameNow || props.selection.size === 0) {
    overlayClass += ' ' + styles.noPointerEvents;
  } else {
    const zoom = props.view.zoom;
    const editingUid = only(props.selection);
    const editingElement = getElementByUid(editingUid) as NamedViewElement;
    const { rw, rh } = labelRadii(editingElement.type);
    const side = editingElement.labelSide;
    const offset = getCanvasOffset();
    nameEditor = (
      <EditableLabel
        uid={editingUid}
        cx={(editingElement.x + offset.x) * zoom}
        cy={(editingElement.y + offset.y) * zoom}
        side={side}
        rw={rw * zoom}
        rh={rh * zoom}
        zoom={zoom}
        value={defined(editingName)}
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
    const bounds = calcViewBox(computeElementBounds(displayElements, derived.selectionUpdates));
    if (bounds) {
      const left = Math.floor(bounds.left) - 10;
      const top = Math.floor(bounds.top) - 10;
      const width = Math.ceil(bounds.right - left) + 10;
      const height = Math.ceil(bounds.bottom - top) + 10;
      viewBox = `${left} ${top} ${width} ${height}`;
    }
  } else {
    const zoom = props.view.zoom >= 0.2 ? props.view.zoom : 1;
    const offset = getCanvasOffset();

    transform = `matrix(${zoom} 0 0 ${zoom} ${offset.x * zoom} ${offset.y * zoom})`;
  }

  const overlay = embedded ? undefined : (
    <div className={overlayClass} onPointerDown={handleEditingEnd}>
      {nameEditor}
    </div>
  );

  // n.b. r.elements (and r.derived) are intentionally NOT cleared here:
  // event handlers read them after render returns (getElementByUid and the
  // pointer callbacks resolve connector ends / persist the dragged-link arc).

  return (
    <div style={{ height: '100%', width: '100%' }} ref={svgRef} className={`${styles.canvas} simlin-canvas`}>
      <svg
        viewBox={viewBox}
        preserveAspectRatio="xMinYMin"
        className={clsx(styles.canvas, styles.simlinCanvas, 'simlin-canvas')}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerCancel={handlePointerCancel}
        onPointerUp={handlePointerCancel}
      >
        <defs>
          <filter id="labelBackground" x="-50%" y="-50%" width="200%" height="200%">
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
        </defs>
        <g transform={transform}>
          {zLayers}
          {dragRect}
        </g>
      </svg>
      {overlay}
    </div>
  );
});
