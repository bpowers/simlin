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

// Momentum scrolling physics for macOS-native feel.
// macOS apps (Finder, Safari, Maps) have snappier deceleration than iOS.
// A friction coefficient of 0.05 means velocity retains 5% after 1 second,
// giving a ~0.5-0.8 second coast for typical pan gestures.
const FRICTION_COEFFICIENT = 0.05;
const FRICTION_LOG = Math.log(FRICTION_COEFFICIENT); // ≈ -3.0

// Stop momentum when velocity drops below this threshold.
// At 60fps, 15 px/s = 0.25 px/frame - imperceptible motion.
// Lower values make the stop feel more gradual and natural.
const VELOCITY_THRESHOLD = 15;

// Pinch-to-zoom uses exponential scaling for natural feel.
// A divisor of 100 means cumulative deltaY of ~100 results in 2x zoom.
// This matches native macOS apps like Maps and Preview.
const PINCH_ZOOM_DIVISOR = 100;

// MIN_ZOOM matches the 0.2 floor used in render() to avoid mismatch between
// view state and actual rendering (which clamps zoom < 0.2 to 1.0)
const MIN_ZOOM = 0.2;
const MAX_ZOOM = 5.0;

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
// once at the top of render()/componentDidMount(); the element-rendering
// methods (connector(), aux(), ...) only *read* these, never recompute or
// mutate during render. This keeps render() free of mid-render instance-field
// mutation.
interface RenderDerivation {
  // The elements to draw (props.view.elements plus any in-creation element).
  displayElements: readonly ViewElement[];
  // UID -> element lookup over displayElements plus the faux drag targets.
  // Reused at event-time (getElementByUid, handlers) -- see this.elements.
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

interface CanvasState {
  // The discrete interaction mode (idle | panning | dragSelecting |
  // movingSelection | movingEndpoint | movingLabel | editingName | pinching),
  // modeled as a tagged union in canvas-interaction.ts. Replaces the former bag
  // of mutually-exclusive booleans (isMovingCanvas/isDragSelecting/...) and the
  // loose pinch/label/deferred fields. The continuous companions below
  // (moveDelta, dragSelectionPoint, movingCanvasOffset, the inCreation concrete
  // elements, and the Slate editingName value) stay outside the union because
  // they are per-frame physics / DOM-adjacent state, not discrete mode.
  interaction: InteractionState;
  editingName: Array<Descendant>;
  dragSelectionPoint: Point | undefined;
  moveDelta: Point | undefined;
  movingCanvasOffset: Point | undefined;
  initialBounds: ViewRect;
  svgSize: Readonly<{ width: number; height: number }> | undefined;
  inCreation: ViewElement | undefined;
  inCreationCloud: CloudViewElement | undefined;
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

export class Canvas extends React.PureComponent<CanvasProps, CanvasState> {
  state: CanvasState;

  readonly svgRef: React.RefObject<HTMLDivElement | null>;

  // XXX: these should all be private, but that doesn't work with styled
  svgObserver: ResizeObserver | undefined;
  mouseDownPoint: Point | undefined;
  selectionCenterOffset: Point | undefined;
  pointerId: number | undefined;
  prevSelectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;

  // Cache key for the elements-by-uid lookup map: when props.version is
  // unchanged we reuse the existing map (and the displayElements array) rather
  // than rebuilding it. Owned exclusively by deriveRenderState().
  cachedVersion = -Infinity;

  // UID -> element lookup, populated by deriveRenderState() and intentionally
  // NOT cleared at the end of render: event handlers (getElementByUid and the
  // pointer callbacks) read it after render returns. Mirrors derived.elementsByUid.
  elements = new Map<UID, ViewElement>();

  // The most recent render derivation. Written only by deriveRenderState();
  // read by the element-rendering methods during render and by the pointer
  // handlers at event time. Seeded with an empty derivation so reads before
  // the first render are safe.
  derived: RenderDerivation = {
    displayElements: [],
    elementsByUid: this.elements,
    selectionUpdates: new Map<UID, ViewElement>(),
    hasAnyModuleReference: false,
    draggedLinkArc: undefined,
  };

  // Multi-touch tracking for pinch gestures
  activePointers = new Map<number, TrackedPointer>();

  // Momentum/inertia animation
  velocityTracker: VelocityTracker = { positions: [] };
  momentumAnimationId: number | undefined;
  momentumStartTime: number | undefined;
  momentumInitialVelocity: Point | undefined;
  momentumStartOffset: Point | undefined;

  constructor(props: CanvasProps) {
    super(props);

    this.svgRef = React.createRef();

    this.state = {
      interaction: idleState,
      editingName: [],
      dragSelectionPoint: undefined,
      moveDelta: undefined,
      movingCanvasOffset: undefined,
      initialBounds: viewRectDefault(),
      svgSize: undefined,
      inCreation: undefined,
      inCreationCloud: undefined,
    };
  }

  // ---- Discrete-interaction-mode accessors --------------------------------
  // The migration (#65) collapsed the former boolean CanvasState modes onto the
  // tagged-union state.interaction. These narrow accessors keep the call sites
  // readable; they are the ONLY places that destructure the union mode, so the
  // render/handler code below stays mode-agnostic. (Deliberately NOT named
  // after the deleted booleans so the modes are read through one vocabulary.)

  // Dragging a link/flow arrowhead (sink) endpoint.
  private get draggingArrowhead(): boolean {
    const i = this.state.interaction;
    return i.mode === 'movingEndpoint' && i.endpoint === 'arrow';
  }

  // Dragging a flow source endpoint.
  private get draggingSource(): boolean {
    const i = this.state.interaction;
    return i.mode === 'movingEndpoint' && i.endpoint === 'source';
  }

  // Dragging an element's label.
  private get draggingLabel(): boolean {
    return this.state.interaction.mode === 'movingLabel';
  }

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
  // directly (the old `editNameOnPointerUp`), not this accessor.
  private get showingNameEditor(): boolean {
    const i = this.state.interaction;
    return i.mode === 'editingName' && !i.onPointerUp;
  }

  // The pointer type captured at the start of an endpoint drag, or undefined
  // when not dragging an endpoint. Drives the touch-is-always-straight link
  // rule (touch links never get an arc).
  private get dragPointerType(): string | undefined {
    const i = this.state.interaction;
    return i.mode === 'movingEndpoint' ? i.pointerType : undefined;
  }

  // The flow segment being dragged (undefined = valve / whole element).
  private get draggingSegmentIndex(): number | undefined {
    const i = this.state.interaction;
    return i.mode === 'movingSelection' ? i.segmentIndex : undefined;
  }

  // The active label-drag side, or undefined when not dragging a label.
  private get labelSide(): 'right' | 'bottom' | 'left' | 'top' | undefined {
    const i = this.state.interaction;
    return i.mode === 'movingLabel' ? i.side : undefined;
  }

  // Execute the discrete effects a reducer transition emitted, in order. The
  // reducer only ever emits `capturePointer` today (selection/tool changes are
  // done by the shell directly), so this is the lone arm.
  private runEffects(effects: readonly InteractionEffect[], target: Element | undefined, pointerId: number): void {
    for (const effect of effects) {
      switch (effect.kind) {
        case 'capturePointer':
          target?.setPointerCapture(pointerId);
          break;
      }
    }
  }

  getCanvasOffset(): Readonly<Point> {
    return this.state.movingCanvasOffset ?? this.props.view.viewBox;
  }

  getElementByUid(uid: UID): ViewElement {
    let element: ViewElement | undefined;
    if (uid === inCreationUid) {
      element = this.state.inCreation;
    } else if (uid === inCreationCloudUid) {
      element = this.state.inCreationCloud;
    } else {
      element = this.elements.get(uid);
    }
    return defined(element);
  }

