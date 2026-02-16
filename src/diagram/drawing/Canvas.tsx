// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/// <reference types="resize-observer-browser" />

import * as React from 'react';

import clsx from 'clsx';
import { Descendant } from 'slate';
import { List, Map, Set } from 'immutable';

import { defined, exists } from '@simlin/core/common';
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
} from '@simlin/core/datamodel';
import { canonicalize } from '@simlin/core/canonicalize';

import { Alias, AliasProps } from './Alias';
import { Aux, auxBounds, auxContains, AuxProps } from './Auxiliary';
import { Cloud, cloudBounds, cloudContains, CloudProps } from './Cloud';
import { isCloudOnSourceSide, isCloudOnSinkSide } from './cloud-utils';
import { calcViewBox, displayName, plainDeserialize, plainSerialize, Point, Rect, screenToCanvasPoint } from './common';
import { Connector, ConnectorProps, getVisualCenter } from './Connector';
import { AuxRadius } from './default';
import { EditableLabel } from './EditableLabel';
import { Flow, flowBounds, UpdateCloudAndFlow, UpdateFlow, UpdateStockAndFlows } from './Flow';
import { applyGroupMovement } from '../group-movement';
import { Group, groupBounds, GroupProps } from './Group';
import { Module, moduleBounds, ModuleProps } from './Module';
import { CustomElement } from './SlateEditor';
import { Stock, stockBounds, stockContains, StockHeight, StockProps, StockWidth } from './Stock';
import { updateArcAngle } from '../arc-utils';
import { shouldShowVariableDetails } from './pointer-utils';
import {
  computeMouseDownSelection,
  computeMouseUpSelection,
  pointerStateReset,
  resolveSelectionForReattachment,
} from '../selection-logic';

import styles from './Canvas.module.css';

export const inCreationUid = -2;
export const fauxTargetUid = -3;
export const inCreationCloudUid = -4;
export const fauxCloudTargetUid = -5;

const fauxTarget = new AuxViewElement({
  name: '$⁚model-internal-faux-target',
  ident: '$⁚model-internal-faux-target',
  uid: fauxTargetUid,
  var: undefined,
  x: 0,
  y: 0,
  labelSide: 'right' as LabelSide,
  isZeroRadius: true,
});

const fauxCloudTarget = new CloudViewElement({
  uid: fauxCloudTargetUid,
  flowUid: -1,
  x: 0,
  y: 0,
  isZeroRadius: true,
});

