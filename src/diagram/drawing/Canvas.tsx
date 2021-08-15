// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/// <reference types="resize-observer-browser" />

import * as React from 'react';

import { createStyles, Theme, withStyles, WithStyles } from '@material-ui/core/styles';

import { Descendant } from 'slate';

import { List, Map, Set } from 'immutable';

import { defined, exists } from '@system-dynamics/core/common';

import {
  ViewElement,
  AliasViewElement,
  AuxViewElement,
  CloudViewElement,
  FlowViewElement,
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
} from '@system-dynamics/core/datamodel';

import { Alias, AliasProps } from './Alias';
import { Aux, auxBounds, auxContains, AuxProps } from './Auxiliary';
import { Cloud, cloudBounds, cloudContains, CloudProps } from './Cloud';
import { calcViewBox, displayName, plainDeserialize, plainSerialize, Point, Rect, screenToCanvasPoint } from './common';
import { Connector, ConnectorProps } from './Connector';
import { AuxRadius } from './default';
import { EditableLabel } from './EditableLabel';
import { Flow, UpdateCloudAndFlow, UpdateFlow, UpdateStockAndFlows } from './Flow';
import { Module, moduleBounds, ModuleProps } from './Module';
import { CustomText } from './SlateEditor';
import { Stock, stockBounds, stockContains, StockHeight, StockProps, StockWidth } from './Stock';

import { canonicalize } from '@system-dynamics/core/canonicalize';

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

const styles = ({ palette }: Theme) =>
  createStyles({
    canvas: {
      boxSizing: 'border-box',
      height: '100%',
      width: '100%',
      userSelect: 'none',
      '-webkit-touch-callout': 'none',
      '& text': {
        fill: palette.common.black,
        fontSize: '12px',
        fontFamily: '"Roboto", "Open Sans", "Arial", sans-serif',
        fontWeight: 300,
        textAnchor: 'middle',
        whiteSpace: 'nowrap',
        verticalAlign: 'middle',
      },
    },
    container: {
      height: '100%',
      width: '100%',
    },
    overlay: {
      position: 'absolute',
      top: 0,
      left: 0,
      height: '100%',
      width: '100%',
    },
    noPointerEvents: {
      pointerEvents: 'none',
      touchAction: 'none',
    },
    selectionOverlay: {
      stroke: '#4444dd',
      strokeWidth: 1,
      fill: '#6363ff',
      opacity: 0.2,
      zIndex: 10,
      transform: 'translateZ(1)',
    },
    // gLayer: {
    //   willChange: 'translate',
    // },
  });

function radToDeg(r: number): number {
  return (r * 180) / Math.PI;
}

const ZMax = 6;

interface CanvasState {
  isMovingCanvas: boolean;
  isDragSelecting: boolean;
  isEditingName: boolean;
  isMovingArrow: boolean;
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
}

interface CanvasPropsFull extends WithStyles<typeof styles> {
  embedded: boolean;
  project: Project;
  model: Model;
  view: StockFlowView;
  version: number;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;
  selection: Set<UID>;
  onRenameVariable: (oldName: string, newName: string) => void;
  onSetSelection: (selected: Set<UID>) => void;
  onMoveSelection: (position: Point, arcPoint?: Point) => void;
  onMoveFlow: (
    flow: FlowViewElement,
    targetUid: number,
    moveDelta: Point,
    fauxTargetCenter: Point | undefined,
    inCreation: boolean,
  ) => void;
  onMoveLabel: (uid: UID, side: 'top' | 'left' | 'bottom' | 'right') => void;
  onAttachLink: (link: LinkViewElement, newTarget: string) => void;
  onCreateVariable: (element: ViewElement) => void;
  onClearSelectedTool: () => void;
  onDeleteSelection: () => void;
  onShowVariableDetails: () => void;
  onViewBoxChange: (viewBox: ViewRect, zoom: number) => void;
}