  // for resolving connector ends
  static buildSelectionMap(
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

  isSelected(element: ViewElement): boolean {
    return this.props.selection.has(element.uid);
  }

  alias(element: AliasViewElement): React.ReactElement {
    const aliasOf = this.elements.get(element.aliasOfUid) as NamedViewElement | undefined;
    let series;
    let isValidTarget: boolean | undefined;
    if (aliasOf) {
      series = this.props.model.variables.get(defined(aliasOf.ident))?.data;
      isValidTarget = this.isValidTarget(aliasOf);
    }
    const isSelected = this.isSelected(element);
    const props: AliasProps = {
      isSelected,
      isValidTarget,
      series,
      onSelection: this.handleSetSelection,
      onLabelDrag: this.handleLabelDrag,
      element,
      aliasOf,
    };
    return <Alias key={element.uid} {...props} />;
  }

  cloud(element: CloudViewElement): React.ReactElement | undefined {
    const isSelected = this.isSelected(element);

    // TODO: fix this -- we apparently can get in the state where a flow doesn't exist but we haven't deleted the cloud
    let flow: FlowViewElement;
    try {
      flow = this.getElementByUid(defined(element.flowUid)) as FlowViewElement;
    } catch {
      return;
    }

    // When dragging a cloud to attach to a stock, we need to visually hide it
    // but keep it in the DOM to maintain pointer capture.
    let isHidden = false;
    if (this.isSelected(flow)) {
      try {
        if (this.draggingArrowhead && isCloudOnSinkSide(element, flow)) {
          isHidden = true;
        } else if (this.draggingSource && isCloudOnSourceSide(element, flow)) {
          isHidden = true;
        }
      } catch (e) {
        console.error('Invalid flow state when checking cloud position:', e);
      }
    }

    const props: CloudProps = {
      element,
      isSelected,
      isHidden,
      onSelection: this.handleSetSelection,
    };

    return <Cloud key={element.uid} {...props} />;
  }

  isValidTarget(element: ViewElement): boolean | undefined {
    const draggingArrowhead = this.draggingArrowhead;
    const draggingSource = this.draggingSource;

    if ((!draggingArrowhead && !draggingSource) || !this.selectionCenterOffset) {
      return undefined;
    }

    const arrowUid = only(this.props.selection);
    const arrow = this.getElementByUid(arrowUid);

    const off = this.selectionCenterOffset;
    const delta = this.state.moveDelta || { x: 0, y: 0 };
    const canvasOffset = this.getCanvasOffset();
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
      const { view } = this.props;
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
  }

  aux(element: AuxViewElement): React.ReactElement {
    const variable = this.props.model.variables.get(element.ident);
    const hasWarning = variable ? variableHasError(variable) : false;
    const isSelected = this.isSelected(element);
    const series = variable?.data;
    const props: AuxProps = {
      element,
      series,
      isSelected,
      isEditingName: isSelected && this.showingNameEditor,
      isValidTarget: this.isValidTarget(element),
      onSelection: this.handleSetSelection,
      onLabelDrag: this.handleLabelDrag,
      hasWarning,
    };

    return <Aux key={element.uid} {...props} />;
  }

  stock(element: StockViewElement): React.ReactElement {
    const variable = this.props.model.variables.get(element.ident);
    const hasWarning = variable ? variableHasError(variable) : false;
    const isSelected = this.isSelected(element);
    const series = variable?.data;
    const props: StockProps = {
      element,
      series,
      isSelected,
      isEditingName: isSelected && this.showingNameEditor,
      isValidTarget: this.isValidTarget(element),
      onSelection: this.handleSetSelection,
      onLabelDrag: this.handleLabelDrag,
      hasWarning,
    };

    return <Stock key={element.uid} {...props} />;
  }

  module(element: ModuleViewElement) {
    const variable = this.props.model.variables.get(element.ident);
    const hasEngineError = variable ? variableHasError(variable) : false;
    // AC1.6: suppress warning when no module in the model has a model reference
    // yet (new model scenario where user is rapidly sketching structure).
    const hasWarning = hasEngineError && this.derived.hasAnyModuleReference;
    const isSelected = this.isSelected(element);
    const props: ModuleProps = {
      element,
      isSelected,
      isEditingName: isSelected && this.showingNameEditor,
      isValidTarget: this.isValidTarget(element),
      onSelection: this.handleSetSelection,
      onLabelDrag: this.handleLabelDrag,
      onDoubleClick: this.handleModuleDoubleClick,
      hasWarning,
    };

    return <Module key={element.uid} {...props} />;
  }

  group(element: GroupViewElement) {
    const isSelected = this.isSelected(element);
    const props: GroupProps = {
      element,
      isSelected,
    };

    return <Group key={element.uid} {...props} />;
  }

  connector(element: LinkViewElement) {
    const draggingArrowhead = this.draggingArrowhead;
    const isSelected = this.props.selection.has(element.uid);

    // Get the updated element from selectionUpdates if available (arc was already adjusted
    // by applyGroupMovement for group selection cases)
    const updatedElement = this.derived.selectionUpdates.get(element.uid);
    if (updatedElement !== undefined && updatedElement.type === 'link') {
      element = updatedElement;
    }

    const from = this.derived.selectionUpdates.get(element.fromUid) || this.getElementByUid(element.fromUid);
    let to = this.derived.selectionUpdates.get(element.toUid) || this.getElementByUid(element.toUid);
    let isSticky = false;

    // Dragging this link's arrowhead — covers both new-link creation and
    // reattaching an existing link.  Unified: straight line when not over
    // a target, dynamic arc when snapped to a valid target. The arc itself is
    // computed once in deriveRenderState (derived.draggedLinkArc); we only
    // resolve the visual `to` endpoint here. Reading the derived arc (instead
    // of recomputing-and-caching it during render) keeps render() free of
    // instance-field mutation while preserving the guarantee that the rendered
    // arc equals the value persisted on pointer-up.
    const isDraggingLink = draggingArrowhead && isSelected;
    if (isDraggingLink && this.selectionCenterOffset) {
      const validTarget = this.findLinkDragTarget();
      if (validTarget) {
        isSticky = true;
        to = validTarget;
      } else {
        const off = this.selectionCenterOffset;
        const delta = this.state.moveDelta ?? { x: 0, y: 0 };
        const canvasOffset = this.getCanvasOffset();
        to = {
          ...(to as AuxViewElement),
          x: off.x - delta.x - canvasOffset.x,
          y: off.y - delta.y - canvasOffset.y,
          isZeroRadius: true,
        };
      }

      const isTouch = this.dragPointerType === 'touch';
      if (isSticky && !isTouch) {
        element = { ...element, arc: this.derived.draggedLinkArc };
      } else {
        element = { ...element, arc: undefined };
      }
    }

    const props: ConnectorProps = {
      element,
      from,
      to,
      isSelected,
      isDashed: to.type === 'stock',
      onSelection: this.handleEditConnector,
    };
    // When not dragging: pass arcPoint for existing arc-adjustment interactions
    // (e.g. clicking the arc mid-line to curve it). During link dragging the arc
    // is already computed on the element, so arcPoint would interfere.
    if (isSelected && !isSticky && !isDraggingLink) {
      props.arcPoint = this.getArcPoint();
    }
    return <Connector key={element.uid} {...props} />;
  }

  getArcPoint(): FlowPoint | undefined {
    if (!this.selectionCenterOffset) {
      return undefined;
    }
    const off = defined(this.selectionCenterOffset);
    const delta = this.state.moveDelta ?? { x: 0, y: 0 };
    const canvasOffset = this.getCanvasOffset();
    return {
      x: off.x - delta.x - canvasOffset.x,
      y: off.y - delta.y - canvasOffset.y,
      attachedToUid: undefined,
    };
  }

  // The element the dragged single link's arrowhead is currently snapped to (a
  // valid aux/flow/module target under the cursor), or undefined for empty
  // space. A pure read over the displayed elements; shared by connector()
  // (visual `to` endpoint) and deriveDraggedLinkArc (arc computation) so both
  // agree on the snap target within a render.
  findLinkDragTarget(): ViewElement | undefined {
    return this.derived.displayElements.find((e: ViewElement) => {
      if (e.type !== 'aux' && e.type !== 'flow' && e.type !== 'module') {
        return false;
      }
      return this.isValidTarget(e) || false;
    });
  }

  // Compute the arc for a single-link arrowhead drag exactly as connector()
  // renders it: an arc only when snapped to a valid target with a mouse
  // pointer (touch links are always straight), undefined otherwise. Writes
  // nothing; called once per render from deriveRenderState so connector() and
  // the pointer-up persist path read the identical value.
  deriveDraggedLinkArc(selectionUpdates: ReadonlyMap<UID, ViewElement>): number | undefined {
    if (!this.draggingArrowhead || !this.selectionCenterOffset) {
      return undefined;
    }
    if (this.props.selection.size !== 1) {
      return undefined;
    }
    const linkUid = only(this.props.selection);
    let link = this.elements.get(linkUid);
    const updated = selectionUpdates.get(linkUid);
    if (updated !== undefined) {
      link = updated;
    }
    if (link === undefined || link.type !== 'link') {
      return undefined;
    }
    if (this.dragPointerType === 'touch') {
      return undefined;
    }
    const validTarget = this.findLinkDragTarget();
    if (!validTarget) {
      return undefined;
    }
    const from = selectionUpdates.get(link.fromUid) || this.getElementByUid(link.fromUid);
    const arcPt = this.getArcPoint();
    return arcPt ? computeLinkCreationArc(from, validTarget, arcPt) : undefined;
  }

  flow(element: FlowViewElement) {
    const variable = this.props.model.variables.get(element.ident);
    const hasWarning = variable ? variableHasError(variable) : false;
    const draggingArrowhead = this.draggingArrowhead;
    const isSelected = this.isSelected(element);
    const series = variable?.data;

    if (element.points.length < 2) {
      return;
    }

    const sourceId = first(element.points).attachedToUid;
    if (!sourceId) {
      return;
    }
    const source = this.getElementByUid(sourceId);
    if (source.type !== 'stock' && source.type !== 'cloud') {
      throw new Error('invariant broken');
    }

    const sinkId = last(element.points).attachedToUid;
    if (!sinkId) {
      return;
    }
    const sink = this.getElementByUid(sinkId);
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
        embedded={this.props.embedded}
        isSelected={isSelected}
        hasWarning={hasWarning}
        isMovingArrow={isSelected && draggingArrowhead}
        isMovingSource={isSelected && this.draggingSource}
        isEditingName={isSelected && this.showingNameEditor}
        isValidTarget={this.isValidTarget(element)}
        onSelection={this.handleSetSelection}
        onLabelDrag={this.handleLabelDrag}
      />
    );
  }