function radToDeg(r: number): number {
  return (r * 180) / Math.PI;
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

interface CanvasState {
  isMovingCanvas: boolean;
  isDragSelecting: boolean;
  isEditingName: boolean;
  isMovingArrow: boolean;
  isMovingSource: boolean;
  isMovingLabel: boolean;
  labelSide: 'right' | 'bottom' | 'left' | 'top' | undefined;
  editingName: Array<Descendant>;
  editNameOnPointerUp: boolean;
  flowStillBeingCreated: boolean;
  dragSelectionPoint: Point | undefined;
  moveDelta: Point | undefined;
  movingCanvasOffset: Point | undefined;
  initialBounds: ViewRect;
  svgSize: Readonly<{ width: number; height: number }> | undefined;
  inCreation: ViewElement | undefined;
  inCreationCloud: CloudViewElement | undefined;
  // Multi-touch pinch state
  isPinching: boolean;
  initialPinchDistance: number;
  initialPinchZoom: number;
  // Store the MODEL coordinates of the point under the initial pinch center.
  // This is the fixed point that should stay under the user's fingers during zoom.
  pinchModelPoint: Point | undefined;
  // Which segment of a flow is being dragged (undefined = valve)
  draggingSegmentIndex: number | undefined;
}

export interface CanvasProps {
  embedded: boolean;
  project: Project;
  model: Model;
  view: StockFlowView;
  version: number;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;
  selection: Set<UID>;
  onRenameVariable: (oldName: string, newName: string) => void;
  onSetSelection: (selected: Set<UID>) => void;
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
}

export class Canvas extends React.PureComponent<CanvasProps, CanvasState> {
  state: CanvasState;

  readonly svgRef: React.RefObject<HTMLDivElement | null>;

  // XXX: these should all be private, but that doesn't work with styled
  svgObserver: ResizeObserver | undefined;
  mouseDownPoint: Point | undefined;
  selectionCenterOffset: Point | undefined;
  pointerId: number | undefined;
  elementBounds = List<Rect | undefined>();
  prevSelectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;
  // we have to regenerate selectionUpdates when selection !== props.selection
  selection = Set<UID>();
  cachedVersion = -Infinity;
  cachedElements = List<ViewElement>();
  elements = Map<UID, ViewElement>();
  selectionUpdates = Map<UID, ViewElement>();
  computeBounds = false;

  // Multi-touch tracking for pinch gestures
  // Note: use globalThis.Map to get native Map (not Immutable.Map from imports)
  activePointers = new globalThis.Map<number, TrackedPointer>();

  // Momentum/inertia animation
  velocityTracker: VelocityTracker = { positions: [] };
  momentumAnimationId: number | undefined;
  momentumStartTime: number | undefined;
  momentumInitialVelocity: Point | undefined;
  momentumStartOffset: Point | undefined;

  // Deferred selection state for click-in-group-then-drag behavior.
  // Set on mouseDown when clicking an already-selected element without modifier;
  // resolved on mouseUp based on whether a drag occurred.
  deferredSingleSelectUid: UID | undefined;
  deferredIsText: boolean | undefined;

  constructor(props: CanvasProps) {
    super(props);

    this.svgRef = React.createRef();

    this.state = {
      isMovingArrow: false,
      isMovingSource: false,
      isMovingCanvas: false,
      isDragSelecting: false,
      isEditingName: false,
      isMovingLabel: false,
      labelSide: undefined,
      editingName: [],
      editNameOnPointerUp: false,
      flowStillBeingCreated: false,
      dragSelectionPoint: undefined,
      moveDelta: undefined,
      movingCanvasOffset: undefined,
      initialBounds: ViewRect.default(),
      svgSize: undefined,
      inCreation: undefined,
      inCreationCloud: undefined,
      // Multi-touch pinch state
      isPinching: false,
      initialPinchDistance: 0,
      initialPinchZoom: 1,
      pinchModelPoint: undefined,
      draggingSegmentIndex: undefined,
    };
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
    elements: Map<UID, ViewElement>,
    inCreation?: ViewElement,
  ): Map<UID, ViewElement> {
    let selection = Map<UID, ViewElement>();
    for (const uid of props.selection) {
      if (uid === inCreationUid && inCreation) {
        selection = selection.set(uid, inCreation);
      } else {
        const e = getOrThrow(elements, uid);
        selection = selection.set(e.uid, e);
      }
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
        if (this.state.isMovingArrow && isCloudOnSinkSide(element, flow)) {
          isHidden = true;
        } else if (this.state.isMovingSource && isCloudOnSourceSide(element, flow)) {
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

    if (this.computeBounds) {
      this.elementBounds = this.elementBounds.push(cloudBounds(element));
    }

    return <Cloud key={element.uid} {...props} />;
  }

  isValidTarget(element: ViewElement): boolean | undefined {
    const { isMovingArrow, isMovingSource } = this.state;

    if ((!isMovingArrow && !isMovingSource) || !this.selectionCenterOffset) {
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
    if (element instanceof CloudViewElement) {
      isTarget = cloudContains(element, pointer);
    } else if (element instanceof StockViewElement) {
      isTarget = stockContains(element, pointer);
    } else if (element instanceof AuxViewElement) {
      isTarget = auxContains(element, pointer);
    } else if (element instanceof FlowViewElement) {
      isTarget = auxContains(element, pointer);
    }
    if (!isTarget) {
      return undefined;
    }

    // don't allow connectors from and to the same element
    if (arrow instanceof LinkViewElement && arrow.fromUid === element.uid) {
      return undefined;
    }

    // dont allow duplicate links between the same two elements
    if (arrow instanceof LinkViewElement) {
      const { view } = this.props;
      for (const e of view.elements) {
        // skip if its not a connector, or if it is the currently selected connector
        if (!(e instanceof LinkViewElement) || e.uid === arrow.uid) {
          continue;
        }

        if (e.fromUid === arrow.fromUid && e.toUid === element.uid) {
          return false;
        }
      }
    }

    if (arrow instanceof FlowViewElement) {
      if (!(element instanceof StockViewElement)) {
        return false;
      }

      if (isMovingSource) {
        // For source movement: check if target stock is valid source
        const lastPt = last(arrow.points);
        // Don't allow connecting source and sink to the same stock
        if (lastPt.attachedToUid === element.uid) {
          return false;
        }
        // For multi-segment flows (3+ points), the source needs to align with
        // the adjacent point (second), not the sink point. For 2-point flows,
        // points.get(1) gives us the last point, which is correct.
        const adjacentToSource = at(arrow.points, 1);
        return (
          Math.abs(adjacentToSource.x - element.cx) < StockWidth / 2 ||
          Math.abs(adjacentToSource.y - element.cy) < StockHeight / 2
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
        // flows, points.size - 2 = 0 gives us the first point, which is correct.
        const adjacentToArrowhead = at(arrow.points, arrow.points.size - 2);
        return (
          Math.abs(adjacentToArrowhead.x - element.cx) < StockWidth / 2 ||
          Math.abs(adjacentToArrowhead.y - element.cy) < StockHeight / 2
        );
      }
    }

    return element instanceof FlowViewElement || element instanceof AuxViewElement;
  }

  aux(element: AuxViewElement): React.ReactElement {
    const variable = this.props.model.variables.get(element.ident);
    const hasWarning = variable?.hasError || false;
    const isSelected = this.isSelected(element);
    const series = variable?.data;
    const props: AuxProps = {
      element,
      series,
      isSelected,
      isEditingName: isSelected && this.state.isEditingName,
      isValidTarget: this.isValidTarget(element),
      onSelection: this.handleSetSelection,
      onLabelDrag: this.handleLabelDrag,
      hasWarning,
    };

    if (this.computeBounds) {
      this.elementBounds = this.elementBounds.push(auxBounds(element));
    }

    return <Aux key={element.uid} {...props} />;
  }

  stock(element: StockViewElement): React.ReactElement {
    const variable = this.props.model.variables.get(element.ident);
    const hasWarning = variable?.hasError || false;
    const isSelected = this.isSelected(element);
    const series = variable?.data;
    const props: StockProps = {
      element,
      series,
      isSelected,
      isEditingName: isSelected && this.state.isEditingName,
      isValidTarget: this.isValidTarget(element),
      onSelection: this.handleSetSelection,
      onLabelDrag: this.handleLabelDrag,
      hasWarning,
    };

    if (this.computeBounds) {
      this.elementBounds = this.elementBounds.push(stockBounds(element));
    }
    return <Stock key={element.uid} {...props} />;
  }

  module(element: ModuleViewElement) {
    const isSelected = this.isSelected(element);
    const props: ModuleProps = {
      element,
      isSelected,
    };

    if (this.computeBounds) {
      this.elementBounds = this.elementBounds.push(moduleBounds(props));
    }
    return <Module key={element.uid} {...props} />;
  }

  group(element: GroupViewElement) {
    const isSelected = this.isSelected(element);
    const props: GroupProps = {
      element,
      isSelected,
    };

    if (this.computeBounds) {
      this.elementBounds = this.elementBounds.push(groupBounds(element));
    }
    return <Group key={element.uid} {...props} />;
  }

  connector(element: LinkViewElement) {
    const { isMovingArrow } = this.state;
    const isSelected = this.props.selection.has(element.uid);

    // Get the updated element from selectionUpdates if available (arc was already adjusted
    // by applyGroupMovement for group selection cases)
    const updatedElement = this.selectionUpdates.get(element.uid);
    if (updatedElement instanceof LinkViewElement) {
      element = updatedElement;
    }

    const from = this.selectionUpdates.get(element.fromUid) || this.getElementByUid(element.fromUid);
    let to = this.selectionUpdates.get(element.toUid) || this.getElementByUid(element.toUid);
    const toUid = to.uid;
    let isSticky = false;
    if (isMovingArrow && isSelected && this.selectionCenterOffset) {
      const validTarget = this.cachedElements.find((e: ViewElement) => {
        if (!(e instanceof AuxViewElement || e instanceof FlowViewElement)) {
          return false;
        }
        return this.isValidTarget(e) || false;
      });
      if (validTarget) {
        isSticky = true;
        to = validTarget;
      } else {
        const off = this.selectionCenterOffset;
        const delta = this.state.moveDelta ?? { x: 0, y: 0 };
        const canvasOffset = this.getCanvasOffset();
        // if to isn't a valid target, that means it is the fauxTarget
        to = (to as AuxViewElement).merge({
          x: off.x - delta.x - canvasOffset.x,
          y: off.y - delta.y - canvasOffset.y,
          isZeroRadius: true,
        }) as ViewElement;
      }
    }
    // When dragging a link arrow (isMovingArrow), adjust arc based on the dynamic to position.
    // For other movement cases, the arc is already adjusted by applyGroupMovement.
    if (isMovingArrow) {
      const oldTo = getOrThrow(this.elements, toUid);
      const oldFrom = getOrThrow(this.elements, from.uid);
      const oldToVisual = getVisualCenter(oldTo);
      const oldFromVisual = getVisualCenter(oldFrom);
      const toVisual = getVisualCenter(to);
      const fromVisual = getVisualCenter(from);

      // Endpoints moved differently - adjust arc based on rotation
      const oldθ = Math.atan2(oldToVisual.cy - oldFromVisual.cy, oldToVisual.cx - oldFromVisual.cx);
      const newθ = Math.atan2(toVisual.cy - fromVisual.cy, toVisual.cx - fromVisual.cx);
      const diffθ = oldθ - newθ;
      element = element.set('arc', updateArcAngle(element.arc, radToDeg(diffθ)));
    }
    const props: ConnectorProps = {
      element,
      from,
      to,
      isSelected,
      isDashed: to instanceof StockViewElement,
      onSelection: this.handleEditConnector,
    };
    if (isSelected && !isSticky) {
      props.arcPoint = this.getArcPoint();
    }
    // this.elementBounds = this.elementBounds.push(Connector.bounds(props));
    return <Connector key={element.uid} {...props} />;
  }

  getArcPoint(): FlowPoint | undefined {
    if (!this.selectionCenterOffset) {
      return undefined;
    }
    const off = defined(this.selectionCenterOffset);
    const delta = this.state.moveDelta ?? { x: 0, y: 0 };
    const canvasOffset = this.getCanvasOffset();
    return new FlowPoint({
      x: off.x - delta.x - canvasOffset.x,
      y: off.y - delta.y - canvasOffset.y,
      attachedToUid: undefined,
    });
  }

  flow(element: FlowViewElement) {
    const variable = this.props.model.variables.get(element.ident);
    const hasWarning = variable?.hasError || false;
    const { isMovingArrow } = this.state;
    const isSelected = this.isSelected(element);
    const series = variable?.data;

    if (element.points.size < 2) {
      return;
    }

    const sourceId = first(element.points).attachedToUid;
    if (!sourceId) {
      return;
    }
    const source = this.getElementByUid(sourceId);
    if (!(source instanceof StockViewElement || source instanceof CloudViewElement)) {
      throw new Error('invariant broken');
    }

    const sinkId = last(element.points).attachedToUid;
    if (!sinkId) {
      return;
    }
    const sink = this.getElementByUid(sinkId);
    if (!(sink instanceof StockViewElement || sink instanceof CloudViewElement)) {
      throw new Error('invariant broken');
    }

    if (this.computeBounds) {
      this.elementBounds = this.elementBounds.push(flowBounds(element));
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
        isMovingArrow={isSelected && isMovingArrow}
        isMovingSource={isSelected && this.state.isMovingSource}
        isEditingName={isSelected && this.state.isEditingName}
        isValidTarget={this.isValidTarget(element)}
        onSelection={this.handleSetSelection}
        onLabelDrag={this.handleLabelDrag}
      />
    );
  }

  constrainFlowMovement(
    flow: FlowViewElement,
    moveDelta: Point,
  ): [FlowViewElement, List<StockViewElement | CloudViewElement>] {
    const sourceId = defined(first(flow.points).attachedToUid);
    let source = this.getElementByUid(sourceId) as StockViewElement | CloudViewElement;
    if (!(source instanceof StockViewElement || source instanceof CloudViewElement)) {
      throw new Error('invariant broken');
    }

    const sinkId = defined(last(flow.points).attachedToUid);
    let sink = this.getElementByUid(sinkId) as StockViewElement | CloudViewElement;
    if (!(sink instanceof StockViewElement || sink instanceof CloudViewElement)) {
      throw new Error('invariant broken');
    }

    const { isMovingArrow, isMovingSource } = this.state;

    if (isMovingSource && this.selectionCenterOffset) {
      // Source movement: find valid target for source end
      const validTarget = this.cachedElements.find((e: ViewElement) => {
        // Don't connect to the sink stock
        if (!(e instanceof StockViewElement) || e.uid === sinkId) {
          return false;
        }
        return this.isValidTarget(e) || false;
      }) as StockViewElement;

      if (validTarget) {
        moveDelta = {
          x: source.cx - validTarget.cx,
          y: source.cy - validTarget.cy,
        };
        source = validTarget.merge({
          uid: sourceId,
          x: source.cx,
          y: source.cy,
        });
      } else {
        const off = this.selectionCenterOffset;
        const canvasOffset = this.getCanvasOffset();

        source = (source as unknown as any).merge({
          x: off.x - canvasOffset.x,
          y: off.y - canvasOffset.y,
          isZeroRadius: true,
        });
      }

      [source, flow] = UpdateCloudAndFlow(source, flow, moveDelta);
      return [flow, List([])];
    }

    if (isMovingArrow && this.selectionCenterOffset) {
      const validTarget = this.cachedElements.find((e: ViewElement) => {
        // connecting both the inflow + outflow of a stock to itself wouldn't make sense.
        if (!(e instanceof StockViewElement) || e.uid === sourceId) {
          return false;
        }
        return this.isValidTarget(e) || false;
      }) as StockViewElement;
      if (validTarget) {
        moveDelta = {
          x: sink.cx - validTarget.cx,
          y: sink.cy - validTarget.cy,
        };
        sink = validTarget.merge({
          uid: sinkId,
          x: sink.cx,
          y: sink.cy,
        });
      } else {
        const off = this.selectionCenterOffset;
        const canvasOffset = this.getCanvasOffset();

        sink = (sink as unknown as any).merge({
          x: off.x - canvasOffset.x,
          y: off.y - canvasOffset.y,
          isZeroRadius: true,
        });
      }

      [sink, flow] = UpdateCloudAndFlow(sink, flow, moveDelta);
      return [flow, List([])];
    }

    const ends = List<StockViewElement | CloudViewElement>([source, sink]);
    return UpdateFlow(flow, ends, moveDelta, this.state.draggingSegmentIndex);
  }

  constrainCloudMovement(
    cloudEl: CloudViewElement,
    moveDelta: Point,
  ): [StockViewElement | CloudViewElement, FlowViewElement] {
    const flow = this.getElementByUid(defined(cloudEl.flowUid)) as FlowViewElement;
    return UpdateCloudAndFlow(cloudEl, flow, moveDelta);
  }

  constrainStockMovement(stockEl: StockViewElement, moveDelta: Point): [StockViewElement, List<FlowViewElement>] {
    const flows = List<FlowViewElement>(
      stockEl.inflows
        .concat(stockEl.outflows)
        .map((uid) => (this.selectionUpdates.get(uid) || this.getElementByUid(uid)) as FlowViewElement | undefined)
        .filter((element) => element !== undefined)
        .map((element) => defined(element)),
    );

    return UpdateStockAndFlows(stockEl, flows, moveDelta);
  }

  renderInitAndCache(): List<ViewElement> {
    if (!this.props.selection.equals(this.selection)) {
      this.selection = this.props.selection;
    }

    let displayElements = this.props.view.elements;
    if (this.state.inCreation) {
      displayElements = displayElements.push(this.state.inCreation);
    }
    if (this.state.inCreationCloud) {
      displayElements = displayElements.push(this.state.inCreationCloud);
    }

    if (this.props.version !== this.cachedVersion) {
      this.elements = Map(displayElements.map((el) => [el.uid, el]))
        .set(fauxTarget.uid, fauxTarget)
        .set(fauxCloudTarget.uid, fauxCloudTarget);
      this.cachedElements = displayElements;
      this.cachedVersion = this.props.version;
    }

    this.selectionUpdates = Canvas.buildSelectionMap(this.props, this.elements, this.state.inCreation);
    if (this.state.labelSide) {
      this.selectionUpdates = this.selectionUpdates.map((el) => {
        return (el as AuxViewElement).set('labelSide', defined(this.state.labelSide));
      });
    }
    if (this.state.moveDelta) {
      const moveDelta = defined(this.state.moveDelta);

      const { updatedElements } = applyGroupMovement({
        elements: this.elements.values(),
        selection: this.props.selection,
        delta: moveDelta,
        arcPoint: this.getArcPoint(),
        segmentIndex: this.state.draggingSegmentIndex,
      });

      this.selectionUpdates = this.selectionUpdates.merge(updatedElements);
    }

    return displayElements;
  }

  clearPointerState(clearSelection = true): void {
    this.pointerId = undefined;
    this.mouseDownPoint = undefined;
    this.selectionCenterOffset = undefined;
    this.deferredSingleSelectUid = undefined;
    this.deferredIsText = undefined;

    this.setState(pointerStateReset());

    if (clearSelection) {
      this.props.onSetSelection(Set());
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
    if (this.state.isPinching) {
      // When exiting pinch mode, clear all gesture state for a clean restart.
      // Continuing with a single finger after pinch leads to confusing UX.
      this.setState({
        isPinching: false,
        initialPinchDistance: 0,
        initialPinchZoom: 1,
        pinchModelPoint: undefined,
      });
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
      this.state.isMovingArrow,
      this.state.isMovingSource,
      this.state.isMovingLabel,
    );

    this.pointerId = undefined;

    // Resolve deferred selection: if user clicked an already-selected element
    // without modifier, we deferred the selection change to allow group drag.
    // Now on mouseUp, if no drag occurred, collapse to the single element.
    if (this.deferredSingleSelectUid !== undefined) {
      const didDrag =
        this.state.moveDelta !== undefined && (this.state.moveDelta.x !== 0 || this.state.moveDelta.y !== 0);
      const newSel = computeMouseUpSelection(this.deferredSingleSelectUid, didDrag);
      const wasDeferredText = this.deferredIsText;
      this.deferredSingleSelectUid = undefined;
      this.deferredIsText = undefined;
      if (newSel) {
        this.props.onSetSelection(newSel);
        if (wasDeferredText && newSel.size === 1) {
          const uid = only(newSel);
          const el = this.getElementByUid(uid);
          if (!el.isNamed()) {
            // Clouds and other non-named elements can't enter text editing
            this.selectionCenterOffset = undefined;
            this.setState(pointerStateReset());
            return;
          }
          const editingName = plainDeserialize('label', displayName(defined((el as NamedViewElement).name)));
          this.setState({
            isEditingName: true,
            editingName,
            moveDelta: undefined,
            isMovingArrow: false,
            isMovingSource: false,
          });
          this.selectionCenterOffset = undefined;
          return;
        }
      }
    }

    if (this.state.isMovingLabel && this.state.labelSide) {
      const selected = only(this.props.selection);
      this.props.onMoveLabel(selected, this.state.labelSide);
      this.clearPointerState(false);
      return;
    }

    if (this.selectionCenterOffset) {
      if (this.state.moveDelta) {
        const arcPoint = this.getArcPoint();
        const delta = this.state.moveDelta;

        if (this.state.editNameOnPointerUp) {
          let inCreation = this.state.inCreation;
          if (inCreation instanceof StockViewElement || inCreation instanceof AuxViewElement) {
            inCreation = inCreation.merge({
              x: inCreation.x - delta.x,
              y: inCreation.y - delta.y,
            });
          } else {
            throw new Error('invariant broken');
          }

          const editingName = plainDeserialize('label', displayName(defined((inCreation as NamedViewElement).name)));
          this.setState({
            isEditingName: true,
            editNameOnPointerUp: false,
            editingName,
            inCreation,
            moveDelta: undefined,
          });
          this.selectionCenterOffset = undefined;
          // we do weird one off things in this codepath, so exit early
          return;
        } else if (!this.state.isMovingArrow && !this.state.isMovingSource) {
          this.props.onMoveSelection(delta, arcPoint, this.state.draggingSegmentIndex);
        } else {
          const element = this.getElementByUid(only(this.props.selection));
          let foundInvalidTarget = false;
          const validTarget = this.cachedElements.find((e: ViewElement) => {
            const isValid = this.isValidTarget(e);
            foundInvalidTarget = foundInvalidTarget || isValid === false;
            return isValid || false;
          });
          if (element instanceof LinkViewElement && validTarget) {
            this.props.onAttachLink(element, defined(validTarget.ident));
          } else if (element instanceof FlowViewElement) {
            // don't create a flow stacked on top of 2 clouds due to a misclick
            if (this.state.moveDelta.x === 0 && this.state.moveDelta.y === 0 && this.state.inCreation) {
              this.clearPointerState();
              return;
            }
            const inCreation = !!this.state.inCreation;
            const isSourceAttach = this.state.isMovingSource;
            let fauxTargetCenter: Point | undefined;
            if (element.points.get(1)?.attachedToUid === fauxCloudTargetUid) {
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
              this.setState({
                isEditingName: true,
                editingName: plainDeserialize('label', displayName(defined(element.name))),
                flowStillBeingCreated: true,
              });
            }
          } else if (!foundInvalidTarget || this.state.inCreation) {
            this.props.onDeleteSelection();
          }
        }

        this.setState({
          moveDelta: undefined,
          inCreation: undefined,
          inCreationCloud: undefined,
          isMovingArrow: false,
          isMovingSource: false,
          draggingSegmentIndex: undefined,
        });
      } else if (this.state.isMovingArrow || this.state.isMovingSource) {
        // User clicked on flow arrowhead/source (or cloud) but didn't move.
        // Clear the movement flags so the cloud reappears.
        this.setState({
          isMovingArrow: false,
          isMovingSource: false,
        });
      }
      this.selectionCenterOffset = undefined;
      if (showDetails) {
        this.props.onShowVariableDetails();
      }
      return;
    }

    if (this.state.isMovingCanvas && this.state.movingCanvasOffset) {
      const newViewBox = this.props.view.viewBox.merge({
        x: this.state.movingCanvasOffset.x,
        y: this.state.movingCanvasOffset.y,
      });

      this.props.onViewBoxChange(newViewBox, this.props.view.zoom);
      this.setState({ movingCanvasOffset: undefined });

      // Start momentum animation for smooth deceleration
      this.startMomentumAnimation();
    }

    if (!this.mouseDownPoint) {
      return;
    }

    // Handle drag selection
    if (this.state.isDragSelecting && this.state.dragSelectionPoint) {
      const pointA = this.mouseDownPoint;
      const pointB = this.state.dragSelectionPoint;
      const canvasOffset = this.getCanvasOffset();

      // Calculate selection rectangle bounds
      const left = Math.min(pointA.x, pointB.x) - canvasOffset.x;
      const right = Math.max(pointA.x, pointB.x) - canvasOffset.x;
      const top = Math.min(pointA.y, pointB.y) - canvasOffset.y;
      const bottom = Math.max(pointA.y, pointB.y) - canvasOffset.y;

      // Find all elements within the selection rectangle
      let selectedElements = Set<UID>();
      for (const element of this.cachedElements) {
        // Clouds use center-point containment (no visible bounds to intersect with rect corners)
        if (element instanceof CloudViewElement) {
          if (element.cx >= left && element.cx <= right && element.cy >= top && element.cy <= bottom) {
            selectedElements = selectedElements.add(element.uid);
          }
        } else if (element instanceof AuxViewElement) {
          if (
            auxContains(element, { x: left, y: top }) ||
            auxContains(element, { x: right, y: top }) ||
            auxContains(element, { x: left, y: bottom }) ||
            auxContains(element, { x: right, y: bottom }) ||
            (element.cx >= left && element.cx <= right && element.cy >= top && element.cy <= bottom)
          ) {
            selectedElements = selectedElements.add(element.uid);
          }
        } else if (element instanceof StockViewElement) {
          // For stocks, check if center is within rectangle
          if (element.cx >= left && element.cx <= right && element.cy >= top && element.cy <= bottom) {
            selectedElements = selectedElements.add(element.uid);
          }
        } else if (element instanceof FlowViewElement) {
          // For flows, check if valve center (cx, cy) is within rectangle
          if (element.cx >= left && element.cx <= right && element.cy >= top && element.cy <= bottom) {
            selectedElements = selectedElements.add(element.uid);
          }
        } else if (element instanceof AliasViewElement || element instanceof ModuleViewElement) {
          // For other named elements, check if center is within rectangle
          if (element.cx >= left && element.cx <= right && element.cy >= top && element.cy <= bottom) {
            selectedElements = selectedElements.add(element.uid);
          }
        }
      }

      // Update selection
      this.props.onSetSelection(selectedElements);
      this.clearPointerState(false);
      return;
    }

    const clearSelection = !this.state.isMovingCanvas;
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

      const newViewBox = new ViewRect({
        x: canvasOffset.x + dWidth / 4,
        y: canvasOffset.y + dHeight / 4,
        width: contentRect.width,
        height: contentRect.height,
      });

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
    const newViewBox = this.props.view.viewBox.merge({
      x: newOffset.x,
      y: newOffset.y,
    });
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

    const newViewBox = viewBox.merge({
      x: viewBox.x - deltaX,
      y: viewBox.y - deltaY,
    });

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

    const newViewBox = viewBox.merge({
      x: newOffset.x,
      y: newOffset.y,
    });

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

  handleLabelDrag = (uid: number, e: React.PointerEvent<SVGElement>) => {
    this.pointerId = e.pointerId;

    const selectionSet = Set([uid]);
    if (!this.props.selection.equals(selectionSet)) {
      this.props.onSetSelection(selectionSet);
    }

    const element = this.getElementByUid(uid);
    const delta = this.getCanvasOffset();
    const client = this.getCanvasPoint(e.clientX, e.clientY);
    const pointer = {
      x: client.x - delta.x,
      y: client.y - delta.y,
    };

    const cx = element.cx;
    const cy = element.cy;

    const angle = radToDeg(Math.atan2(cy - pointer.y, cx - pointer.x));

    let side: 'top' | 'left' | 'bottom' | 'right';
    if (-45 < angle && angle <= 45) {
      side = 'left';
    } else if (45 < angle && angle <= 135) {
      side = 'top';
    } else if (-135 < angle && angle <= -45) {
      side = 'bottom';
    } else {
      side = 'right';
    }

    this.setState({
      isMovingLabel: true,
      labelSide: side,
    });
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

    this.setState({
      isMovingCanvas: true,
      movingCanvasOffset: newOffset,
    });
  }

  handleDragSelection(e: React.PointerEvent<SVGElement>): void {
    if (!this.mouseDownPoint) {
      return;
    }

    const dragSelectionPoint = this.getCanvasPoint(e.clientX, e.clientY);

    this.setState({
      isDragSelecting: true,
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
    if (this.state.isPinching && this.activePointers.size >= 2) {
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
    } else if (this.state.isDragSelecting) {
      this.handleDragSelection(e);
    } else if (this.state.isMovingCanvas) {
      this.handleMovingCanvas(e);
    }
  };

  // Handle pinch-to-zoom gesture movement
  handlePinchMove = (): void => {
    if (!this.state.isPinching || !this.state.pinchModelPoint) {
      return;
    }

    const currentDistance = this.getPinchDistance();
    if (currentDistance === 0 || this.state.initialPinchDistance === 0) {
      return;
    }

    // Calculate scale factor
    const scale = currentDistance / this.state.initialPinchDistance;
    let newZoom = this.state.initialPinchZoom * scale;

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
    const modelPoint = this.state.pinchModelPoint;
    const newOffset = {
      x: currentCenterCanvas.x - modelPoint.x,
      y: currentCenterCanvas.y - modelPoint.y,
    };

    const newViewBox = this.props.view.viewBox.merge({
      x: newOffset.x,
      y: newOffset.y,
    });

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

      this.setState({
        isPinching: true,
        initialPinchDistance: distance,
        initialPinchZoom: this.props.view.zoom,
        pinchModelPoint,
        isMovingCanvas: false,
        isDragSelecting: false,
        // Clear any canvas offset to prevent momentum from starting when exiting pinch
        movingCanvasOffset: undefined,
      });
      return;
    }

    // If already pinching and a third finger comes in, ignore it
    if (this.state.isPinching) {
      return;
    }

    // For non-primary touches when we already have a primary, track for potential pinch
    if (!e.isPrimary && this.pointerId !== undefined) {
      return;
    }

    const client = this.getCanvasPoint(e.clientX, e.clientY);

    const canvasOffset = this.getCanvasOffset();
    const { selectedTool } = this.props;
    if (selectedTool === 'aux' || selectedTool === 'stock') {
      let inCreation: AuxViewElement | StockViewElement;
      if (selectedTool === 'aux') {
        const name = this.getNewVariableName('New Variable');
        inCreation = new AuxViewElement({
          uid: inCreationUid,
          var: undefined,
          x: client.x - canvasOffset.x,
          y: client.y - canvasOffset.y,
          name,
          ident: canonicalize(name),
          labelSide: 'right',
          isZeroRadius: false,
        });
      } else {
        const name = this.getNewVariableName('New Stock');
        inCreation = new StockViewElement({
          uid: inCreationUid,
          var: undefined,
          x: client.x - canvasOffset.x,
          y: client.y - canvasOffset.y,
          name,
          ident: canonicalize(name),
          labelSide: 'bottom',
          isZeroRadius: false,
          inflows: List<UID>(),
          outflows: List<UID>(),
        });
      }

      this.pointerId = e.pointerId;
      this.selectionCenterOffset = client;

      (e.target as any).setPointerCapture(e.pointerId);

      this.setState({
        isEditingName: false,
        editNameOnPointerUp: true,
        inCreation,
        moveDelta: {
          x: 0,
          y: 0,
        },
      });
      this.props.onSetSelection(Set([inCreation.uid]));
      return;
    }
    this.pointerId = e.pointerId;

    if (selectedTool === 'flow') {
      const canvasOffset = this.getCanvasOffset();
      const x = client.x - canvasOffset.x;
      const y = client.y - canvasOffset.y;

      const inCreationCloud = new CloudViewElement({
        uid: inCreationCloudUid,
        flowUid: inCreationUid,
        x,
        y,
        isZeroRadius: false,
      });

      const name = this.getNewVariableName('New Flow');
      const inCreation = new FlowViewElement({
        uid: inCreationUid,
        var: undefined,
        name,
        ident: canonicalize(name),
        x,
        y,
        labelSide: 'bottom',
        points: List([
          new FlowPoint({ x, y, attachedToUid: inCreationCloud.uid }),
          new FlowPoint({ x, y, attachedToUid: fauxCloudTarget.uid }),
        ]),
        isZeroRadius: false,
      });

      this.selectionCenterOffset = client;

      this.setState({
        isEditingName: false,
        isMovingArrow: true,
        inCreation,
        inCreationCloud,
        moveDelta: {
          x: 0,
          y: 0,
        },
      });
      this.props.onSetSelection(Set([inCreation.uid]));
      return;
    }

    // onclick handlers are weird.  If we mouse down on a circle, move
    // off the circle, and mouse-up on the canvas, the canvas gets an
    // onclick.  Instead, capture where we mouse-down'd, and on mouse up
    // check if its the same.
    this.mouseDownPoint = this.getCanvasPoint(e.clientX, e.clientY);

    if (e.pointerType === 'touch' || e.shiftKey) {
      // Initialize velocity tracking for momentum
      this.velocityTracker.positions = [];
      const canvasOffset = this.getCanvasOffset();
      this.trackPosition(canvasOffset.x, canvasOffset.y);

      this.setState({
        isDragSelecting: false,
        isMovingCanvas: true,
        inCreation: undefined,
        inCreationCloud: undefined,
      });
    } else {
      // on mobile, no drag selection
      this.setState({
        isDragSelecting: true,
        isMovingCanvas: false,
        inCreation: undefined,
        inCreationCloud: undefined,
      });
    }
  };

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

    let isEditingName = !!isText;
    let editingName: Array<CustomElement> = [];
    let isMovingArrow = !!isArrowhead;
    let isMovingSource = !!isSource;

    this.pointerId = e.pointerId;

    // For multi-selection, use the click point as the offset
    // This ensures smooth dragging from where the user clicked
    this.selectionCenterOffset = this.getCanvasPoint(e.clientX, e.clientY);

    if (!isEditingName) {
      (e.target as any).setPointerCapture(e.pointerId);
    }

    const { selectedTool } = this.props;
    let inCreation: ViewElement | undefined;

    if (selectedTool === 'link' && element.isNamed()) {
      isEditingName = false;
      isMovingArrow = true;
      inCreation = new LinkViewElement({
        uid: inCreationUid,
        fromUid: element.uid,
        toUid: fauxTarget.uid,
        arc: 0.0,
        multiPoint: undefined,
        isStraight: false,
      });
      element = inCreation;
    } else if (selectedTool === 'flow' && element instanceof StockViewElement) {
      isEditingName = false;
      isMovingArrow = true;
      const startPoint = new FlowPoint({
        x: element.cx,
        y: element.cy,
        attachedToUid: element.uid,
      });
      const endPoint = new FlowPoint({
        x: element.cx,
        y: element.cy,
        attachedToUid: fauxCloudTarget.uid,
      });
      const name = this.getNewVariableName('New Flow');
      inCreation = new FlowViewElement({
        uid: inCreationUid,
        var: undefined,
        name: name,
        ident: canonicalize(name),
        x: element.cx,
        y: element.cy,
        labelSide: 'bottom',
        points: List([startPoint, endPoint]),
        isZeroRadius: false,
      });
      element = inCreation;
    } else {
      // Not a link/flow tool action -- compute selection and handle clouds
      this.props.onClearSelectedTool();

      const isMultiSelect = e.ctrlKey || e.metaKey || e.shiftKey;
      const { newSelection, deferSingleSelect } = computeMouseDownSelection(
        this.props.selection,
        element.uid,
        isMultiSelect,
      );

      if (deferSingleSelect !== undefined) {
        // Element is already in the selection and no modifier -- defer selection
        // change to mouseUp so that group drag works without dissolving selection
        this.deferredSingleSelectUid = deferSingleSelect;
        this.deferredIsText = isText;
        isEditingName = false;

        this.setState({
          isEditingName: false,
          editingName,
          isMovingArrow: false,
          isMovingSource: false,
          inCreation,
          moveDelta: { x: 0, y: 0 },
          draggingSegmentIndex: segmentIndex,
        });
        return;
      }

      // Cloud re-attachment only when the cloud will be the sole selection
      const willBeSoleSelection = newSelection !== undefined && newSelection.size === 1;
      if (element instanceof CloudViewElement && element.flowUid !== undefined && willBeSoleSelection) {
        let flow: FlowViewElement | undefined;
        try {
          const flowElement = this.getElementByUid(element.flowUid);
          if (flowElement instanceof FlowViewElement) {
            flow = flowElement;
          }
        } catch (e) {
          console.warn(`Cloud ${element.uid} references invalid flow ${element.flowUid}:`, e);
        }
        if (flow) {
          if (isCloudOnSourceSide(element, flow)) {
            isMovingSource = true;
            element = flow;
          } else if (isCloudOnSinkSide(element, flow)) {
            isMovingArrow = true;
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
        const enteredReattachment = isMovingSource || isMovingArrow;
        this.props.onSetSelection(resolveSelectionForReattachment(newSelection, enteredReattachment, element.uid));
      }
    }

    this.setState({
      isEditingName,
      editingName,
      isMovingArrow,
      isMovingSource,
      inCreation,
      moveDelta: { x: 0, y: 0 },
      draggingSegmentIndex: segmentIndex,
    });

    if (selectedTool === 'link' || selectedTool === 'flow') {
      this.props.onSetSelection(Set([element.uid]));
    }
  };

  handleEditingNameChange = (value: Descendant[]): void => {
    this.setState({ editingName: value });
  };

  handleEditingNameDone = (isCancel: boolean) => {
    if (!this.state.isEditingName) {
      return;
    }

    if (isCancel) {
      if (this.state.flowStillBeingCreated) {
        this.setState({
          flowStillBeingCreated: true,
        });
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
      this.props.onCreateVariable((element as unknown as any).set('name', newName));
    } else {
      this.props.onRenameVariable(oldName, newName);
    }

    this.clearPointerState();
  };

  focusCanvas() {
    // an SVG element can't actually be focused.  Instead, blur any _other_
    // focused element.
    if (typeof document !== 'undefined' && document && document.activeElement) {
      const e: any = document.activeElement;
      // blur doesn't exist on "Element" but it definitely is a real thing

      e.blur();
    }
  }

  buildLayers(displayElements: List<ViewElement>): React.ReactElement[][] {
    // create different layers for each of the display types so that views compose together nicely
    const zLayers = new Array(ZMax) as React.ReactElement[][];
    for (let i = 0; i < ZMax; i++) {
      zLayers[i] = [];
    }

    for (let element of displayElements) {
      if (this.selectionUpdates.has(element.uid)) {
        element = getOrThrow(this.selectionUpdates, element.uid);
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
      if (element instanceof AuxViewElement) {
        component = this.aux(element);
        zOrder = 5;
      } else if (element instanceof LinkViewElement) {
        component = this.connector(element);
        zOrder = 2;
      } else if (element instanceof StockViewElement) {
        component = this.stock(element);
        zOrder = 4;
      } else if (element instanceof FlowViewElement) {
        component = this.flow(element);
        zOrder = 3;
      } else if (element instanceof CloudViewElement) {
        component = this.cloud(element);
        zOrder = 4;
      } else if (element instanceof AliasViewElement) {
        component = this.alias(element);
        zOrder = 5;
      } else if (element instanceof ModuleViewElement) {
        component = this.module(element);
        zOrder = 4;
      } else if (element instanceof GroupViewElement) {
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
    const displayElements = this.renderInitAndCache();

    this.computeBounds = true;
    if (this.computeBounds) {
      this.elementBounds = List<Rect | undefined>();
    }

    // we are ignoring the result here, because we're calling it for
    // the side effect of computing individual bounds
    this.buildLayers(displayElements);

    let initialBounds: ViewRect | undefined;
    const bounds = calcViewBox(this.elementBounds);
    if (bounds) {
      const left = Math.floor(bounds.left) - 10;
      const top = Math.floor(bounds.top) - 10;
      const width = Math.ceil(bounds.right - left) + 10;
      const height = Math.ceil(bounds.bottom - top) + 10;
      initialBounds = new ViewRect({
        x: left,
        y: top,
        width,
        height,
      });
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

      const newViewBox = new ViewRect({
        x,
        y,
        width: svgWidth,
        height: svgHeight,
      });

      this.props.onViewBoxChange(newViewBox, zoom);

      this.setState({
        svgSize: {
          width: svgWidth,
          height: svgHeight,
        },
      });
    }

    this.computeBounds = false;
    this.elementBounds = List<Rect | undefined>();
  }

  render() {
    const { selectedTool, embedded } = this.props;

    let isEditingName = this.state.isEditingName;
    if (isEditingName && selectedTool !== this.prevSelectedTool) {
      setTimeout(() => {
        this.handleEditingNameDone(false);
      });
      isEditingName = false;
    }
    this.prevSelectedTool = selectedTool;

    // phase 1: initialize some data structures we need and potentially
    // invalidate cached data structures we have
    const displayElements = this.renderInitAndCache();

    if (embedded) {
      this.computeBounds = true;
    }

    // phase 2: create React components and add them to the appropriate layer
    const zLayers = this.buildLayers(displayElements);

    let overlayClass = styles.overlay;
    let nameEditor;

    let dragRect;
    if (this.state.isDragSelecting && this.mouseDownPoint && this.state.dragSelectionPoint) {
      const pointA = this.mouseDownPoint;
      const pointB = this.state.dragSelectionPoint;
      const offset = this.getCanvasOffset();

      const x = Math.min(pointA.x, pointB.x) - offset.x;
      const y = Math.min(pointA.y, pointB.y) - offset.y;
      const w = Math.abs(pointA.x - pointB.x);
      const h = Math.abs(pointA.y - pointB.y);

      dragRect = <rect className={styles.dragRectOverlay} x={x} y={y} width={w} height={h} />;
    }

    if (!isEditingName || this.props.selection.isEmpty()) {
      overlayClass += ' ' + styles.noPointerEvents;
    } else {
      const zoom = this.props.view.zoom;
      const editingUid = only(this.props.selection);
      const editingElement = this.getElementByUid(editingUid) as NamedViewElement;
      const rw = editingElement instanceof StockViewElement ? StockWidth / 2 : AuxRadius;
      const rh = editingElement instanceof StockViewElement ? StockHeight / 2 : AuxRadius;
      const side = editingElement.labelSide;
      const offset = this.getCanvasOffset();
      nameEditor = (
        <EditableLabel
          uid={editingUid}
          cx={(editingElement.cx + offset.x) * zoom}
          cy={(editingElement.cy + offset.y) * zoom}
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
      const bounds = calcViewBox(this.elementBounds);
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

    // we don't need these things anymore

    if (this.elementBounds) {
      this.elementBounds = List<Rect | undefined>();
    }
    this.selectionUpdates = Map<UID, ViewElement>();
    // n.b. we don't want to clear this.elements as thats used when handling callbacks

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