export type CanvasProps = Pick<
  CanvasPropsFull,
  | 'embedded'
  | 'project'
  | 'model'
  | 'view'
  | 'version'
  | 'selectedTool'
  | 'selection'
  | 'onRenameVariable'
  | 'onSetSelection'
  | 'onMoveSelection'
  | 'onMoveFlow'
  | 'onAttachLink'
  | 'onCreateVariable'
  | 'onClearSelectedTool'
  | 'onDeleteSelection'
  | 'onViewBoxChange'
>;

export const Canvas = withStyles(styles)(
  class InnerCanvas extends React.PureComponent<CanvasPropsFull, CanvasState> {
    state: CanvasState;

    readonly svgRef: React.RefObject<InstanceType<typeof HTMLDivElement>>;
    private svgObserver: ResizeObserver | undefined;

    private mouseDownPoint: Point | undefined;
    private selectionCenterOffset: Point | undefined;

    private pointerId: number | undefined;

    private elementBounds = List<Rect | undefined>();

    private prevSelectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;

    // we have to regenerate selectionUpdates when selection !== props.selection
    private selection = Set<UID>();

    private cachedVersion = -Infinity;
    private cachedElements = List<ViewElement>();
    private elements = Map<UID, ViewElement>();
    private selectionUpdates = Map<UID, ViewElement>();

    private computeBounds = false;

    constructor(props: CanvasPropsFull) {
      super(props);

      this.svgRef = React.createRef();

      this.state = {
        isMovingArrow: false,
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
          const e = defined(elements.get(uid));
          selection = selection.set(e.uid, e);
        }
      }
      return selection;
    }

    private isSelected(element: ViewElement): boolean {
      return this.props.selection.has(element.uid);
    }

    private alias(element: AliasViewElement): React.ReactElement {
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

    private cloud(element: CloudViewElement): React.ReactElement | undefined {
      const isSelected = this.isSelected(element);

      const flow = this.getElementByUid(defined(element.flowUid)) as FlowViewElement;

      if (this.state.isMovingArrow && this.isSelected(flow)) {
        if (defined(flow.points.last()).attachedToUid === element.uid) {
          return undefined;
        }
      }

      const props: CloudProps = {
        element,
        isSelected,
        onSelection: this.handleSetSelection,
      };

      if (this.computeBounds) {
        this.elementBounds = this.elementBounds.push(cloudBounds(element));
      }

      return <Cloud key={element.uid} {...props} />;
    }

    private isValidTarget(element: ViewElement): boolean | undefined {
      if (!this.state.isMovingArrow || !this.selectionCenterOffset) {
        return undefined;
      }

      const arrowUid = defined(this.props.selection.first());
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
        const first = defined(arrow.points.first());
        // make sure we don't point a flow back at its source
        if (first.attachedToUid === element.uid) {
          return false;
        }
        return Math.abs(first.x - element.cx) < StockWidth / 2 || Math.abs(first.y - element.cy) < StockHeight / 2;
      }

      return element instanceof FlowViewElement || element instanceof AuxViewElement;
    }

    private aux(element: AuxViewElement): React.ReactElement {
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

    private stock(element: StockViewElement): React.ReactElement {
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

    private module(element: ModuleViewElement) {
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

    private connector(element: LinkViewElement) {
      const { isMovingArrow } = this.state;
      const isSelected = this.props.selection.has(element.uid);

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
      if (isMovingArrow || this.isSelected(from) || this.isSelected(to)) {
        const oldTo = defined(this.elements.get(toUid));
        const oldFrom = defined(this.elements.get(from.uid));
        const oldθ = Math.atan2(oldTo.cy - oldFrom.cy, oldTo.cx - oldFrom.cx);
        const newθ = Math.atan2(to.cy - from.cy, to.cx - from.cx);
        const diffθ = oldθ - newθ;
        const angle = element.arc || 180.0;
        element = element.set('arc', angle - radToDeg(diffθ));
      }
      const props: ConnectorProps = {
        element,
        from,
        to,
        isSelected,
        onSelection: this.handleEditConnector,
      };
      if (isSelected && !isSticky) {
        props.arcPoint = this.getArcPoint();
      }
      // this.elementBounds = this.elementBounds.push(Connector.bounds(props));
      return <Connector key={element.uid} {...props} />;
    }

    private getArcPoint(): FlowPoint | undefined {
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

    private flow(element: FlowViewElement) {
      const variable = this.props.model.variables.get(element.ident);
      const hasWarning = variable?.hasError || false;
      const { isMovingArrow } = this.state;
      const isSelected = this.isSelected(element);
      const series = variable?.data;

      if (element.points.size < 2) {
        return;
      }

      const sourceId = defined(element.points.first()).attachedToUid;
      if (!sourceId) {
        return;
      }
      const source = this.getElementByUid(sourceId);
      if (!(source instanceof StockViewElement || source instanceof CloudViewElement)) {
        throw new Error('invariant broken');
      }

      const sinkId = defined(element.points.last()).attachedToUid;
      if (!sinkId) {
        return;
      }
      const sink = this.getElementByUid(sinkId);
      if (!(sink instanceof StockViewElement || sink instanceof CloudViewElement)) {
        throw new Error('invariant broken');
      }

      return (
        <Flow
          key={element.uid}
          element={element}
          series={series}
          source={source}
          sink={sink}
          isSelected={isSelected}
          hasWarning={hasWarning}
          isMovingArrow={isSelected && isMovingArrow}
          isEditingName={isSelected && this.state.isEditingName}
          isValidTarget={this.isValidTarget(element)}
          onSelection={this.handleSetSelection}
          onLabelDrag={this.handleLabelDrag}
        />
      );
    }

    private constrainFlowMovement(
      flow: FlowViewElement,
      moveDelta: Point,
    ): [FlowViewElement, List<StockViewElement | CloudViewElement>] {
      if (flow.points.size !== 2) {
        console.log('TODO: non-simple flow');
        return [flow, List<StockViewElement | CloudViewElement>()];
      }

      const sourceId = defined(defined(flow.points.first()).attachedToUid);
      const source = this.getElementByUid(sourceId) as StockViewElement | CloudViewElement;
      if (!(source instanceof StockViewElement || source instanceof CloudViewElement)) {
        throw new Error('invariant broken');
      }

      const sinkId = defined(defined(defined(flow.points.last()).attachedToUid));
      let sink = this.getElementByUid(sinkId) as StockViewElement | CloudViewElement;
      if (!(sink instanceof StockViewElement || sink instanceof CloudViewElement)) {
        throw new Error('invariant broken');
      }

      const { isMovingArrow } = this.state;
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
          // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-unsafe-assignment
          sink = ((sink as unknown) as any).merge({
            x: off.x - canvasOffset.x,
            y: off.y - canvasOffset.y,
            isZeroRadius: true,
          });
        }

        [sink, flow] = UpdateCloudAndFlow(sink, flow, moveDelta);
        return [flow, List([])];
      }

      const ends = List<StockViewElement | CloudViewElement>([source, sink]);
      return UpdateFlow(flow, ends, moveDelta);
    }

    private constrainCloudMovement(
      cloudEl: CloudViewElement,
      moveDelta: Point,
    ): [StockViewElement | CloudViewElement, FlowViewElement] {
      const flow = this.getElementByUid(defined(cloudEl.flowUid)) as FlowViewElement;
      return UpdateCloudAndFlow(cloudEl, flow, moveDelta);
    }

    private constrainStockMovement(
      stockEl: StockViewElement,
      moveDelta: Point,
    ): [StockViewElement, List<FlowViewElement>] {
      const flows = List<FlowViewElement>(
        stockEl.inflows
          .concat(stockEl.outflows)
          .map((uid) => (this.selectionUpdates.get(uid) || this.getElementByUid(uid)) as FlowViewElement | undefined)
          .filter((element) => element !== undefined)
          .map((element) => defined(element)),
      );

      return UpdateStockAndFlows(stockEl, flows, moveDelta);
    }

    private renderInitAndCache(): List<ViewElement> {
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

      this.selectionUpdates = InnerCanvas.buildSelectionMap(this.props, this.elements, this.state.inCreation);
      if (this.state.labelSide) {
        this.selectionUpdates = this.selectionUpdates.map((el) => {
          // eslint-disable-next-line @typescript-eslint/no-unsafe-call
          return (el as AuxViewElement).set('labelSide', defined(this.state.labelSide));
        });
      }
      if (this.state.moveDelta) {
        let otherUpdates = List<ViewElement>();
        const { x, y } = defined(this.state.moveDelta);
        this.selectionUpdates = this.selectionUpdates.map((initialEl) => {
          // only constrain flow movement if we're not doing a group-move
          if (initialEl instanceof FlowViewElement && this.selectionUpdates.size === 1) {
            const [flow, updatedClouds] = this.constrainFlowMovement(initialEl, defined(this.state.moveDelta));
            otherUpdates = otherUpdates.concat(updatedClouds);
            return flow;
          } else if (initialEl instanceof StockViewElement && this.selectionUpdates.size === 1) {
            const [stock, updatedFlows] = this.constrainStockMovement(initialEl, defined(this.state.moveDelta));
            otherUpdates = otherUpdates.concat(updatedFlows);
            return stock;
          } else if (initialEl instanceof CloudViewElement && this.selectionUpdates.size === 1) {
            const [cloud, updatedFlow] = this.constrainCloudMovement(initialEl, defined(this.state.moveDelta));
            otherUpdates = otherUpdates.push(updatedFlow);
            return cloud;
          } else if (!(initialEl instanceof LinkViewElement)) {
            // eslint-disable-next-line @typescript-eslint/no-unsafe-call
            return (initialEl as AuxViewElement).merge({
              x: initialEl.cx - x,
              y: initialEl.cy - y,
            });
          } else {
            return initialEl;
          }
        });
        // now add flows that also were updated
        const namedUpdates: Map<UID, ViewElement> = otherUpdates.toMap().mapKeys((_, el) => el.uid);
        this.selectionUpdates = this.selectionUpdates.concat(namedUpdates);
      }

      return displayElements;
    }

    clearPointerState(clearSelection = true): void {
      this.pointerId = undefined;
      this.mouseDownPoint = undefined;
      this.selectionCenterOffset = undefined;

      this.setState({
        isMovingCanvas: false,
        isMovingArrow: false,
        isEditingName: false,
        isDragSelecting: false,
        isMovingLabel: false,
        labelSide: undefined,
        dragSelectionPoint: undefined,
        inCreation: undefined,
        inCreationCloud: undefined,
      });

      if (clearSelection) {
        this.props.onSetSelection(Set());
      }

      this.focusCanvas();
    }

    handlePointerCancel = (e: React.PointerEvent<SVGElement>): void => {
      if (this.props.embedded) {
        return;
      }
      if (!this.pointerId || this.pointerId !== e.pointerId) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();

      this.props.onShowVariableDetails();

      this.pointerId = undefined;

      if (this.state.isMovingLabel && this.state.labelSide) {
        const selected = defined(this.props.selection.first());
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

            const editingName = plainDeserialize(displayName(defined((inCreation as NamedViewElement).name)));
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
          } else if (!this.state.isMovingArrow) {
            this.props.onMoveSelection(delta, arcPoint);
          } else {
            const element = this.getElementByUid(defined(this.props.selection.first()));
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
              let fauxTargetCenter: Point | undefined;
              if (element.points.get(1)?.attachedToUid === fauxCloudTargetUid) {
                const canvasOffset = this.getCanvasOffset();
                fauxTargetCenter = {
                  x: this.selectionCenterOffset.x - canvasOffset.x,
                  y: this.selectionCenterOffset.y - canvasOffset.y,
                };
              }
              this.props.onMoveFlow(element, validTarget ? validTarget.uid : 0, delta, fauxTargetCenter, inCreation);
              if (inCreation) {
                this.setState({
                  isEditingName: true,
                  editingName: plainDeserialize(displayName(defined(element.name))),
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
          });
        }
        this.selectionCenterOffset = undefined;
        return;
      }

      if (this.state.isMovingCanvas && this.state.movingCanvasOffset) {
        const newViewBox = this.props.view.viewBox.merge({
          x: this.state.movingCanvasOffset.x,
          y: this.state.movingCanvasOffset.y,
        });

        this.props.onViewBoxChange(newViewBox, this.props.view.zoom);
        this.setState({ movingCanvasOffset: undefined });
      }

      if (!this.mouseDownPoint) {
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

      this.setState({
        isMovingCanvas: true,
        movingCanvasOffset: {
          x: base.x + (curr.x - this.mouseDownPoint.x),
          y: base.y + (curr.y - this.mouseDownPoint.y),
        },
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

      if (this.pointerId !== e.pointerId) {
        return;
      } else if (this.pointerId && e.pointerType === 'mouse' && e.buttons === 0) {
        this.handlePointerCancel(e);
      }
      // e.preventDefault();
      // e.stopPropagation();

      if (this.selectionCenterOffset) {
        this.handleSelectionMove(e);
      } else if (this.state.isDragSelecting) {
        this.handleDragSelection(e);
      } else if (this.state.isMovingCanvas) {
        this.handleMovingCanvas(e);
      }
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

      if (!e.isPrimary) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();

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

        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
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
    ): void => {
      if (this.props.embedded) {
        return;
      }

      let isEditingName = !!isText;
      let editingName: Array<CustomText> = [];
      let isMovingArrow = !!isArrowhead;

      this.pointerId = e.pointerId;
      this.selectionCenterOffset = this.getCanvasPoint(e.clientX, e.clientY);

      if (!isEditingName) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
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
        // not an action we recognize, deselect the tool and continue on
        this.props.onClearSelectedTool();

        // single-element selection only for now
        const selection = Set([element.uid]);

        if (isEditingName) {
          const uid = defined(selection.first());
          const editingElement = this.getElementByUid(uid) as NamedViewElement;
          editingName = plainDeserialize(displayName(defined(editingElement.name)));
        }
      }

      this.setState({
        isEditingName,
        editingName,
        isMovingArrow,
        inCreation,
        moveDelta: {
          x: 0,
          y: 0,
        },
      });

      this.props.onSetSelection(Set([element.uid]));
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

      const uid = defined(this.props.selection.first());
      const element = this.getElementByUid(uid);
      const oldName = displayName(defined((element as NamedViewElement).name));
      const newName = plainSerialize(defined(this.state.editingName));

      if (uid === inCreationUid) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
        this.props.onCreateVariable(((element as unknown) as any).set('name', newName));
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
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
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
          element = defined(this.selectionUpdates.get(element.uid));
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
        }

        if (!component) {
          continue;
        }

        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
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
      const { selectedTool, embedded, classes } = this.props;

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

      let overlayClass = classes.overlay;
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

        dragRect = <rect className={classes.selectionOverlay} x={x} y={y} width={w} height={h} />;
      }

      if (!isEditingName || this.props.selection.isEmpty()) {
        overlayClass += ' ' + classes.noPointerEvents;
      } else {
        const zoom = this.props.view.zoom;
        const editingUid = defined(this.props.selection.first());
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
            rw={rw}
            rh={rh}
            value={defined(this.state.editingName)}
            onChange={this.handleEditingNameChange}
            onDone={this.handleEditingNameDone}
          />
        );
      }

      let transform;
      let viewBox: string | undefined;
      if (embedded) {
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
        <div className={classes.container} ref={this.svgRef}>
          <svg
            viewBox={viewBox}
            preserveAspectRatio="xMinYMin"
            className={classes.canvas}
            onPointerDown={this.handlePointerDown}
            onPointerMove={this.handlePointerMove}
            onPointerCancel={this.handlePointerCancel}
            onPointerUp={this.handlePointerCancel}
          >
            <defs />
            <g transform={transform}>
              {zLayers}
              {dragRect}
            </g>
          </svg>
          {overlay}
        </div>
      );
    }
  },
);