  // The single render-phase derivation step. Invoked once at the top of
  // render() and componentDidMount(); it is the ONLY method permitted to write
  // the render caches (this.elements, this.cachedVersion, this.derived). Every
  // element-rendering method below reads this.derived and never recomputes or
  // mutates a cache mid-render. Returns the produced derivation (also stored on
  // this.derived for event-time reads).
  deriveRenderState(): RenderDerivation {
    let displayElements: readonly ViewElement[] = this.props.view.elements;
    if (this.state.inCreation) {
      displayElements = [...displayElements, this.state.inCreation];
    }
    if (this.state.inCreationCloud) {
      displayElements = [...displayElements, this.state.inCreationCloud];
    }

    // Rebuild the uid lookup only when the project version changed. this.elements
    // is held across renders because event handlers read it after render returns
    // ("n.b. we don't want to clear this.elements"). The displayElements array
    // identity must track the same key, so cache both together.
    if (this.props.version !== this.cachedVersion) {
      const elements = new Map<UID, ViewElement>(displayElements.map((el) => [el.uid, el]));
      elements.set(fauxTarget.uid, fauxTarget);
      elements.set(fauxCloudTarget.uid, fauxCloudTarget);
      this.elements = elements;
      this.cachedVersion = this.props.version;
    }

    let selectionUpdates = Canvas.buildSelectionMap(this.props, this.elements, this.state.inCreation);
    const activeLabelSide = this.labelSide;
    if (activeLabelSide) {
      selectionUpdates = mapValues(selectionUpdates, (el) => {
        return { ...el, labelSide: activeLabelSide } as ViewElement;
      }) as Map<UID, ViewElement>;
    }
    if (this.state.moveDelta) {
      const moveDelta = defined(this.state.moveDelta);

      // When dragging a single link arrow (creation or reattachment),
      // suppress arcPoint so processLinks doesn't compute a rotation-based
      // arc.  connector() handles arc computation directly.
      const isDraggingLink = this.draggingArrowhead && this.props.selection.size === 1;
      const { updatedElements } = applyGroupMovement({
        elements: this.elements.values(),
        selection: this.props.selection,
        delta: moveDelta,
        arcPoint: isDraggingLink ? undefined : this.getArcPoint(),
        segmentIndex: this.draggingSegmentIndex,
      });

      selectionUpdates = new Map([...selectionUpdates, ...updatedElements]);
    }

    const derived: RenderDerivation = {
      displayElements,
      elementsByUid: this.elements,
      selectionUpdates,
      hasAnyModuleReference: anyModuleHasModelReference(this.props.model.variables),
      draggedLinkArc: undefined,
    };
    // Publish before computing the dragged-link arc: deriveDraggedLinkArc reads
    // this.derived.displayElements (via findLinkDragTarget) and selectionUpdates.
    this.derived = derived;
    derived.draggedLinkArc = this.deriveDraggedLinkArc(selectionUpdates);

    return derived;
  }

  clearPointerState(clearSelection = true): void {
    this.pointerId = undefined;
    this.mouseDownPoint = undefined;
    this.selectionCenterOffset = undefined;

    // The former loose instance fields (deferredSingleSelectUid, deferredIsText,
    // dragPointerType) now live inside the interaction union, so they are reset
    // by pointerStateReset()'s `interaction: idle`.
    this.setState(pointerStateReset());

    if (clearSelection) {
      this.props.onSetSelection(new Set());
    }

    this.focusCanvas();
  }

