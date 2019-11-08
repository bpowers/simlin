// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { Operation, Value } from 'slate';
import Plain from 'slate-plain-serializer';

import { List, Map, Set } from 'immutable';

import { defined, Series } from '../../common';

import { Model } from '../../../engine/model';
import { Project } from '../../../engine/project';
import { Stock as StockVar } from '../../../engine/vars';
import { Point as XmilePoint, UID, View, ViewElement } from '../../../engine/xmile';

import { Aux, auxBounds, auxContains, AuxProps } from './Aux';
import { Cloud, cloudBounds, cloudContains, CloudProps } from './Cloud';
import { calcViewBox, displayName, Point, Rect } from './common';
import { findSide } from './CommonLabel';
import { Connector, ConnectorProps } from './Connector';
import { AuxRadius } from './default';
import { EditableLabel } from './EditableLabel';
import { Flow, UpdateCloudAndFlow, UpdateFlow, UpdateStockAndFlows } from './Flow';
import { Module, moduleBounds, ModuleProps } from './Module';
import { Stock, stockBounds, stockContains, StockHeight, StockProps, StockWidth } from './Stock';

export const inCreationUid = -2;
export const fauxTargetUid = -3;
export const inCreationCloudUid = -4;

const fauxTarget = new ViewElement({
  type: 'aux',
  name: '$·model-internal-faux-target',
  uid: fauxTargetUid,
  x: 0,
  y: 0,
});