  handlePointerCancel = (e: React.PointerEvent<SVGElement>): void => {
    if (this.props.embedded) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    // Remove this pointer from tracking
    this.activePointers.delete(e.pointerId);

    // Handle end of pinch gesture
    if (this.state.interaction.mode === 'pinching') {
      // When exiting pinch mode, clear all gesture state for a clean restart.
      // Continuing with a single finger after pinch leads to confusing UX.
      const { state: nextInteraction } = reduceInteraction(
        this.state.interaction,
        { kind: 'pinchEnd' },
        this.interactionContext(),
      );
      this.setState({ interaction: nextInteraction });
      this.activePointers.clear();
      this.pointerId = undefined;
      this.mouseDownPoint = undefined;
      return;
    }

    if (this.pointerId === undefined || this.pointerId !== e.pointerId) {
      return;
    }

    const showDetails = shouldShowVariableDetails(
      this.selectionCenterOffset !== undefined,
      this.state.moveDelta,
      this.props.view.zoom,
      this.draggingArrowhead,
      this.draggingSource,
      this.draggingLabel,
    );

    this.pointerId = undefined;

    // Resolve deferred selection: if user clicked an already-selected element
    // without modifier, we deferred the selection change to allow group drag.
    // Now on mouseUp, if no drag occurred, collapse to the single element. The
    // deferred fields now live in the movingSelection union variant.
    const interaction = this.state.interaction;
    if (interaction.mode === 'movingSelection' && interaction.deferredSingleSelectUid !== undefined) {
      const didDrag = isDrag(this.state.moveDelta, this.props.view.zoom);
      const newSel = resolveDeferredSelection(interaction.deferredSingleSelectUid, didDrag);
      const wasDeferredText = interaction.deferredIsText;
      // Drop the deferred fields by collapsing the movingSelection variant to a
      // plain one (segmentIndex stays so a drag still moves the right segment).
      if (newSel) {
        this.props.onSetSelection(newSel);
        if (wasDeferredText && newSel.size === 1) {
          const uid = only(newSel);
          const el = this.getElementByUid(uid);
          if (!isNamedViewElement(el)) {
            // Clouds and other non-named elements can't enter text editing
            this.selectionCenterOffset = undefined;
            this.setState(pointerStateReset());
            return;
          }
          const editingName = plainDeserialize('label', displayName(defined((el as NamedViewElement).name)));
          this.setState({
            interaction: { mode: 'editingName', onPointerUp: false, creatingFlow: false },
            editingName,
            moveDelta: undefined,
          });
          this.selectionCenterOffset = undefined;
          return;
        }
      }
    }

    if (interaction.mode === 'movingLabel') {
      const selected = only(this.props.selection);
      this.props.onMoveLabel(selected, interaction.side);
      this.clearPointerState(false);
      return;
    }

    if (this.selectionCenterOffset) {
      if (this.state.moveDelta) {
        const arcPoint = this.getArcPoint();
        const delta = this.state.moveDelta;
        // The mode after committing the move: idle, unless we hand off into name
        // editing (creation tool, or a just-created flow). Computed once because
        // every boolean that used to be cleared piecemeal now lives in the union.
        let nextInteraction: InteractionState = idleState;

        if (interaction.mode === 'editingName' && interaction.onPointerUp) {
          let inCreation = this.state.inCreation;
          if (
            inCreation !== undefined &&
            (inCreation.type === 'stock' || inCreation.type === 'aux' || inCreation.type === 'module')
          ) {
            inCreation = {
              ...inCreation,
              x: inCreation.x - delta.x,
              y: inCreation.y - delta.y,
            };
          } else {
            throw new Error('invariant broken');
          }

          const editingName = plainDeserialize('label', displayName(defined((inCreation as NamedViewElement).name)));
          this.setState({
            interaction: { mode: 'editingName', onPointerUp: false, creatingFlow: false },
            editingName,
            inCreation,
            moveDelta: undefined,
          });
          this.selectionCenterOffset = undefined;
          // we do weird one off things in this codepath, so exit early
          return;
        } else if (!this.draggingArrowhead && !this.draggingSource) {
          // A sub-threshold pointer wobble during a click is not a drag: don't
          // nudge the element. shouldShowVariableDetails (which applies the
          // same threshold) will open the details panel for it instead.
          if (isDragMovement(delta, this.props.view.zoom)) {
            this.props.onMoveSelection(delta, arcPoint, this.draggingSegmentIndex);
          }
        } else {
          const element = this.getElementByUid(only(this.props.selection));
          let foundInvalidTarget = false;
          const validTarget = this.derived.displayElements.find((e: ViewElement) => {
            const isValid = this.isValidTarget(e);
            foundInvalidTarget = foundInvalidTarget || isValid === false;
            return isValid || false;
          });
          if (element.type === 'link' && validTarget) {
            // Use the arc that was last rendered — computed once per render in
            // deriveRenderState (derived.draggedLinkArc) and drawn by connector()
            // — so the saved link matches the visual exactly. Works for both
            // new-link creation and existing-link reattachment.
            const linkToAttach = { ...element, arc: this.derived.draggedLinkArc };
            this.props.onAttachLink(linkToAttach, defined(validTarget.ident));
          } else if (element.type === 'flow') {
            // don't create a flow stacked on top of 2 clouds due to a misclick
            // (a click that wobbled a pixel is still a misclick, not a drag)
            if (!isDragMovement(this.state.moveDelta, this.props.view.zoom) && this.state.inCreation) {
              this.clearPointerState();
              return;
            }
            const inCreation = !!this.state.inCreation;
            const isSourceAttach = this.draggingSource;
            let fauxTargetCenter: Point | undefined;
            if (element.points[1]?.attachedToUid === fauxCloudTargetUid) {
              const canvasOffset = this.getCanvasOffset();
              fauxTargetCenter = {
                x: this.selectionCenterOffset.x - canvasOffset.x,
                y: this.selectionCenterOffset.y - canvasOffset.y,
              };
            }
            // For source movement when not snapped to a valid target, compute the faux source center
            if (isSourceAttach && !validTarget) {
              const canvasOffset = this.getCanvasOffset();
              fauxTargetCenter = {
                x: this.selectionCenterOffset.x - canvasOffset.x,
                y: this.selectionCenterOffset.y - canvasOffset.y,
              };
            }
            this.props.onMoveFlow(
              element,
              validTarget ? validTarget.uid : 0,
              delta,
              fauxTargetCenter,
              inCreation,
              isSourceAttach,
            );
            if (inCreation) {
              // Hand off into editing the just-created flow's name. creatingFlow
              // (formerly flowStillBeingCreated) makes a later name-cancel delete
              // the flow. The editingName Slate value is carried alongside.
              nextInteraction = { mode: 'editingName', onPointerUp: false, creatingFlow: true };
              this.setState({ editingName: plainDeserialize('label', displayName(defined(element.name))) });
            }
          } else if (!foundInvalidTarget || this.state.inCreation) {
            this.props.onDeleteSelection();
          }
        }

        // Single coalesced commit: the discrete mode (idle, or the editingName
        // hand-off computed above) plus the continuous companions that travel
        // with a move. Replaces the former piecemeal isMovingArrow / isMovingSource
        // / draggingSegmentIndex clears -- those all collapse into `interaction`.
        this.setState({
          interaction: nextInteraction,
          moveDelta: undefined,
          inCreation: undefined,
          inCreationCloud: undefined,
        });
      } else if (this.draggingArrowhead || this.draggingSource) {
        // User clicked on flow arrowhead/source (or cloud) but didn't move.
        // Clear the movement mode so the cloud reappears.
        this.setState({ interaction: idleState });
      }
      this.selectionCenterOffset = undefined;
      if (showDetails) {
        this.props.onShowVariableDetails();
      }
      return;
    }

    if (interaction.mode === 'panning' && this.state.movingCanvasOffset) {
      const newViewBox = {
        ...this.props.view.viewBox,
        x: this.state.movingCanvasOffset.x,
        y: this.state.movingCanvasOffset.y,
      };

      this.props.onViewBoxChange(newViewBox, this.props.view.zoom);
      this.setState({ movingCanvasOffset: undefined });

      // Start momentum animation for smooth deceleration
      this.startMomentumAnimation();
    }

    if (!this.mouseDownPoint) {
      return;
    }

    // Handle drag selection
    if (interaction.mode === 'dragSelecting' && this.state.dragSelectionPoint) {
      const pointA = this.mouseDownPoint;
      const pointB = this.state.dragSelectionPoint;
      const canvasOffset = this.getCanvasOffset();

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
      const selectedElements = computeDragSelection(this.derived.displayElements, rect, auxCornerHit);

      // Update selection
      this.props.onSetSelection(selectedElements);
      this.clearPointerState(false);
      return;
    }

    // A pan must not clear the selection; everything reaching here does. The
    // panning branch above only cleared movingCanvasOffset, so the mode is still
    // 'panning' here (mirrors the former `!this.state.isMovingCanvas`).
    const clearSelection = interaction.mode !== 'panning';
    this.clearPointerState(clearSelection);
  };

  handleSvgResize(contentRect: { width: number; height: number }) {
    const updates = {
      svgSize: {
        width: contentRect.width,
        height: contentRect.height,
      },
    };
    const oldSize = this.state.svgSize;
    if (oldSize) {
      const dWidth = contentRect.width - oldSize.width;
      const dHeight = contentRect.height - oldSize.height;
      const canvasOffset = this.getCanvasOffset();

      const newViewBox: ViewRect = {
        x: canvasOffset.x + dWidth / 4,
        y: canvasOffset.y + dHeight / 4,
        width: contentRect.width,
        height: contentRect.height,
      };

      this.props.onViewBoxChange(newViewBox, this.props.view.zoom);
    }

    this.setState(updates);
  }

  componentWillUnmount() {
    if (this.svgObserver) {
      this.svgObserver.disconnect();
      this.svgObserver = undefined;
    }
    // Remove native event listeners for gesture prevention
    const svg = this.svgRef.current?.querySelector('svg');
    if (svg) {
      svg.removeEventListener('wheel', this.handleNativeWheel);
      svg.removeEventListener('gesturestart', this.handleGestureStart);
      svg.removeEventListener('gesturechange', this.handleGestureChange);
      svg.removeEventListener('gestureend', this.handleGestureEnd);
    }
    // Cancel any running momentum animation and clear all momentum state
    this.stopMomentumAnimation();
    // Clear velocity tracking and pointer data
    this.velocityTracker.positions = [];
    this.activePointers.clear();
    // Clear single-pointer gesture state
    this.pointerId = undefined;
    this.mouseDownPoint = undefined;
    this.selectionCenterOffset = undefined;
  }

  // Flutter-style friction simulation: calculates position at time t
  // Based on Flutter's FrictionSimulation class
  // x(t) = x0 + v0 * (friction^t - 1) / ln(friction)
  frictionPosition(velocity: number, time: number): number {
    return (velocity * (Math.pow(FRICTION_COEFFICIENT, time) - 1)) / FRICTION_LOG;
  }

  // Velocity at time t: v(t) = v0 * friction^t
  frictionVelocity(velocity: number, time: number): number {
    return velocity * Math.pow(FRICTION_COEFFICIENT, time);
  }

  // Calculate velocity from recent positions for momentum scrolling.
  // Returns zero if the pointer was stationary before release (intentional stop).
  calculateVelocity(): Point {
    const positions = this.velocityTracker.positions;
    if (positions.length < 2) {
      return { x: 0, y: 0 };
    }

    const now = window.performance.now();
    const lastPosition = positions[positions.length - 1];

    // If the pointer has been stationary for more than 40ms before release,
    // the user intentionally stopped - don't start momentum.
    // 40ms ≈ 2.5 frames at 60fps, enough to detect intentional stops
    // while still capturing quick flick-and-release gestures.
    const timeSinceLastMove = now - lastPosition.timestamp;
    if (timeSinceLastMove > 40) {
      return { x: 0, y: 0 };
    }

    // Use last 100ms of samples for velocity calculation
    const recentPositions = positions.filter((p) => now - p.timestamp < 100);

    if (recentPositions.length < 2) {
      // Fall back to last two positions
      const last = positions[positions.length - 1];
      const prev = positions[positions.length - 2];
      const dt = (last.timestamp - prev.timestamp) / 1000; // seconds
      if (dt <= 0) return { x: 0, y: 0 };
      return {
        x: (last.x - prev.x) / dt,
        y: (last.y - prev.y) / dt,
      };
    }

    // Calculate average velocity over recent samples
    const first = recentPositions[0];
    const last = recentPositions[recentPositions.length - 1];
    const dt = (last.timestamp - first.timestamp) / 1000; // seconds
    if (dt <= 0) return { x: 0, y: 0 };

    return {
      x: (last.x - first.x) / dt,
      y: (last.y - first.y) / dt,
    };
  }

  // Start momentum animation after pan release
  startMomentumAnimation = () => {
    // Cancel any existing momentum animation first (defensive)
    this.stopMomentumAnimation();

    const velocity = this.calculateVelocity();
    const speed = Math.sqrt(velocity.x * velocity.x + velocity.y * velocity.y);

    // Don't start animation if velocity is at or below threshold
    if (speed <= VELOCITY_THRESHOLD) {
      return;
    }

    this.momentumInitialVelocity = velocity;
    this.momentumStartOffset = { ...this.getCanvasOffset() };
    this.momentumStartTime = window.performance.now();

    this.momentumAnimationId = window.requestAnimationFrame(this.animateMomentum);
  };

  // Animation frame callback for momentum scrolling
  animateMomentum = (timestamp: number) => {
    if (
      this.momentumStartTime === undefined ||
      this.momentumInitialVelocity === undefined ||
      this.momentumStartOffset === undefined
    ) {
      return;
    }

    const elapsed = (timestamp - this.momentumStartTime) / 1000; // seconds
    const vx = this.momentumInitialVelocity.x;
    const vy = this.momentumInitialVelocity.y;

    // Calculate current velocity
    const currentVx = this.frictionVelocity(vx, elapsed);
    const currentVy = this.frictionVelocity(vy, elapsed);
    const currentSpeed = Math.sqrt(currentVx * currentVx + currentVy * currentVy);

    // Stop when velocity drops below threshold
    if (currentSpeed < VELOCITY_THRESHOLD) {
      this.stopMomentumAnimation();
      return;
    }

    // Calculate new position using friction simulation
    // Note: We ADD the friction position because higher offset = view moves in positive direction
    // but velocity is in screen coordinates where dragging right should move view left
    const dx = this.frictionPosition(vx, elapsed);
    const dy = this.frictionPosition(vy, elapsed);

    const newOffset = {
      x: this.momentumStartOffset.x + dx,
      y: this.momentumStartOffset.y + dy,
    };

    // Update viewBox with new offset
    const newViewBox = {
      ...this.props.view.viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };
    this.props.onViewBoxChange(newViewBox, this.props.view.zoom);

    // Continue animation
    this.momentumAnimationId = window.requestAnimationFrame(this.animateMomentum);
  };

  stopMomentumAnimation = () => {
    if (this.momentumAnimationId !== undefined) {
      window.cancelAnimationFrame(this.momentumAnimationId);
      this.momentumAnimationId = undefined;
    }
    this.momentumStartTime = undefined;
    this.momentumInitialVelocity = undefined;
    this.momentumStartOffset = undefined;
  };

  // Track position for velocity calculation during pan
  trackPosition = (x: number, y: number) => {
    const now = window.performance.now();
    this.velocityTracker.positions.push({ x, y, timestamp: now });

    // Keep only last 200ms of positions to avoid memory bloat
    // Only reallocate array if there's actually something to remove
    const cutoff = now - 200;
    const positions = this.velocityTracker.positions;
    if (positions.length > 0 && positions[0].timestamp <= cutoff) {
      this.velocityTracker.positions = positions.filter((p) => p.timestamp > cutoff);
    }
  };

  handleWheelPan = (e: WheelEvent): void => {
    const zoom = this.props.view.zoom;
    const viewBox = this.props.view.viewBox;

    // Convert wheel delta to canvas coordinates
    // deltaMode: 0 = pixels, 1 = lines, 2 = pages
    let deltaX = e.deltaX;
    let deltaY = e.deltaY;

    if (e.deltaMode === 1) {
      // Lines - multiply by line height (typically ~16-20px)
      deltaX *= 16;
      deltaY *= 16;
    } else if (e.deltaMode === 2) {
      // Pages - use actual viewport dimensions from DOM, not stored viewBox
      // which may be stale during resize transitions
      const viewportWidth = this.svgRef.current?.clientWidth ?? viewBox.width;
      const viewportHeight = this.svgRef.current?.clientHeight ?? viewBox.height;
      deltaX *= viewportWidth;
      deltaY *= viewportHeight;
    }

    // Scale delta by zoom level (inverse because higher zoom = smaller view area)
    deltaX /= zoom;
    deltaY /= zoom;

    const newViewBox = {
      ...viewBox,
      x: viewBox.x - deltaX,
      y: viewBox.y - deltaY,
    };

    this.props.onViewBoxChange(newViewBox, zoom);
  };

  // Native wheel event handler with { passive: false } to ensure preventDefault works.
  // React's synthetic onWheel handler is passive by default, so we must use native events.
  handleNativeWheel = (e: WheelEvent): void => {
    if (this.props.embedded) {
      return;
    }

    // Always prevent default to stop browser zoom, even at zoom limits
    e.preventDefault();

    // Stop any momentum animation when user starts interacting
    this.stopMomentumAnimation();

    // On Mac trackpads, pinch-to-zoom is reported as wheel events with ctrlKey
    if (e.ctrlKey || e.metaKey) {
      this.handleNativeWheelZoom(e);
    } else {
      this.handleWheelPan(e);
    }
  };

  // Native wheel zoom handler using exponential scaling for natural macOS feel.
  // Exponential scaling ensures symmetric behavior: zoom in 2x then out 2x returns to original.
  handleNativeWheelZoom = (e: WheelEvent): void => {
    const zoom = this.props.view.zoom;

    // Exponential scaling: deltaY of PINCH_ZOOM_DIVISOR results in 2x zoom change.
    // Negative deltaY = pinch out = zoom in, so we negate to get correct direction.
    const scale = Math.pow(2, -e.deltaY / PINCH_ZOOM_DIVISOR);
    let newZoom = zoom * scale;

    // Clamp zoom level
    newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, newZoom));

    // Use epsilon comparison for floating point
    if (Math.abs(newZoom - zoom) < 0.0001) {
      return;
    }

    // Get cursor position in canvas coordinates
    const cursorCanvas = this.getCanvasPoint(e.clientX, e.clientY);
    const viewBox = this.props.view.viewBox;

    // Calculate the point under cursor in model coordinates
    const modelX = cursorCanvas.x - viewBox.x;
    const modelY = cursorCanvas.y - viewBox.y;

    // Calculate new offset to keep the point under cursor stable
    const newCursorCanvas = this.getCanvasPointWithZoom(e.clientX, e.clientY, newZoom);
    const newOffset = {
      x: newCursorCanvas.x - modelX,
      y: newCursorCanvas.y - modelY,
    };

    const newViewBox = {
      ...viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };

    this.props.onViewBoxChange(newViewBox, newZoom);
  };

  // Safari-specific gesture events for pinch-to-zoom prevention.
  // Safari triggers these events alongside wheel events for trackpad pinch gestures.
  handleGestureStart = (e: Event): void => {
    if (this.props.embedded) {
      return;
    }
    e.preventDefault();
  };

  handleGestureChange = (e: Event): void => {
    if (this.props.embedded) {
      return;
    }
    e.preventDefault();
  };

  handleGestureEnd = (e: Event): void => {
    if (this.props.embedded) {
      return;
    }
    e.preventDefault();
  };