const styles = createStyles({
  canvas: {
    boxSizing: 'border-box',
    height: '100%',
    width: '100%',
    userSelect: 'none',
    '-webkit-touch-callout': 'none',
  },
  container: {
    height: '100%',
    width: '100%',
    '& text': {
      fontSize: '12px',
      fontFamily: '"Roboto", "Open Sans", "Arial", sans-serif',
      fontWeight: 300,
      textAnchor: 'middle',
      whiteSpace: 'nowrap',
      verticalAlign: 'middle',
    },
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
  gLayer: {
    // transform: 'translateZ(-1)',
  },
});

const FlowSource = 0;
const FlowSink = 1;

function radToDeg(r: number): number {
  return (r * 180) / Math.PI;
}

const ZOrder = Map<
  'flow' | 'module' | 'stock' | 'aux' | 'connector' | 'style' | 'reference' | 'cloud' | 'alias',
  number
>([
  ['style', 0],
  ['module', 1],
  ['connector', 2],
  ['flow', 3],
  ['cloud', 4],
  ['stock', 4],
  ['aux', 5],
  ['reference', 5],
  ['alias', 5],
]);

const ZMax = 6;

type WellKnownElement = 'stock' | 'flow' | 'aux' | 'connector' | 'module' | 'alias' | 'cloud';

const KnownTypes = Set<string>(['stock', 'flow', 'aux', 'connector', 'module', 'alias', 'cloud']);

interface CanvasState {
  isMovingCanvas: boolean;
  isDragSelecting: boolean;
  isEditingName: boolean;
  isMovingArrow: boolean;
  isMovingLabel: boolean;
  labelSide: 'right' | 'bottom' | 'left' | 'top' | undefined;
  editingName: Value | undefined;
  dragSelectionPoint: Point | undefined;
  moveDelta: Point | undefined;
  canvasOffset: Point;
  inCreation: ViewElement | undefined;
  inCreationCloud: ViewElement | undefined;
}

interface CanvasPropsFull extends WithStyles<typeof styles> {
  embedded: boolean;
  project: Project;
  model: Model;
  view: View;
  data: Map<string, Series>;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;
  selection: Set<UID>;
  onRenameVariable: (oldName: string, newName: string) => void;
  onSetSelection: (selected: Set<UID>) => void;
  onMoveSelection: (position: Point, arcPoint?: Point) => void;
  onMoveFlow: (link: ViewElement, targetUid: number, moveDetla: Point) => void;
  onMoveLabel: (uid: UID, side: 'top' | 'left' | 'bottom' | 'right') => void;
  onAttachLink: (link: ViewElement, newTarget: string) => void;
  onCreateVariable: (element: ViewElement) => void;
  onClearSelectedTool: () => void;
  onDeleteSelection: () => void;
}

export type CanvasProps = Pick<
  CanvasPropsFull,
  | 'embedded'
  | 'project'
  | 'model'
  | 'view'
  | 'data'
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
>;

export const Canvas = withStyles(styles)(
  class InnerCanvas extends React.PureComponent<CanvasPropsFull, CanvasState> {
    state: CanvasState;

    private mouseDownPoint: Point | undefined;
    private selectionCenterOffset: Point | undefined;
    private prevCanvasOffset: Point | undefined;

    private pointerId: number | undefined;

    private elementBounds = List<Rect | undefined>();

    // we have to regenerate selectionUpdates when selection !== props.selection
    private selection = Set<UID>();

    private cachedElements = List<ViewElement>();
    private elements = Map<UID, ViewElement>();
    private nameMap = Map<string, UID>();
    private selectionUpdates = Map<UID, ViewElement>();

    // a helper object to go from well-known element types to constructors
    readonly builder: {
      [K in WellKnownElement]: (v: ViewElement) => React.ReactElement | undefined;
    };

    constructor(props: CanvasPropsFull) {
      super(props);

      this.builder = {
        aux: this.aux,
        stock: this.stock,
        flow: this.flow,
        connector: this.connector,
        module: this.module,
        alias: this.alias,
        cloud: this.cloud,
      };

      this.state = {
        isMovingArrow: false,
        isMovingCanvas: false,
        isDragSelecting: false,
        isEditingName: false,
        isMovingLabel: false,
        labelSide: undefined,
        editingName: undefined,
        dragSelectionPoint: undefined,
        moveDelta: undefined,
        canvasOffset: { x: 0, y: 0 },
        inCreation: undefined,
        inCreationCloud: undefined,
      };
    }

    getElementByUid(uid: UID): ViewElement {
      if (uid === inCreationUid) {
        return defined(this.state.inCreation);
      } else if (uid === fauxTargetUid) {
        return fauxTarget;
      } else if (uid === inCreationCloudUid) {
        return defined(this.state.inCreationCloud);
      }
      return defined(this.elements.get(uid));
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

    private getNamedElement(name: string): ViewElement | undefined {
      const uid = defined(this.nameMap.get(name));
      return this.selectionUpdates.get(uid) || this.elements.get(uid);
    }

    private isSelected(element: ViewElement): boolean {
      return this.props.selection.has(element.uid);
    }

    private alias = (element: ViewElement): React.ReactElement => {
      // FIXME
      return this.aux(element, true);
    };

    private cloud = (element: ViewElement): React.ReactElement | undefined => {
      const isSelected = this.isSelected(element);

      const flow = this.getElementByUid(defined(element.flowUid));

      if (this.state.isMovingArrow && this.isSelected(flow)) {
        if (defined(defined(flow.pts).last()).uid === element.uid) {
          return undefined;
        }
      }

      const props: CloudProps = {
        element,
        isSelected,
        onSelection: this.handleSetSelection,
      };

      this.elementBounds = this.elementBounds.push(cloudBounds(element));

      return <Cloud key={element.uid} {...props} />;
    };

    private isValidTarget(element: ViewElement): boolean | undefined {
      if (!this.state.isMovingArrow || !this.selectionCenterOffset) {
        return undefined;
      }

      const arrowUid = defined(this.props.selection.first());
      const arrow = this.getElementByUid(arrowUid);

      const off = this.selectionCenterOffset;
      const delta = this.state.moveDelta || { x: 0, y: 0 };
      const pointer = {
        x: off.x - delta.x - this.state.canvasOffset.x,
        y: off.y - delta.y - this.state.canvasOffset.y,
      };

      let isTarget = false;
      switch (element.type) {
        case 'cloud':
          isTarget = cloudContains(element, pointer);
          break;
        case 'stock':
          isTarget = stockContains(element, pointer);
          break;
        case 'flow':
        case 'aux':
          isTarget = auxContains(element, pointer);
          break;
      }
      if (!isTarget) {
        return undefined;
      }

      if (arrow.type === 'flow') {
        if (element.type !== 'stock') {
          return false;
        }
        const first = defined(defined(arrow.pts).first());
        // make sure we don't point a flow back at its source
        if (first.uid === element.uid) {
          return false;
        }
        return Math.abs(first.x - element.cx) < StockWidth / 2 || Math.abs(first.y - element.cy) < StockHeight / 2;
      }

      return element.type === 'flow' || element.type === 'aux';
    }

    private aux = (element: ViewElement, isGhost: boolean = false): React.ReactElement => {
      const variableErrors = this.props.model.vars.get(element.ident)?.errors.size || 0;
      const isSelected = this.isSelected(element);
      const series = this.props.data.get(element.ident);
      const props: AuxProps = {
        element,
        series,
        isSelected,
        isEditingName: isSelected && this.state.isEditingName,
        isValidTarget: this.isValidTarget(element),
        onSelection: this.handleSetSelection,
        onLabelDrag: this.handleLabelDrag,
        hasWarning: variableErrors > 0,
      };

      this.elementBounds = this.elementBounds.push(auxBounds(element));

      return <Aux key={element.ident} {...props} />;
    };

    private stock = (element: ViewElement): React.ReactElement => {
      const variableErrors = this.props.model.vars.get(element.ident)?.errors.size || 0;
      const isSelected = this.isSelected(element);
      const series = this.props.data.get(element.ident);
      const props: StockProps = {
        element,
        series,
        isSelected,
        isEditingName: isSelected && this.state.isEditingName,
        isValidTarget: this.isValidTarget(element),
        onSelection: this.handleSetSelection,
        onLabelDrag: this.handleLabelDrag,
        hasWarning: variableErrors > 0,
      };
      this.elementBounds = this.elementBounds.push(stockBounds(element));
      return <Stock key={element.ident} {...props} />;
    };

    private module = (element: ViewElement) => {
      const isSelected = this.isSelected(element);
      const props: ModuleProps = {
        element,
        isSelected,
      };
      this.elementBounds = this.elementBounds.push(moduleBounds(props));
      return <Module key={element.ident} {...props} />;
    };

    private connector = (element: ViewElement) => {
      const { isMovingArrow } = this.state;
      const isSelected = this.props.selection.has(element.uid);

      const from = this.getNamedElement(defined(element.from));
      if (!from) {
        console.log(`connector with unknown from ${element.from}`);
        return;
      }
      let to = this.getNamedElement(defined(element.to));
      if (!to) {
        console.log(`connector with unknown to ${element.from}`);
        return;
      }
      const toUid = to.uid;
      let isSticky = false;
      if (isMovingArrow && isSelected && this.selectionCenterOffset) {
        const validTarget = this.cachedElements.find((e: ViewElement) => {
          if (!(e.type === 'aux' || e.type === 'flow')) {
            return false;
          }
          return this.isValidTarget(e) || false;
        });
        if (validTarget) {
          isSticky = true;
          to = validTarget;
        } else {
          const off = this.selectionCenterOffset;
          const delta = this.state.moveDelta || { x: 0, y: 0 };
          to = to.merge({
            x: off.x - delta.x - this.state.canvasOffset.x,
            y: off.y - delta.y - this.state.canvasOffset.y,
            isZeroRadius: true,
          });
        }
      }
      if (isMovingArrow || this.isSelected(from) || this.isSelected(to)) {
        const oldTo = defined(this.elements.get(toUid));
        const oldFrom = defined(this.elements.get(from.uid));
        const oldθ = Math.atan2(oldTo.cy - oldFrom.cy, oldTo.cx - oldFrom.cx);
        const newθ = Math.atan2(to.cy - from.cy, to.cx - from.cx);
        const diffθ = oldθ - newθ;
        const angle = element.angle || 180;
        element = element.set('angle', angle + radToDeg(diffθ));
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
    };

    private getArcPoint(): XmilePoint | undefined {
      if (!this.selectionCenterOffset) {
        return undefined;
      }
      const off = defined(this.selectionCenterOffset);
      const delta = this.state.moveDelta || { x: 0, y: 0 };
      return new XmilePoint({
        x: off.x - delta.x - this.state.canvasOffset.x,
        y: off.y - delta.y - this.state.canvasOffset.y,
      });
    }

    private flow = (element: ViewElement) => {
      const variableErrors = this.props.model.vars.get(element.ident)?.errors.size || 0;
      const { isMovingArrow } = this.state;
      const isSelected = this.isSelected(element);
      const data = this.props.data.get(element.ident);

      if (!element.pts || element.pts.size < 2) {
        return;
      }

      const sourceId = defined(element.pts.get(0)).uid;
      if (!sourceId) {
        return;
      }
      const source = this.getElementByUid(sourceId);

      const sinkId = defined(element.pts.get(element.pts.size - 1)).uid;
      if (!sinkId) {
        return;
      }
      const sink = this.getElementByUid(sinkId);

      return (
        <Flow
          key={element.uid}
          element={element}
          series={data}
          source={source}
          sink={sink}
          isSelected={isSelected}
          hasWarning={variableErrors > 0}
          isMovingArrow={isSelected && isMovingArrow}
          isEditingName={isSelected && this.state.isEditingName}
          isValidTarget={this.isValidTarget(element)}
          onSelection={this.handleSetSelection}
          onLabelDrag={this.handleLabelDrag}
        />
      );
    };

    private constrainFlowMovement(flow: ViewElement, moveDelta: Point): [ViewElement, List<ViewElement>] {
      if (!flow.pts || flow.pts.size !== 2) {
        console.log('TODO: non-simple flow');
        return [flow, List()];
      }

      const sourceId = defined(defined(flow.pts.first()).uid);
      const source = this.getElementByUid(sourceId);

      const sinkId = defined(defined(flow.pts.last()).uid);
      let sink = this.getElementByUid(sinkId);

      const { isMovingArrow } = this.state;
      if (isMovingArrow && this.selectionCenterOffset) {
        const validTarget = this.cachedElements.find((e: ViewElement) => {
          // connecting both the inflow + outflow of a stock to itself wouldn't make sense.
          if (!(e.type === 'stock') || e.uid === sourceId) {
            return false;
          }
          return this.isValidTarget(e) || false;
        });
        if (validTarget) {
          moveDelta = {
            x: sink.cx - validTarget.cx,
            y: sink.cy - validTarget.cy,
          };
          sink = validTarget.merge({
            uid: sinkId,
            x: sink.cx,
            y: sink.cy,
            width: undefined,
            height: undefined,
          });
        } else {
          const off = this.selectionCenterOffset;
          sink = sink.merge({
            x: off.x - this.state.canvasOffset.x,
            y: off.y - this.state.canvasOffset.y,
            width: undefined,
            height: undefined,
            isZeroRadius: true,
          });
        }

        [sink, flow] = UpdateCloudAndFlow(sink, flow, moveDelta);
        return [flow, List([])];
      }

      const ends = List<ViewElement>([source, sink]);
      return UpdateFlow(flow, ends, moveDelta);
    }

    private constrainCloudMovement(cloudEl: ViewElement, moveDelta: Point): [ViewElement, ViewElement] {
      const flow = this.getElementByUid(defined(cloudEl.flowUid));
      return UpdateCloudAndFlow(cloudEl, flow, moveDelta);
    }

    private constrainStockMovement(stockEl: ViewElement, moveDelta: Point): [ViewElement, List<ViewElement>] {
      const stock = defined(this.props.model.vars.get(stockEl.ident)) as StockVar;
      const flowNames: List<string> = stock.inflows.concat(stock.outflows);
      const flows: List<ViewElement> = flowNames.map(ident => defined(this.getNamedElement(ident)));

      return UpdateStockAndFlows(stockEl, flows, moveDelta);
    }

    private populateNamedElements(displayElements: List<ViewElement>): void {
      if (!this.cachedElements.equals(displayElements)) {
        this.nameMap = Map(displayElements.filter(el => el.hasName).map(el => [el.ident, el.uid])).set(
          fauxTarget.ident,
          fauxTarget.uid,
        );
        this.elements = Map(displayElements.map(el => [el.uid, el])).set(fauxTarget.uid, fauxTarget);
        this.cachedElements = displayElements;
      }

      this.selectionUpdates = InnerCanvas.buildSelectionMap(this.props, this.elements, this.state.inCreation);
      if (this.state.labelSide) {
        this.selectionUpdates = this.selectionUpdates.map(el => {
          return el.set('labelSide', this.state.labelSide);
        });
      }
      if (this.state.moveDelta) {
        let otherUpdates = List<ViewElement>();
        const { x, y } = defined(this.state.moveDelta);
        this.selectionUpdates = this.selectionUpdates.map(initialEl => {
          // only constrain flow movement if we're not doing a group-move
          if (initialEl.type === 'flow' && this.selectionUpdates.size === 1) {
            const [flow, updatedClouds] = this.constrainFlowMovement(initialEl, defined(this.state.moveDelta));
            otherUpdates = otherUpdates.concat(updatedClouds);
            return flow;
          } else if (initialEl.type === 'stock' && this.selectionUpdates.size === 1) {
            const [stock, updatedFlows] = this.constrainStockMovement(initialEl, defined(this.state.moveDelta));
            otherUpdates = otherUpdates.concat(updatedFlows);
            return stock;
          } else if (initialEl.type === 'cloud' && this.selectionUpdates.size === 1) {
            const [cloud, updatedFlow] = this.constrainCloudMovement(initialEl, defined(this.state.moveDelta));
            otherUpdates = otherUpdates.push(updatedFlow);
            return cloud;
          } else if (initialEl.type !== 'connector') {
            return initialEl.merge({
              x: defined(initialEl.x) - x,
              y: defined(initialEl.y) - y,
            });
          } else {
            return initialEl;
          }
        });
        // now add flows that also were updated
        const namedUpdates: Map<UID, ViewElement> = otherUpdates.toMap().mapKeys((_, el) => el.uid);
        this.selectionUpdates = this.selectionUpdates.concat(namedUpdates);
      }
    }

    clearPointerState(clearSelection: boolean = true): void {
      this.pointerId = undefined;
      this.mouseDownPoint = undefined;
      this.selectionCenterOffset = undefined;
      this.prevCanvasOffset = undefined;

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
      if (!this.pointerId || this.pointerId !== e.pointerId) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();

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

          if (!this.state.isMovingArrow) {
            this.props.onMoveSelection(delta, arcPoint);
          } else {
            const element = this.getElementByUid(defined(this.props.selection.first()));
            let foundInvalidTarget = false;
            const validTarget = this.cachedElements.find((e: ViewElement) => {
              const isValid = this.isValidTarget(e);
              foundInvalidTarget = foundInvalidTarget || isValid === false;
              return isValid || false;
            });
            if (element.type === 'connector' && validTarget) {
              this.props.onAttachLink(element, validTarget.ident);
            } else if (element.type === 'flow') {
              this.props.onMoveFlow(element, validTarget ? validTarget.uid : 0, delta);
              if (this.state.inCreation) {
                this.setState({
                  isEditingName: true,
                  editingName: Plain.deserialize(displayName(defined(element.name))),
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

      if (!this.mouseDownPoint) {
        return;
      }

      const clearSelection = !this.state.isMovingCanvas;
      this.clearPointerState(clearSelection);
    };


    handleLabelDrag = (uid: number, e: React.PointerEvent<SVGElement>) => {
      this.pointerId = e.pointerId;

      const selectionSet = Set([uid]);
      if (!this.props.selection.equals(selectionSet)) {
        this.props.onSetSelection(selectionSet);
      }

      const element = this.getElementByUid(uid);
      const delta = this.state.moveDelta || { x: 0, y: 0 };
      const pointer = {
        x: delta.x + e.clientX,
        y: delta.y + e.clientY,
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

      const dx = this.selectionCenterOffset.x - e.clientX;
      const dy = this.selectionCenterOffset.y - e.clientY;

      this.setState({
        moveDelta: {
          x: dx,
          y: dy,
        } as (Point | undefined),
      });
    }

    handleMovingCanvas(e: React.PointerEvent<SVGElement>): void {
      if (!this.mouseDownPoint) {
        return;
      }

      const prev = this.prevCanvasOffset || { x: 0, y: 0 };

      this.setState({
        isMovingCanvas: true,
        canvasOffset: {
          x: prev.x + e.clientX - this.mouseDownPoint.x,
          y: prev.y + e.clientY - this.mouseDownPoint.y,
        },
      });
    }

    handleDragSelection(e: React.PointerEvent<SVGElement>): void {
      if (!this.mouseDownPoint) {
        return;
      }

      this.setState({
        isDragSelecting: true,
        dragSelectionPoint: {
          x: e.clientX,
          y: e.clientY,
        },
      });
    }

    handlePointerMove = (e: React.PointerEvent<SVGElement>): void => {
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

    handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
      if (!e.isPrimary) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();

      const { selectedTool } = this.props;
      if (selectedTool === 'aux' || selectedTool === 'stock') {
        const inCreation = new ViewElement({
          type: selectedTool,
          uid: inCreationUid,
          x: e.clientX - this.state.canvasOffset.x,
          y: e.clientY - this.state.canvasOffset.y,
          name: selectedTool === 'stock' ? 'New Stock' : 'New Variable',
          labelSide: selectedTool === 'aux' ? 'right' : undefined,
        });
        const editingName = Plain.deserialize(displayName(defined(inCreation.name)));
        this.setState({
          isEditingName: true,
          editingName,
          inCreation,
        });
        this.props.onSetSelection(Set([inCreation.uid]));
        return;
      }
      this.pointerId = e.pointerId;

      if (selectedTool === 'flow') {
        const { canvasOffset } = this.state;
        const x = e.clientX - canvasOffset.x;
        const y = e.clientY - canvasOffset.y;

        const inCreationCloud = new ViewElement({
          type: 'cloud',
          name: '$·model-internal-flow-creation-cloud',
          uid: inCreationCloudUid,
          x,
          y,
          flowUid: inCreationUid,
        });

        const inCreation = new ViewElement({
          type: 'flow',
          uid: inCreationUid,
          name: 'New Flow',
          x,
          y,
          pts: List([
            new XmilePoint({ x, y, uid: inCreationCloud.uid }),
            new XmilePoint({ x, y, uid: fauxTarget.uid }),
          ]),
        });

        this.selectionCenterOffset = {
          x: e.clientX,
          y: e.clientY,
        };

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
      this.mouseDownPoint = {
        x: e.clientX,
        y: e.clientY,
      };
      if (this.state.canvasOffset) {
        this.prevCanvasOffset = this.state.canvasOffset;
      }

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
      let isEditingName = !!isText;
      let editingName: Value | undefined;
      let isMovingArrow = !!isArrowhead;

      this.pointerId = e.pointerId;
      this.selectionCenterOffset = {
        x: e.clientX,
        y: e.clientY,
      };

      (e.target as any).setPointerCapture(e.pointerId);

      const { selectedTool } = this.props;
      let inCreation: ViewElement | undefined;

      if (selectedTool === 'link' && (element.type === 'aux' || element.type === 'flow' || element.type === 'stock')) {
        isEditingName = false;
        isMovingArrow = true;
        const fromName = element.ident;
        inCreation = new ViewElement({
          type: 'connector',
          uid: inCreationUid,
          from: fromName,
          to: fauxTarget.ident,
        });
        element = inCreation;
      } else if (selectedTool === 'flow' && element.type === 'stock') {
        isEditingName = false;
        isMovingArrow = true;
        inCreation = new ViewElement({
          type: 'flow',
          uid: inCreationUid,
          name: 'New Flow',
          x: element.cx,
          y: element.cy,
          pts: List([
            new XmilePoint({ x: element.cx, y: element.cy, uid: element.uid }),
            new XmilePoint({ x: element.cx, y: element.cy, uid: fauxTarget.uid }),
          ]),
        });
        element = inCreation;
      } else {
        // not an action we recognize, deselect th tool and continue on
        this.props.onClearSelectedTool();

        // single-element selection only for now
        const selection = Set([element.uid]);

        if (isEditingName) {
          const uid = defined(selection.first());
          const editingElement = this.getElementByUid(uid);
          editingName = Plain.deserialize(displayName(defined(editingElement.name)));
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

    handleEditingNameChange = (change: { operations: List<Operation>; value: Value }): any => {
      this.setState({ editingName: change.value });
    };

    handleEditingNameDone = (isCancel: boolean) => {
      if (!this.state.isEditingName) {
        return;
      }

      if (isCancel) {
        this.clearPointerState();
        return;
      }

      const uid = defined(this.props.selection.first());
      const element = this.getElementByUid(uid);
      const oldName = displayName(defined(element.name));
      const newName = Plain.serialize(defined(this.state.editingName));

      if (uid === inCreationUid) {
        this.props.onCreateVariable(element.set('name', newName));
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

    render() {
      const { view, embedded, classes } = this.props;

      if (!this.props.selection.equals(this.selection)) {
        this.selection = this.props.selection;
      }

      // filter all the elements in this XMILE view down to just the ones
      // we know how to display.
      let displayElements = view.elements.filter(e => KnownTypes.has(e.type));
      if (this.state.inCreation) {
        displayElements = displayElements.push(this.state.inCreation);
      }
      if (this.state.inCreationCloud) {
        displayElements = displayElements.push(this.state.inCreationCloud);
      }

      // create different layers for each of the display types so that views compose together nicely
      const zLayers: React.ReactElement[][] = new Array(ZMax);
      for (let i = 0; i < ZMax; i++) {
        zLayers[i] = [];
      }

      // phase 1: build up a map of ident -> ViewElement
      this.populateNamedElements(displayElements);

      // FIXME: this is so gross
      this.elementBounds = List();

      // phase 3: create React components and add them to the appropriate layer
      for (let element of displayElements) {
        if (!this[element.type]) {
          continue;
        }

        if (this.selectionUpdates.has(element.uid)) {
          element = defined(this.selectionUpdates.get(element.uid));
        }

        if (!this.builder.hasOwnProperty(element.type)) {
          continue;
        }

        const component: React.ReactElement = this.builder[element.type](element);
        zLayers[defined(ZOrder.get(element.type))].push(component);
      }

      let overlayClass = classes.overlay;
      let nameEditor;

      let dragRect;
      if (this.state.isDragSelecting && this.mouseDownPoint && this.state.dragSelectionPoint) {
        const pointA = this.mouseDownPoint;
        const pointB = this.state.dragSelectionPoint;
        const offset = this.state.canvasOffset;

        const x = Math.min(pointA.x, pointB.x) - offset.x;
        const y = Math.min(pointA.y, pointB.y) - offset.y;
        const w = Math.abs(pointA.x - pointB.x);
        const h = Math.abs(pointA.y - pointB.y);

        dragRect = <rect className={classes.selectionOverlay} x={x} y={y} width={w} height={h} />;
      }

      if (!this.state.isEditingName) {
        overlayClass += ' ' + classes.noPointerEvents;
      } else {
        const editingUid = defined(this.props.selection.first());
        const editingElement = this.getElementByUid(editingUid);
        const defaultSide = editingElement.type === 'stock' ? 'top' : 'bottom';
        const rw = editingElement.type === 'stock' ? 45 / 2 : AuxRadius;
        const rh = editingElement.type === 'stock' ? 35 / 2 : AuxRadius;
        const side = findSide(editingElement, defaultSide);
        const offset = this.state.canvasOffset;
        nameEditor = (
          <EditableLabel
            uid={editingUid}
            cx={editingElement.cx + offset.x}
            cy={editingElement.cy + offset.y}
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
      } else if (this.state.canvasOffset.x !== 0 || this.state.canvasOffset.y !== 0) {
        const offset = this.state.canvasOffset;
        transform = `translate(${offset.x} ${offset.y})`;
      }

      const overlay = embedded ? (
        undefined
      ) : (
        <div className={overlayClass} onPointerDown={this.handleEditingEnd}>
          {nameEditor}
        </div>
      );

      // we don't need these things anymore
      this.elementBounds = List();
      this.selectionUpdates = Map();
      // n.b. we don't want to clear this.elements or this.nameMap, as thats used when handling callbacks

      return (
        <div className={classes.container}>
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
            <g transform={transform} className={classes.gLayer}>
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