  // Helper to get canvas point with a specific zoom level
  getCanvasPointWithZoom(x: number, y: number, zoom: number): Point {
    if (this.svgRef.current) {
      const bounds = this.svgRef.current.getBoundingClientRect();
      x -= bounds.x;
      y -= bounds.y;
    }
    return screenToCanvasPoint(x, y, zoom);
  }

  // Calculate distance between two pointers for pinch gesture
  getPinchDistance(): number {
    const pointers = Array.from(this.activePointers.values());
    if (pointers.length < 2) {
      return 0;
    }
    const dx = pointers[1].x - pointers[0].x;
    const dy = pointers[1].y - pointers[0].y;
    return Math.sqrt(dx * dx + dy * dy);
  }

  // Get the center point between two pointers
  getPinchCenter(): Point {
    const pointers = Array.from(this.activePointers.values());
    if (pointers.length < 2) {
      return { x: 0, y: 0 };
    }
    return {
      x: (pointers[0].x + pointers[1].x) / 2,
      y: (pointers[0].y + pointers[1].y) / 2,
    };
  }

  handleModuleDoubleClick = (element: ModuleViewElement): void => {
    const variable = this.props.model.variables.get(element.ident);
    if (variable?.type !== 'module' || !variable.modelName) {
      return;
    }
    this.props.onDrillIntoModule(element.ident, variable.modelName);
  };

  handleLabelDrag = (uid: number, e: React.PointerEvent<SVGElement>) => {
    this.pointerId = e.pointerId;

    const selectionSet = new Set([uid]);
    if (!setsEqual(this.props.selection, selectionSet)) {
      this.props.onSetSelection(selectionSet);
    }

    const element = this.getElementByUid(uid);
    const delta = this.getCanvasOffset();
    const client = this.getCanvasPoint(e.clientX, e.clientY);
    const pointer = {
      x: client.x - delta.x,
      y: client.y - delta.y,
    };

    const side = labelSideForPointer({ x: element.x, y: element.y }, pointer);

    const { state: nextInteraction, effects } = reduceInteraction(
      this.state.interaction,
      { kind: 'labelDragStart', side },
      this.interactionContext(),
    );
    this.runEffects(effects, e.target as Element | undefined, e.pointerId);
    this.setState({ interaction: nextInteraction });
  };

  handleSelectionMove(e: React.PointerEvent<SVGElement>): void {
    if (!this.selectionCenterOffset) {
      return;
    }

    const currPt = this.getCanvasPoint(e.clientX, e.clientY);

    const dx = this.selectionCenterOffset.x - currPt.x;
    const dy = this.selectionCenterOffset.y - currPt.y;

    this.setState({
      moveDelta: {
        x: dx,
        y: dy,
      } as Point | undefined,
    });
  }

  handleMovingCanvas(e: React.PointerEvent<SVGElement>): void {
    if (!this.mouseDownPoint) {
      return;
    }

    const base = this.props.view.viewBox;
    const curr = this.getCanvasPoint(e.clientX, e.clientY);

    const newOffset = {
      x: base.x + (curr.x - this.mouseDownPoint.x),
      y: base.y + (curr.y - this.mouseDownPoint.y),
    };

    // Track position for momentum calculation
    this.trackPosition(newOffset.x, newOffset.y);

    // The panning mode was already entered on pointer-down; re-affirm it (it is
    // the move-guard in handlePointerMove) alongside the continuous offset.
    this.setState({
      interaction: { mode: 'panning' },
      movingCanvasOffset: newOffset,
    });
  }

  handleDragSelection(e: React.PointerEvent<SVGElement>): void {
    if (!this.mouseDownPoint) {
      return;
    }

    const dragSelectionPoint = this.getCanvasPoint(e.clientX, e.clientY);

    this.setState({
      interaction: { mode: 'dragSelecting' },
      dragSelectionPoint,
    });
  }

  handlePointerMove = (e: React.PointerEvent<SVGElement>): void => {
    if (this.props.embedded) {
      return;
    }

    // Update tracked pointer position
    if (this.activePointers.has(e.pointerId)) {
      this.activePointers.set(e.pointerId, {
        id: e.pointerId,
        x: e.clientX,
        y: e.clientY,
        timestamp: window.performance.now(),
      });
    }

    // Handle pinch gesture
    if (this.state.interaction.mode === 'pinching' && this.activePointers.size >= 2) {
      this.handlePinchMove();
      return;
    }

    if (this.pointerId !== e.pointerId) {
      return;
    } else if (this.pointerId && e.pointerType === 'mouse' && e.buttons === 0) {
      this.handlePointerCancel(e);
    }

    if (this.selectionCenterOffset) {
      this.handleSelectionMove(e);
    } else if (this.state.interaction.mode === 'dragSelecting') {
      this.handleDragSelection(e);
    } else if (this.state.interaction.mode === 'panning') {
      this.handleMovingCanvas(e);
    }
  };

  // Handle pinch-to-zoom gesture movement
  handlePinchMove = (): void => {
    const interaction = this.state.interaction;
    if (interaction.mode !== 'pinching') {
      return;
    }

    const currentDistance = this.getPinchDistance();
    if (currentDistance === 0 || interaction.initialDistance === 0) {
      return;
    }

    // Calculate scale factor
    const scale = currentDistance / interaction.initialDistance;
    let newZoom = interaction.initialZoom * scale;

    // Clamp zoom level
    newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, newZoom));

    // Get current pinch center in screen coordinates, then convert to canvas
    // coordinates at the NEW zoom level
    const currentCenter = this.getPinchCenter();
    const currentCenterCanvas = this.getCanvasPointWithZoom(currentCenter.x, currentCenter.y, newZoom);

    // The pinchModelPoint is fixed in model space - it's the point that was
    // under the user's fingers when the pinch started. We want that same
    // model point to remain under the current screen center.
    // newOffset = currentCenterCanvas - pinchModelPoint
    const modelPoint = interaction.modelPoint;
    const newOffset = {
      x: currentCenterCanvas.x - modelPoint.x,
      y: currentCenterCanvas.y - modelPoint.y,
    };

    const newViewBox = {
      ...this.props.view.viewBox,
      x: newOffset.x,
      y: newOffset.y,
    };

    this.props.onViewBoxChange(newViewBox, newZoom);
  };

  getNewVariableName(base: string): string {
    const variables = this.props.model.variables;
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
  }

  getCanvasPoint(x: number, y: number): Point {
    if (this.svgRef.current) {
      const bounds = this.svgRef.current.getBoundingClientRect();
      x -= bounds.x;
      y -= bounds.y;
    }
    return screenToCanvasPoint(x, y, this.props.view.zoom);
  }

  handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    if (this.props.embedded) {
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    // Stop any momentum animation when user starts interacting
    this.stopMomentumAnimation();

    // Track this pointer for multi-touch detection
    this.activePointers.set(e.pointerId, {
      id: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      timestamp: window.performance.now(),
    });

    // Check for pinch gesture (two touches)
    if (this.activePointers.size === 2 && e.pointerType === 'touch') {
      // Start pinch mode - clear all single-finger gesture state to prevent
      // simultaneous pan+pinch or drag+pinch if user adds second finger mid-gesture
      this.pointerId = undefined;
      this.mouseDownPoint = undefined;
      this.selectionCenterOffset = undefined;
      // Reset velocity tracker since pinch doesn't use momentum
      this.velocityTracker.positions = [];

      const distance = this.getPinchDistance();
      const center = this.getPinchCenter();
      const centerCanvas = this.getCanvasPoint(center.x, center.y);
      const viewBox = this.props.view.viewBox;

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
        this.state.interaction,
        {
          kind: 'pinchStart',
          initialDistance: distance,
          initialZoom: this.props.view.zoom,
          modelPoint: pinchModelPoint,
        },
        this.interactionContext(),
      );
      this.runEffects(effects, e.target as Element | undefined, e.pointerId);
      this.setState({
        interaction: nextInteraction,
        movingCanvasOffset: undefined,
      });
      return;
    }

    // If already pinching and a third finger comes in, ignore it
    if (this.state.interaction.mode === 'pinching') {
      return;
    }

    // For non-primary touches when we already have a primary, track for potential pinch
    if (!e.isPrimary && this.pointerId !== undefined) {
      return;
    }

    const client = this.getCanvasPoint(e.clientX, e.clientY);

    const canvasOffset = this.getCanvasOffset();
    const { selectedTool } = this.props;
    if (selectedTool === 'aux' || selectedTool === 'stock' || selectedTool === 'module') {
      let inCreation: AuxViewElement | StockViewElement | ModuleViewElement;
      if (selectedTool === 'aux') {
        const name = this.getNewVariableName('New Variable');
        inCreation = {
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
        const name = this.getNewVariableName('New Stock');
        inCreation = {
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
        const name = this.getNewVariableName('New Module');
        inCreation = {
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

      this.pointerId = e.pointerId;
      this.selectionCenterOffset = client;

      // The creation-tool press enters the editing-on-pointer-up handoff and
      // captures the pointer (the capturePointer effect runs setPointerCapture).
      // The staged element + zero moveDelta are the continuous companions the
      // shell owns.
      const { state: nextInteraction, effects } = reduceInteraction(
        this.state.interaction,
        { kind: 'createToolPointerDown', tool: selectedTool },
        this.interactionContext(),
      );
      this.runEffects(effects, e.target as Element | undefined, e.pointerId);
      this.setState({
        interaction: nextInteraction,
        inCreation,
        moveDelta: {
          x: 0,
          y: 0,
        },
      });
      this.props.onSetSelection(new Set([inCreation.uid]));
      return;
    }
    this.pointerId = e.pointerId;

    if (selectedTool === 'flow') {
      const canvasOffset = this.getCanvasOffset();
      const x = client.x - canvasOffset.x;
      const y = client.y - canvasOffset.y;

      const inCreationCloud: CloudViewElement = {
        type: 'cloud',
        uid: inCreationCloudUid,
        flowUid: inCreationUid,
        x,
        y,
        isZeroRadius: false,
        ident: undefined,
      };

      const name = this.getNewVariableName('New Flow');
      const inCreation: FlowViewElement = {
        type: 'flow',
        uid: inCreationUid,
        var: undefined,
        name,
        ident: canonicalize(name),
        x,
        y,
        labelSide: 'bottom',
        points: [
          { x, y, attachedToUid: inCreationCloud.uid },
          { x, y, attachedToUid: fauxCloudTarget.uid },
        ],
        isZeroRadius: false,
      };

      this.selectionCenterOffset = client;

      // Flow tool on empty canvas: enter arrowhead-drag of the staged flow so the
      // user drags the sink into place (no pointer capture in this branch, as
      // before). The staged flow + source cloud are the continuous companions.
      const { state: nextInteraction, effects } = reduceInteraction(
        this.state.interaction,
        { kind: 'flowToolPointerDown', pointerType: e.pointerType },
        this.interactionContext(),
      );
      this.runEffects(effects, e.target as Element | undefined, e.pointerId);
      this.setState({
        interaction: nextInteraction,
        inCreation,
        inCreationCloud,
        moveDelta: {
          x: 0,
          y: 0,
        },
      });
      this.props.onSetSelection(new Set([inCreation.uid]));
      return;
    }

    // onclick handlers are weird.  If we mouse down on a circle, move
    // off the circle, and mouse-up on the canvas, the canvas gets an
    // onclick.  Instead, capture where we mouse-down'd, and on mouse up
    // check if its the same.
    this.mouseDownPoint = this.getCanvasPoint(e.clientX, e.clientY);

    // Discrete decision: touch / shift-drag pans, everything else rubber-band
    // drag-selects. Routed through the pure reducer so the pan-vs-select rule
    // lives in canvas-interaction; the continuous pan offset + momentum stay in
    // the shell.
    const pan = e.pointerType === 'touch' || e.shiftKey;
    const { state: nextInteraction, effects } = reduceInteraction(
      idleState,
      { kind: 'canvasPointerDown', pan },
      this.interactionContext(),
    );
    this.runEffects(effects, e.target as Element | undefined, e.pointerId);
    if (nextInteraction.mode === 'panning') {
      // Initialize velocity tracking for momentum
      this.velocityTracker.positions = [];
      const canvasOffset = this.getCanvasOffset();
      this.trackPosition(canvasOffset.x, canvasOffset.y);
    }
    // The pan-vs-drag-select mode came from the reducer; the in-creation
    // companions are cleared regardless (an empty-canvas press stages nothing).
    this.setState({
      interaction: nextInteraction,
      inCreation: undefined,
      inCreationCloud: undefined,
    });
  };

  // The read-only environment the pure reducer needs from the shell.
  interactionContext(): InteractionContext {
    return { selection: this.props.selection };
  }

  handleEditingEnd = (e: React.PointerEvent<HTMLDivElement>): void => {
    e.preventDefault();
    e.stopPropagation();

    this.handleEditingNameDone(false);
  };

  handleEditConnector = (element: ViewElement, e: React.PointerEvent<SVGElement>, isArrowhead: boolean): void => {
    this.handleSetSelection(element, e, false, isArrowhead);
  };

  // called from handleMouseDown in elements like Aux
  handleSetSelection = (
    element: ViewElement,
    e: React.PointerEvent<SVGElement>,
    isText?: boolean,
    isArrowhead?: boolean,
    segmentIndex?: number,
    isSource?: boolean,
  ): void => {
    if (this.props.embedded) {
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
    let editingName: Array<CustomElement> = [];
    let draggingArrowEndpoint = !!isArrowhead;
    let draggingSourceEndpoint = !!isSource;

    this.pointerId = e.pointerId;

    // For multi-selection, use the click point as the offset
    // This ensures smooth dragging from where the user clicked
    this.selectionCenterOffset = this.getCanvasPoint(e.clientX, e.clientY);

    if (!isEditingName) {
      (e.target as Element).setPointerCapture(e.pointerId);
    }

    const { selectedTool } = this.props;
    let inCreation: ViewElement | undefined;

    if (selectedTool === 'link' && isNamedViewElement(element)) {
      isEditingName = false;
      draggingArrowEndpoint = true;
      inCreation = {
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
      element = inCreation;
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
      const name = this.getNewVariableName('New Flow');
      inCreation = {
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
      element = inCreation;
    } else {
      // Not a link/flow tool action -- compute selection and handle clouds
      this.props.onClearSelectedTool();

      const isMultiSelect = e.ctrlKey || e.metaKey || e.shiftKey;
      const { newSelection, deferSingleSelect } = decideMouseDownSelection(
        this.props.selection,
        element.uid,
        isMultiSelect,
      );

      if (deferSingleSelect !== undefined) {
        // Element is already in the selection and no modifier -- defer selection
        // change to mouseUp so that group drag works without dissolving selection.
        // The deferred fields ride inside the movingSelection variant now.
        this.setState({
          interaction: {
            mode: 'movingSelection',
            deferredSingleSelectUid: deferSingleSelect,
            deferredIsText: !!isText,
            segmentIndex,
          },
          editingName,
          inCreation,
          moveDelta: { x: 0, y: 0 },
        });
        return;
      }

      // Cloud re-attachment only when the cloud will be the sole selection
      const willBeSoleSelection = newSelection !== undefined && newSelection.size === 1;
      if (element.type === 'cloud' && element.flowUid !== undefined && willBeSoleSelection) {
        let flow: FlowViewElement | undefined;
        try {
          const flowElement = this.getElementByUid(element.flowUid);
          if (flowElement.type === 'flow') {
            flow = flowElement;
          }
        } catch (e) {
          console.warn(`Cloud ${element.uid} references invalid flow ${element.flowUid}:`, e);
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
        const editingElement = this.getElementByUid(uid) as NamedViewElement;
        editingName = plainDeserialize('label', displayName(defined(editingElement.name)));
      } else {
        isEditingName = false;
      }

      if (newSelection !== undefined) {
        const enteredReattachment = draggingSourceEndpoint || draggingArrowEndpoint;
        this.props.onSetSelection(resolveSelectionForReattachment(newSelection, enteredReattachment, element.uid));
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

    this.setState({
      interaction: nextInteraction,
      editingName,
      inCreation,
      moveDelta: { x: 0, y: 0 },
    });

    if (selectedTool === 'link' || selectedTool === 'flow') {
      this.props.onSetSelection(new Set([element.uid]));
    }
  };

  handleEditingNameChange = (value: Descendant[]): void => {
    this.setState({ editingName: value });
  };

  handleEditingNameDone = (isCancel: boolean) => {
    const interaction = this.state.interaction;
    // Old guard was `if (!this.state.isEditingName) return` -- the editor must be
    // SHOWING NOW. The staging variant (`onPointerUp: true`, set during a
    // creation drag before the editor mounts) must NOT run this, so exclude it
    // here too (mirrors the showingNameEditor accessor while narrowing the union).
    if (interaction.mode !== 'editingName' || interaction.onPointerUp) {
      return;
    }

    if (isCancel) {
      // Cancelling the initial name edit of a just-created flow deletes the
      // flow; creatingFlow (formerly flowStillBeingCreated) is reset by
      // clearPointerState's `interaction: idle` below, so a later rename-cancel
      // can't re-trigger this.
      if (interaction.creatingFlow) {
        this.props.onDeleteSelection();
      }
      this.clearPointerState();
      return;
    }

    const uid = only(this.props.selection);
    const element = this.getElementByUid(uid);
    const oldName = displayName(defined((element as NamedViewElement).name));
    const newName = plainSerialize(defined(this.state.editingName));

    if (uid === inCreationUid) {
      this.props.onCreateVariable({ ...element, name: newName } as ViewElement);
    } else {
      this.props.onRenameVariable(oldName, newName);
    }

    this.clearPointerState();
  };

  focusCanvas() {
    // an SVG element can't actually be focused.  Instead, blur any _other_
    // focused element.
    if (typeof document !== 'undefined' && document && document.activeElement) {
      const activeElement = document.activeElement;
      if ('blur' in activeElement && typeof activeElement.blur === 'function') {
        activeElement.blur();
      }
    }
  }

  buildLayers(displayElements: readonly ViewElement[]): React.ReactElement[][] {
    const selectionUpdates = this.derived.selectionUpdates;

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
        component = this.aux(element);
        zOrder = 5;
      } else if (element.type === 'link') {
        component = this.connector(element);
        zOrder = 2;
      } else if (element.type === 'stock') {
        component = this.stock(element);
        zOrder = 4;
      } else if (element.type === 'flow') {
        component = this.flow(element);
        zOrder = 3;
      } else if (element.type === 'cloud') {
        component = this.cloud(element);
        zOrder = 4;
      } else if (element.type === 'alias') {
        component = this.alias(element);
        zOrder = 5;
      } else if (element.type === 'module') {
        component = this.module(element);
        zOrder = 4;
      } else if (element.type === 'group') {
        component = this.group(element);
        zOrder = 0; // Groups render behind everything else
      }

      if (!component) {
        continue;
      }

      zLayers[zOrder].push(component);
    }

    return zLayers;
  }

  componentDidMount() {
    const derived = this.deriveRenderState();

    // Compute initial diagram bounds via the explicit pure pass (no longer a
    // side effect of rendering each element).
    const elementBounds = computeElementBounds(derived.displayElements, derived.selectionUpdates);

    let initialBounds: ViewRect | undefined;
    const bounds = calcViewBox(elementBounds);
    if (bounds) {
      const left = Math.floor(bounds.left) - 10;
      const top = Math.floor(bounds.top) - 10;
      const width = Math.ceil(bounds.right - left) + 10;
      const height = Math.ceil(bounds.bottom - top) + 10;
      initialBounds = { x: left, y: top, width, height };
      this.setState({ initialBounds });
    }

    const svgElement = exists(this.svgRef.current);
    this.svgObserver?.disconnect();
    this.svgObserver = new ResizeObserver((entries: ResizeObserverEntry[]) => {
      const entry = defined(entries[0]);
      const target = entry.target as HTMLDivElement;
      this.handleSvgResize({
        width: target.clientWidth,
        height: target.clientHeight,
      });
    });

    this.svgObserver.observe(svgElement);

    // Register native event listeners with { passive: false } to ensure preventDefault() works.
    // React's synthetic event handlers are passive by default for wheel events, which means
    // preventDefault() is ignored and the browser still performs its native pinch-to-zoom.
    const svg = svgElement.querySelector('svg');
    if (svg) {
      svg.addEventListener('wheel', this.handleNativeWheel, { passive: false });
      // Safari-specific gesture events for pinch-to-zoom prevention
      svg.addEventListener('gesturestart', this.handleGestureStart, { passive: false });
      svg.addEventListener('gesturechange', this.handleGestureChange, { passive: false });
      svg.addEventListener('gestureend', this.handleGestureEnd, { passive: false });
    }

    const svgWidth = svgElement.clientWidth;
    const svgHeight = svgElement.clientHeight;

    const viewBox = this.props.view.viewBox;
    let zoom = this.props.view.zoom;

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
      if (initialBounds) {
        const currWidth = svgWidth / zoom;
        const currHeight = svgHeight / zoom;

        // convert diagram bounds to cx,cy
        initialBounds = defined(initialBounds);
        const diagramCx = initialBounds.x + initialBounds.width / 2;
        const diagramCy = initialBounds.y + initialBounds.height / 2;

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

      this.props.onViewBoxChange(newViewBox, zoom);

      this.setState({
        svgSize: {
          width: svgWidth,
          height: svgHeight,
        },
      });
    }
  }

  render() {
    const { selectedTool, embedded } = this.props;

    let isEditingName = this.showingNameEditor;
    if (isEditingName && selectedTool !== this.prevSelectedTool) {
      setTimeout(() => {
        this.handleEditingNameDone(false);
      });
      isEditingName = false;
    }
    this.prevSelectedTool = selectedTool;

    // phase 1: the single render derivation. Produces displayElements, the uid
    // lookup, selection updates, module-warning flag, and the dragged-link arc.
    // This is the only place render() mutates instance caches.
    const derived = this.deriveRenderState();
    const displayElements = derived.displayElements;

    // phase 2: create React components and add them to the appropriate layer
    const zLayers = this.buildLayers(displayElements);

    let overlayClass = styles.overlay;
    let nameEditor;

    let dragRect;
    if (this.state.interaction.mode === 'dragSelecting' && this.mouseDownPoint && this.state.dragSelectionPoint) {
      const pointA = this.mouseDownPoint;
      const pointB = this.state.dragSelectionPoint;
      const offset = this.getCanvasOffset();

      const x = Math.min(pointA.x, pointB.x) - offset.x;
      const y = Math.min(pointA.y, pointB.y) - offset.y;
      const w = Math.abs(pointA.x - pointB.x);
      const h = Math.abs(pointA.y - pointB.y);

      dragRect = <rect className={styles.dragRectOverlay} x={x} y={y} width={w} height={h} />;
    }

    if (!isEditingName || this.props.selection.size === 0) {
      overlayClass += ' ' + styles.noPointerEvents;
    } else {
      const zoom = this.props.view.zoom;
      const editingUid = only(this.props.selection);
      const editingElement = this.getElementByUid(editingUid) as NamedViewElement;
      const { rw, rh } = labelRadii(editingElement.type);
      const side = editingElement.labelSide;
      const offset = this.getCanvasOffset();
      nameEditor = (
        <EditableLabel
          uid={editingUid}
          cx={(editingElement.x + offset.x) * zoom}
          cy={(editingElement.y + offset.y) * zoom}
          side={side}
          rw={rw * zoom}
          rh={rh * zoom}
          zoom={zoom}
          value={defined(this.state.editingName)}
          onChange={this.handleEditingNameChange}
          onDone={this.handleEditingNameDone}
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
      const zoom = this.props.view.zoom >= 0.2 ? this.props.view.zoom : 1;
      const offset = this.getCanvasOffset();

      transform = `matrix(${zoom} 0 0 ${zoom} ${offset.x * zoom} ${offset.y * zoom})`;
    }

    const overlay = embedded ? undefined : (
      <div className={overlayClass} onPointerDown={this.handleEditingEnd}>
        {nameEditor}
      </div>
    );

    // n.b. this.elements (and this.derived) are intentionally NOT cleared here:
    // event handlers read them after render returns (getElementByUid and the
    // pointer callbacks resolve connector ends / persist the dragged-link arc).

    return (
      <div style={{ height: '100%', width: '100%' }} ref={this.svgRef} className={`${styles.canvas} simlin-canvas`}>
        <svg
          viewBox={viewBox}
          preserveAspectRatio="xMinYMin"
          className={clsx(styles.canvas, styles.simlinCanvas, 'simlin-canvas')}
          onPointerDown={this.handlePointerDown}
          onPointerMove={this.handlePointerMove}
          onPointerCancel={this.handlePointerCancel}
          onPointerUp={this.handlePointerCancel}
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
  }
}
