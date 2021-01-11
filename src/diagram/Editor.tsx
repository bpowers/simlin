// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { toUint8Array, fromUint8Array } from 'js-base64';

import { List, Map, Set, Stack } from 'immutable';

import { History } from 'history';

import { Canvg } from 'canvg';

import type { Engine as IEngine } from '@system-dynamics/engine';
import { open, errorCodeDescription } from '@system-dynamics/engine';

import {
  Project,
  Model,
  Variable,
  UID,
  Stock as StockVar,
  ViewElement,
  NamedViewElement,
  StockFlowView,
  GraphicalFunction,
  LinkViewElement,
  AuxViewElement,
  FlowViewElement,
  StockViewElement,
  CloudViewElement,
  viewElementType,
} from '@system-dynamics/core/datamodel';

import { baseURL, defined, exists, Series, toInt, uint8ArraysEqual } from '@system-dynamics/core/common';

import { Canvas, fauxCloudTargetUid, inCreationCloudUid, inCreationUid } from './drawing/Canvas';
import { Point } from './drawing/common';
import { takeoffθ } from './drawing/Connector';
import { UpdateCloudAndFlow, UpdateFlow, UpdateStockAndFlows } from './drawing/Flow';

import IconButton from '@material-ui/core/IconButton';
import Input from '@material-ui/core/Input';
import Paper from '@material-ui/core/Paper';
import Snackbar from '@material-ui/core/Snackbar';

import ClearIcon from '@material-ui/icons/Clear';
import EditIcon from '@material-ui/icons/Edit';
import MenuIcon from '@material-ui/icons/Menu';
import SearchIcon from '@material-ui/icons/Search';

import SpeedDial, { CloseReason } from '@material-ui/core/SpeedDial';
import SpeedDialAction from '@material-ui/core/SpeedDialAction';
import SpeedDialIcon from '@material-ui/core/SpeedDialIcon';

import { AuxIcon } from './AuxIcon';
import { Toast } from './ErrorToast';
import { FlowIcon } from './FlowIcon';
import { LinkIcon } from './LinkIcon';
import { ModelPropertiesDrawer } from './ModelPropertiesDrawer';
import { Snapshotter } from './Snapshotter';
import { Status } from './Status';
import { StockIcon } from './StockIcon';
import { UndoRedoBar } from './UndoRedoBar';
import { VariableDetails } from './VariableDetails';

import { createStyles, Theme } from '@material-ui/core/styles';
import withStyles, { WithStyles } from '@material-ui/core/styles/withStyles';
import { renderSvgToString } from './render-common';

import { Card } from '@material-ui/core';
import Button from '@material-ui/core/Button';
import CardActions from '@material-ui/core/CardActions';
import CardContent from '@material-ui/core/CardContent';
import { canonicalize } from '@system-dynamics/core/canonicalize';

const MaxUndoSize = 5;

function radToDeg(r: number): number {
  return (r * 180) / Math.PI;
}

const styles = ({ spacing, palette }: Theme) =>
  createStyles({
    root: {},
    undoRedoBar: {
      display: 'flex',
      position: 'absolute',
      bottom: spacing(3.5),
      right: spacing(12),
    },
    speedDial: {
      position: 'absolute',
      bottom: spacing(2),
      right: spacing(3),
    },
    snapshotCard: {
      position: 'absolute',
      top: 0,
      right: 0,
      width: 240,
      marginTop: 12,
      marginRight: 12,
    },
    snapshotImg: {
      width: '100%',
      objectFit: 'scale-down',
    },
    searchbox: {
      position: 'relative',
      top: 0,
      left: 0,
      paddingLeft: 64,
      paddingTop: 12,
      paddingBottom: 12,
      paddingRight: 70,
      height: '100%',
      width: '100%',
    },
    menuButton: {
      marginLeft: 4,
      position: 'absolute',
      zIndex: 100,
      left: 0,
      top: 0,
      display: 'block',
      color: '#666',
    },
    paper: {
      position: 'absolute',
      top: 8,
      left: 8,
      height: 48,
      width: 359, // 392
    },
    varDetails: {
      position: 'absolute',
      top: 8,
      left: 8,
    },
    searchButton: {
      color: '#aaa',
    },
    divider: {
      position: 'absolute',
      top: 0,
      right: 0,
      height: 28,
      marginTop: 10,
      marginRight: 54,
      borderLeftWidth: 1,
      borderLeftStyle: 'solid',
      borderColor: '#ddd',
    },
    editor: {
      boxSizing: 'border-box',
      overflow: 'hidden',
    },
    editorBg: {
      background: '#f2f2f2',
      // background: '#fffff8',
      width: '100%',
      height: '100%',
      position: 'fixed',
    },
    selectedTool: {
      backgroundColor: palette.secondary.main,
    },
  });

class ModelError implements Error {
  name = 'ModelError';
  message: string;
  constructor(msg: string) {
    this.message = msg;
  }
}

interface EditorState {
  modelErrors: List<Error>;
  activeProject?: Project;
  projectHistory: Stack<Readonly<Uint8Array>>;
  projectOffset: number;
  modelName: string;
  dialOpen: boolean;
  dialVisible: boolean;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;
  data: Map<string, Series>;
  selection: Set<UID>;
  drawerOpen: boolean;
  projectVersion: number;
  snapshotBlob: Blob | undefined;
  variableDetailsActiveTab: number;
}

interface EditorProps extends WithStyles<typeof styles> {
  username: string;
  projectName: string;
  embedded?: boolean;
  baseURL?: string;
  history?: History;
}

export const Editor = withStyles(styles)(
  class InnerEditor extends React.PureComponent<EditorProps, EditorState> {
    private activeEngine?: IEngine;

    constructor(props: EditorProps) {
      super(props);

      this.state = {
        projectHistory: Stack<Readonly<Uint8Array>>(),
        projectOffset: 0,
        modelErrors: List<Error>(),
        modelName: 'main',
        dialOpen: false,
        dialVisible: true,
        selectedTool: undefined,
        data: Map(),
        selection: Set<number>(),
        drawerOpen: false,
        projectVersion: -1,
        snapshotBlob: undefined,
        variableDetailsActiveTab: 0,
      };

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(async () => {
        const project = await this.loadModel();
        if (!project) {
          return;
        }

        this.scheduleSimRun();
      });
    }

    private project(): Project | undefined {
      return this.state.activeProject;
    }

    private engine(): IEngine | undefined {
      return this.activeEngine;
    }

    private scheduleSimRun(): void {
      setTimeout(() => {
        const engine = this.engine();
        if (!engine) {
          return;
        }
        this.loadSim(engine);
      });
    }

    private loadSim(engine: IEngine) {
      if (!engine.isSimulatable()) {
        return;
      }
      try {
        engine.simRunToEnd();
        const idents = engine.simVarNames() as string[];
        const time = engine.simSeries('time');
        const data = Map<string, Series>(
          idents.map((ident) => [ident, { name: ident, time, values: engine.simSeries(ident) }]),
        );
        setTimeout(() => {
          engine.simClose();
        });
        this.setState({ data });
      } catch (e) {
        this.setState({
          modelErrors: this.state.modelErrors.push(e),
        });
      }
    }

    private updateProject(serializedProject: Readonly<Uint8Array>, scheduleSave = true) {
      if (this.state.projectHistory.size > 0) {
        const current = this.state.projectHistory.get(this.state.projectOffset);
        if (uint8ArraysEqual(serializedProject, current)) {
          return;
        }
      }

      const activeProject = this.updateVariableErrors(Project.deserializeBinary(serializedProject));

      const priorHistory = this.state.projectHistory.slice();

      // fractionally increase the version -- the server will only send back integer versions,
      // but this will ensure we can use a simple version check in the Canvas to invalidate caches.
      const projectVersion = this.state.projectVersion + 0.01;

      this.setState({
        projectHistory: priorHistory.unshift(serializedProject).slice(0, MaxUndoSize),
        activeProject,
        projectVersion,
        projectOffset: 0,
      });
      if (scheduleSave) {
        this.scheduleSave(serializedProject);
      }
    }

    private scheduleSave(project: Readonly<Uint8Array>): void {
      const { projectVersion } = this.state;
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(async () => {
        await this.save(project, toInt(projectVersion));
      });
    }

    private async save(project: Readonly<Uint8Array>, currVersion: number): Promise<void> {
      const bodyContents = {
        currVersion,
        projectPB: fromUint8Array(project as Uint8Array),
      };

      const base = this.getBaseURL();
      const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
      const response = await fetch(apiPath, {
        credentials: 'same-origin',
        method: 'POST',
        cache: 'no-cache',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(bodyContents),
      });

      const status = response.status;
      if (!(status >= 200 && status < 400)) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
        const body = await response.json();
        const errorMsg =
          body && body.error ? (body.error as string) : `HTTP ${status}; maybe try a different username ¯\\_(ツ)_/¯`;
        this.appendModelError(errorMsg);
        return;
      }

      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const projectResponse = await response.json();
      const projectVersion = defined(projectResponse.version) as number;

      this.setState({ projectVersion });
    }

    private getBaseURL(): string {
      return this.props.baseURL !== undefined ? this.props.baseURL : baseURL;
    }

    private appendModelError(msg: string) {
      this.setState((prevState: EditorState) => ({
        modelErrors: prevState.modelErrors.push(new ModelError(msg)),
      }));
    }

    private async loadModel(): Promise<Project | undefined> {
      const base = this.getBaseURL();
      const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
      const response = await fetch(apiPath);
      if (response.status >= 400) {
        this.appendModelError(`unable to load ${apiPath}`);
        return;
      }

      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const projectResponse = await response.json();

      const projectBinary = toUint8Array(projectResponse.pb);
      const project = Project.deserializeBinary(projectBinary);

      // we don't call updateProject here because we don't want to
      // POST a new version up when we've just downloaded it.
      this.setState({
        activeProject: project,
        projectVersion: defined(projectResponse.version) as number,
        projectHistory: Stack([projectBinary]),
        projectOffset: 0,
      });

      await this.openEngine(projectBinary, project);

      return project;
    }

    handleDialClick = (_event: React.MouseEvent<HTMLDivElement, MouseEvent>) => {
      this.setState({
        dialOpen: !this.state.dialOpen,
        selectedTool: this.state.dialOpen ? undefined : this.state.selectedTool,
      });
    };

    // eslint-disable-next-line @typescript-eslint/ban-types
    handleDialClose = (e: React.SyntheticEvent<{}>, reason: CloseReason) => {
      if (reason === 'mouseLeave' || reason === 'blur') {
        return;
      }
      this.setState({
        dialOpen: false,
        selectedTool: undefined,
      });
    };

    handleRename = (oldName: string, newName: string) => {
      if (oldName === newName) {
        return;
      }

      const engine = defined(this.engine());
      let err = engine.rename(this.state.modelName, oldName, newName);
      if (err) {
        const details = err.getDetails();
        const msg = `${errorCodeDescription(err.code)}` + (details ? `: ${details}` : '');
        this.appendModelError(msg);
        return;
      }
      const view = defined(this.getView());

      const oldIdent = canonicalize(oldName);
      newName = newName.replace('\n', '\\n');

      const elements = view.elements.map((element: ViewElement) => {
        if (!element.isNamed()) {
          return element;
        }
        const namedElement = element as AuxViewElement;
        if (namedElement.ident() !== oldIdent) {
          return element;
        }

        return namedElement.set('name', newName);
      });

      const viewPb = view.set('elements', elements).toPb();
      const serializedView = viewPb.serializeBinary();
      err = engine.setView(this.state.modelName, 0, serializedView);
      if (err) {
        const details = err.getDetails();
        const msg = `${errorCodeDescription(err.code)}` + (details ? `: ${details}` : '');
        this.appendModelError(msg);
        return;
      }

      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
    };

    handleSelection = (selection: Set<UID>) => {
      this.setState({
        selection,
        variableDetailsActiveTab: 0,
      });
    };

    handleSelectionDelete = () => {
      const selection = this.state.selection;
      const { modelName } = this.state;
      const view = defined(this.getView());

      // this will remove the selected elements, clouds, and connectors
      let elements = view.elements.filter((element: ViewElement) => {
        const remove =
          selection.contains(element.uid) ||
          (element instanceof CloudViewElement && selection.contains(element.flowUid)) ||
          (element instanceof LinkViewElement &&
            (selection.contains(element.toUid) || selection.contains(element.fromUid)));
        return !remove;
      });

      // next we have to potentially make new clouds if we've deleted a stock
      let { nextUid } = view;
      const clouds: CloudViewElement[] = [];
      elements = elements.map((element: ViewElement) => {
        if (!(element instanceof FlowViewElement)) {
          return element;
        }
        const points = element.points.map((pt) => {
          if (!pt.attachedToUid || !selection.contains(pt.attachedToUid)) {
            return pt;
          }

          const cloud = new CloudViewElement({
            uid: nextUid++,
            x: pt.x,
            y: pt.y,
            flowUid: element.uid,
            isZeroRadius: false,
          });

          clouds.push(cloud);

          return pt.set('attachedToUid', cloud.uid);
        });
        element = element.set('points', points);
        return element;
      });
      elements = elements.concat(clouds);

      const engine = defined(this.engine());
      for (const ident of this.getSelectionIdents()) {
        engine.deleteVariable(modelName, ident);
      }
      // this will ensure that deletions the engine does above are also serialized to the state
      this.updateView(view.merge({ elements, nextUid }));
      this.setState({
        selection: Set<number>(),
      });
      this.scheduleSimRun();
    };

    handleMoveLabel = (uid: UID, side: 'top' | 'left' | 'bottom' | 'right') => {
      const view = defined(this.getView());

      const elements = view.elements.map((element: ViewElement) => {
        if (element.uid !== uid || !element.isNamed()) {
          return element;
        }
        return (element as AuxViewElement).set('labelSide', side);
      });

      this.updateView(view.set('elements', elements));
    };

    handleFlowAttach = (flow: FlowViewElement, targetUid: number, cursorMoveDelta: Point) => {
      let { selection } = this.state;
      const view = defined(this.getView());

      let isCreatingNew = false;
      let stockDetachingIdent: string | undefined;
      let stockAttachingIdent: string | undefined;
      let sourceStockIdent: string | undefined;
      let uidToDelete: number | undefined;
      let updatedCloud: ViewElement | undefined;
      let newClouds = List<ViewElement>();

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
        if (!(element instanceof FlowViewElement)) {
          return element;
        }

        const oldTo = getUid(defined(defined(element.points.last()).attachedToUid));
        let newCloud = false;
        let updateCloud = false;
        let to: StockViewElement | CloudViewElement;
        if (targetUid) {
          if (oldTo instanceof CloudViewElement) {
            uidToDelete = oldTo.uid;
          }
          const newTarget = getUid(targetUid);
          if (!(newTarget instanceof StockViewElement || newTarget instanceof CloudViewElement)) {
            throw new Error(`new target isn't a stock or cloud (uid ${newTarget.uid})`);
          }
          to = newTarget;
        } else if (oldTo instanceof CloudViewElement) {
          updateCloud = true;
          to = oldTo.merge({
            x: oldTo.cx - cursorMoveDelta.x,
            y: oldTo.cy - cursorMoveDelta.y,
          });
        } else {
          newCloud = true;
          to = new CloudViewElement({
            uid: nextUid++,
            x: oldTo.cx - cursorMoveDelta.x,
            y: oldTo.cy - cursorMoveDelta.y,
            flowUid: flow.uid,
            isZeroRadius: false,
          });
        }

        if (oldTo.uid !== to.uid) {
          if (oldTo instanceof StockViewElement) {
            stockDetachingIdent = oldTo.ident();
          }
          if (to instanceof StockViewElement) {
            stockAttachingIdent = to.ident();
          }
        }

        const moveDelta = {
          x: oldTo.cx - to.cx,
          y: oldTo.cy - to.cy,
        };
        const points = element.points.map((point) => {
          if (point.attachedToUid !== oldTo.uid) {
            return point;
          }
          return point.set('attachedToUid', to.uid);
        });
        to = (to as StockViewElement).merge({
          x: oldTo.cx,
          y: oldTo.cy,
        });
        element = element.set('points', points);

        [to, element] = UpdateCloudAndFlow(to, element as FlowViewElement, moveDelta);
        if (newCloud) {
          newClouds = newClouds.push(to);
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
        flow = flow.merge({
          uid: nextUid++,
        });
        const firstPt = defined(flow.points.first());
        const sourceUid = firstPt.attachedToUid;
        if (sourceUid === inCreationCloudUid) {
          const newCloud = new CloudViewElement({
            uid: nextUid++,
            x: firstPt.x,
            y: firstPt.y,
            flowUid: flow.uid,
            isZeroRadius: false,
          });
          elements = elements.push(newCloud);
          flow = flow.set(
            'points',
            flow.points.map((pt) => {
              if (pt.attachedToUid === inCreationCloudUid) {
                return pt.set('attachedToUid', newCloud.uid);
              }
              return pt;
            }),
          );
        } else if (sourceUid) {
          const sourceStock = getUid(sourceUid);
          sourceStockIdent = defined(sourceStock.ident());
        }
        const lastPt = defined(flow.points.last());
        if (lastPt.attachedToUid === fauxCloudTargetUid) {
          let newCloud = false;
          let to: StockViewElement | CloudViewElement;
          if (targetUid) {
            to = getUid(targetUid) as StockViewElement | CloudViewElement;
            stockAttachingIdent = defined(to.ident());
            cursorMoveDelta = {
              x: 0,
              y: 0,
            };
          } else {
            to = new CloudViewElement({
              uid: nextUid++,
              x: lastPt.x,
              y: lastPt.y,
              flowUid: flow.uid,
              isZeroRadius: false,
            });
            newCloud = true;
          }
          flow = flow.set(
            'points',
            flow.points.map((pt) => {
              if (pt.attachedToUid === fauxCloudTargetUid) {
                return pt.set('attachedToUid', to.uid);
              }
              return pt;
            }),
          );
          [to, flow] = UpdateCloudAndFlow(to, flow, cursorMoveDelta);
          if (newCloud) {
            elements = elements.push(to);
          }
        }
        elements = elements.push(flow);
        selection = Set([flow.uid]);
        isCreatingNew = true;
      }
      elements = elements.concat(newClouds);

      const engine = defined(this.engine());
      if (isCreatingNew) {
        engine.addNewVariable(this.state.modelName, 'flow', (flow as NamedViewElement).name);
        if (sourceStockIdent) {
          engine.addStocksFlow(this.state.modelName, sourceStockIdent, flow.ident(), 'out');
        }
      }
      if (stockAttachingIdent) {
        engine.addStocksFlow(this.state.modelName, stockAttachingIdent, flow.ident(), 'in');
      }
      if (stockDetachingIdent) {
        engine.removeStocksFlow(this.state.modelName, stockDetachingIdent, flow.ident(), 'in');
      }
      this.updateView(view.merge({ nextUid, elements }));
      this.setState({ selection });
      this.scheduleSimRun();
    };

    handleLinkAttach = (link: LinkViewElement, newTarget: string) => {
      let { selection } = this.state;
      let view = defined(this.getView());

      const getUid = (uid: number) => {
        for (const e of view.elements) {
          if (e.uid === uid) {
            return e;
          }
        }
        throw new Error(`unknown uid ${uid}`);
      };

      const getName = (ident: string) => {
        for (const e of view.elements) {
          if (e.isNamed() && e.ident() === ident) {
            return e;
          }
        }
        throw new Error(`unknown name ${ident}`);
      };

      let elements = view.elements.map((element: ViewElement) => {
        if (element.uid !== link.uid) {
          return element;
        }

        if (!(element instanceof LinkViewElement)) {
          return element;
        }

        const from = getUid(element.fromUid);
        const oldTo = getUid(element.toUid);
        const to = getName(defined(newTarget));

        const oldθ = Math.atan2(oldTo.cy - from.cy, oldTo.cx - from.cx);
        const newθ = Math.atan2(to.cy - from.cy, to.cx - from.cx);
        const diffθ = oldθ - newθ;
        const angle = (element.arc || 180) - radToDeg(diffθ);

        return element.merge({
          arc: angle,
          toUid: to.uid,
        });
      });
      let nextUid = view.nextUid;
      if (link.uid === inCreationUid) {
        const from = getUid(link.fromUid);
        const to = getName(newTarget);

        const oldθ = Math.atan2(0 - from.cy, 0 - from.cx);
        const newθ = Math.atan2(to.cy - from.cy, to.cx - from.cx);
        const diffθ = oldθ - newθ;
        const angle = (link.arc || 180) - radToDeg(diffθ);

        const newLink = link.merge({
          uid: nextUid++,
          toUid: to.uid,
          arc: angle,
        });
        elements = elements.push(newLink);
        selection = Set([newLink.uid]);
      }
      view = view.merge({ nextUid, elements });

      this.updateView(view);
      this.setState({ selection });
    };

    updateView(view: StockFlowView) {
      const viewPb = view.toPb();
      const serializedView = viewPb.serializeBinary();
      const engine = this.engine();
      if (engine) {
        const err = engine.setView(this.state.modelName, 0, serializedView);
        if (err) {
          const details = err.getDetails();
          const msg = `${errorCodeDescription(err.code)}` + (details ? `: ${details}` : '');
          this.appendModelError(msg);
          return;
        }
        this.updateProject(engine.serializeToProtobuf());
      }
    }

    handleCreateVariable = (element: ViewElement) => {
      const view = defined(this.getView());

      let nextUid = view.nextUid;
      const elements = view.elements.push(element.set('uid', nextUid++));

      this.engine()?.addNewVariable(this.state.modelName, viewElementType(element), (element as NamedViewElement).name);
      this.updateView(view.merge({ nextUid, elements }));
      this.setState({
        selection: Set<number>(),
      });
    };

    handleSelectionMove = (delta: Point, arcPoint?: Point) => {
      const view = defined(this.getView());
      const origElements = view.elements;
      const origNamedElements = Map<string, ViewElement>(
        origElements.filter((e) => e.isNamed()).map((e) => [defined(e.ident()), e]),
      );
      const selection = this.state.selection;

      const getName = (ident: string) => {
        for (const e of view.elements) {
          if (e.isNamed() && e.ident() === ident) {
            return e;
          }
        }
        throw new Error(`unknown name ${ident}`);
      };
      const getUid = (uid: UID) => {
        for (const e of view.elements) {
          if (e.uid === uid) {
            return e;
          }
        }
        throw new Error(`unknown UID ${uid}`);
      };

      let updatedElements = List<ViewElement>();

      let elements = view.elements.map((element: ViewElement) => {
        if (!selection.has(element.uid)) {
          return element;
        }

        if (selection.size === 1 && element instanceof FlowViewElement) {
          const pts = element.points;
          const sourceId = defined(defined(pts.first()).attachedToUid);
          const source = getUid(sourceId) as StockViewElement | CloudViewElement;

          const sinkId = defined(defined(pts.last()).attachedToUid);
          const sink = getUid(sinkId) as StockViewElement | CloudViewElement;

          const ends = List<StockViewElement | CloudViewElement>([source, sink]);
          const [newElement, newUpdatedClouds] = UpdateFlow(element, ends, delta);
          element = newElement;
          updatedElements = updatedElements.concat(newUpdatedClouds);
        } else if (selection.size === 1 && element instanceof CloudViewElement) {
          const flow = defined(getUid(defined(element.flowUid))) as FlowViewElement;
          const [newCloud, newUpdatedFlow] = UpdateCloudAndFlow(element, flow, delta);
          element = newCloud;
          updatedElements = updatedElements.push(newUpdatedFlow);
        } else if (selection.size === 1 && element instanceof StockViewElement) {
          const stock = defined(defined(this.getModel()).variables.get(element.ident())) as StockVar;
          const flowNames: List<string> = stock.inflows.concat(stock.outflows);
          const flows: List<ViewElement> = flowNames.map(getName);
          const [newElement, newUpdatedFlows] = UpdateStockAndFlows(element, flows as List<FlowViewElement>, delta);
          element = newElement;
          updatedElements = updatedElements.concat(newUpdatedFlows);
        } else if (element instanceof LinkViewElement) {
          const from = getUid(element.fromUid);
          const to = getUid(element.toUid);
          const newTakeoffθ = takeoffθ({ element, from, to, arcPoint: defined(arcPoint) });
          const newTakeoff = radToDeg(newTakeoffθ);
          element = element.merge({
            arc: newTakeoff,
          });
        } else {
          // everything else has an x and a y, the cast is to make typescript
          // happy with our dumb type decisions
          element = (element as AuxViewElement).merge({
            x: element.cx - delta.x,
            y: element.cy - delta.y,
          });
        }
        return element;
      });

      const updatedFlowsByUid: Map<UID, ViewElement> = updatedElements.toMap().mapKeys((_, e) => e.uid);
      elements = elements.map((element) => {
        if (updatedFlowsByUid.has(element.uid)) {
          return defined(updatedFlowsByUid.get(element.uid));
        }
        return element;
      });

      let namedElements = Map<string, ViewElement>();
      let selectedElements = Map<string, ViewElement>();
      for (const e of elements) {
        if (!e.isNamed()) {
          continue;
        }
        const ident = defined(e.ident());
        if (selection.has(e.uid)) {
          selectedElements = selectedElements.set(ident, e);
        }
        namedElements = namedElements.set(ident, selectedElements.get(ident, e));
      }

      elements = elements.map((element: ViewElement) => {
        if (!(element instanceof LinkViewElement)) {
          return element.isNamed() ? defined(namedElements.get(defined(element.ident()))) : element;
        }
        const fromName = defined(getUid(element.fromUid).ident());
        const toName = defined(getUid(element.toUid).ident());
        // if it hasn't been updated, nothing to do
        if (!(selectedElements.has(fromName) || selectedElements.has(toName))) {
          return element;
        }
        const from = selectedElements.get(fromName) || namedElements.get(fromName);
        if (!from) {
          return element;
        }
        const to = selectedElements.get(toName) || namedElements.get(toName);
        if (!to) {
          return element;
        }
        const atan2 = Math.atan2;
        const oldTo = defined(origNamedElements.get(toName));
        const oldFrom = defined(origNamedElements.get(fromName));
        const oldθ = atan2(oldTo.cy - oldFrom.cy, oldTo.cx - oldFrom.cx);
        const newθ = atan2(to.cy - from.cy, to.cx - from.cx);
        const diffθ = oldθ - newθ;

        return element.update('arc', (angle) => {
          return defined(angle) - radToDeg(diffθ);
        });
      });
      this.updateView(view.merge({ elements }));
    };

    handleDrawerToggle = (isOpen: boolean) => {
      this.setState({
        drawerOpen: isOpen,
      });
    };

    handleStartTimeChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const engine = this.engine();
      if (!engine) {
        return;
      }
      const value = Number(event.target.value);
      engine.setSimSpecStart(value);
      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
    };

    handleStopTimeChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const engine = this.engine();
      if (!engine) {
        return;
      }
      const value = Number(event.target.value);
      engine.setSimSpecStop(value);
      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
    };

    handleDtChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const engine = this.engine();
      if (!engine) {
        return;
      }
      const value = Number(event.target.value);
      engine.setSimSpecDt(value, false);
      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
    };

    handleTimeUnitsChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const engine = this.engine();
      if (!engine) {
        return;
      }
      const value = event.target.value;
      engine.setSimSpecTimeUnits(value);
      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
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

      return (
        <ModelPropertiesDrawer
          modelName={this.props.projectName}
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

      return model.views.first();
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

      const onRenameVariable = !embedded ? this.handleRename : (_oldName: string, _newName: string): void => {};
      const onSetSelection = !embedded ? this.handleSelection : (_selected: Set<UID>): void => {};
      const onMoveSelection = !embedded ? this.handleSelectionMove : (_position: Point): void => {};
      const onMoveFlow = !embedded ? this.handleFlowAttach : (_e: ViewElement, _t: number, _p: Point): void => {};
      const onMoveLabel = !embedded
        ? this.handleMoveLabel
        : (_u: UID, _s: 'top' | 'left' | 'bottom' | 'right'): void => {};
      const onAttachLink = !embedded ? this.handleLinkAttach : (_element: ViewElement, _to: string): void => {};
      const onCreateVariable = !embedded ? this.handleCreateVariable : (_element: ViewElement): void => {};
      const onClearSelectedTool = !embedded ? this.handleClearSelectedTool : () => {};
      const onDeleteSelection = !embedded ? this.handleSelectionDelete : () => {};

      return (
        <Canvas
          embedded={!!embedded}
          project={project}
          model={model}
          view={view}
          version={this.state.projectVersion}
          data={this.state.data}
          selectedTool={this.state.selectedTool}
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
            horizontal: 'left',
          }}
          open={this.state.modelErrors.size > 0}
          autoHideDuration={6000}
        >
          <div>
            {this.state.modelErrors.map((err, i) => (
              <Toast variant="warning" onClose={this.handleCloseSnackbar} message={err.message} key={i} />
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
        if (selection.contains(e.uid) && e.isNamed()) {
          names.push(defined(e.ident()));
        }
      }

      return names;
    }

    // FIXME: use a map
    getNamedSelectedElement(): ViewElement | undefined {
      if (this.state.selection.size !== 1) {
        return;
      }

      const uid = defined(this.state.selection.first());

      const view = this.getView();
      if (!view) {
        return;
      }

      for (const e of view.elements) {
        if (e.uid === uid && e.isNamed()) {
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

    getSearchBar() {
      const { embedded } = this.props;
      const classes = this.props.classes;

      if (embedded) {
        return undefined;
      }

      const namedElement = this.getNamedSelectedElement();
      let name;
      let placeholder: string | undefined = 'Find in Model';
      if (namedElement) {
        name = defined((namedElement as NamedViewElement).name).replace('\\n', ' ');
        placeholder = undefined;
      }

      const engine = this.engine();
      const status = !engine || engine.isSimulatable() ? 'ok' : 'error';

      return (
        <Paper className={classes.paper} elevation={2}>
          <IconButton className={classes.menuButton} color="inherit" aria-label="Menu" onClick={this.handleShowDrawer}>
            <MenuIcon />
          </IconButton>
          <Input
            key={name}
            className={classes.searchbox}
            disableUnderline={true}
            placeholder={placeholder}
            inputProps={{
              'aria-label': 'Description',
            }}
            defaultValue={name}
            endAdornment={name ? undefined : <SearchIcon className={classes.searchButton} />}
          />
          <div className={classes.divider} />
          <Status status={status} />
        </Paper>
      );
    }

    handleEquationChange = (ident: string, newEquation: string) => {
      const engine = this.engine();
      if (!engine) {
        return;
      }
      engine.setEquation(this.state.modelName, ident, newEquation);
      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
    };

    handleTableChange = (ident: string, newTable: GraphicalFunction | null) => {
      const engine = defined(this.engine());
      if (newTable) {
        const gf = newTable.toPb();
        engine.setGraphicalFunction(this.state.modelName, ident, gf.serializeBinary());
      } else {
        engine.removeGraphicalFunction(this.state.modelName, ident);
      }
      this.updateProject(engine.serializeToProtobuf());
      this.scheduleSimRun();
    };

    getVariableDetails() {
      const { embedded } = this.props;
      const classes = this.props.classes;

      if (embedded) {
        return;
      }

      const namedElement = this.getNamedSelectedElement();
      if (!namedElement) {
        return;
      }

      const project = defined(this.project());
      const model = defined(this.getModel());

      const ident = defined(namedElement.ident());
      const variable = defined(model.variables.get(ident));
      const series = project.getSeries(this.state.data, this.state.modelName, variable.ident);

      const activeTab = this.state.variableDetailsActiveTab;

      return (
        <div className={classes.varDetails}>
          <VariableDetails
            key={`vd-${this.state.projectVersion}-${this.state.projectOffset}-${ident}`}
            variable={variable}
            viewElement={namedElement}
            data={series}
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

      if (namedElement.ident() !== ident) {
        return;
      }

      this.handleSelectionDelete();
    };

    handleClearSelectedTool = () => {
      this.setState({ selectedTool: undefined });
    };

    handleSelectStock = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
      this.setState({
        selectedTool: 'stock',
      });
    };

    handleSelectFlow = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
      this.setState({
        selectedTool: 'flow',
      });
    };

    handleSelectAux = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
      this.setState({
        selectedTool: 'aux',
      });
    };

    handleSelectLink = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
      this.setState({
        selectedTool: 'link',
      });
    };

    updateVariableErrors(project: Project): Project {
      const engine = this.engine();
      if (!engine) {
        return project;
      }

      const modelName = this.state.modelName;
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const varErrors = engine.getModelVariableErrors(modelName);
      if (varErrors.size > 0) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
        for (const ident of varErrors.keys()) {
          project = project.updateIn(
            ['models', modelName, 'variables', ident],
            (v: Variable): Variable => {
              return v.set('hasError', true);
            },
          );
        }
      }

      return project;
    }

    async openEngine(serializedProject: Readonly<Uint8Array>, project: Project): Promise<IEngine | undefined> {
      this.activeEngine?.free();
      this.activeEngine = undefined;

      const engine: IEngine | undefined = await open(serializedProject as Uint8Array);
      if (!engine) {
        this.appendModelError(`opening the project in the engine failed`);
        return;
      }
      this.activeEngine = engine;

      this.setState({ activeProject: this.updateVariableErrors(project) });

      return engine;
    }

    handleUndoRedo = (kind: 'undo' | 'redo') => {
      const delta = kind === 'undo' ? 1 : -1;
      let projectOffset = this.state.projectOffset + delta;
      // ensure our offset is always valid
      projectOffset = Math.min(projectOffset, this.state.projectHistory.size - 1);
      projectOffset = Math.max(projectOffset, 0);
      const serializedProject = defined(this.state.projectHistory.get(projectOffset));
      const projectVersion = this.state.projectVersion + 0.01;
      const activeProject = Project.deserializeBinary(serializedProject);
      this.setState({ activeProject, projectOffset, projectVersion });

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(async () => {
        await this.openEngine(serializedProject, activeProject);
        this.scheduleSimRun();
        this.scheduleSave(serializedProject);
      });
    };

    async takeSnapshot() {
      const project = this.project();
      if (!project || !this.state.modelName) {
        return;
      }
      const { data, modelName } = this.state;

      const [svg, viewbox] = renderSvgToString(project, modelName, data);
      const osCanvas = new OffscreenCanvas(viewbox.width * 4, viewbox.height * 4);
      const ctx = osCanvas.getContext('2d');
      const canvas = Canvg.fromString(exists(ctx), svg, {
        ignoreMouse: true,
        ignoreAnimation: true,
        // ignoreDimensions: false,
      });

      await canvas.render();

      const snapshotBlob = await osCanvas.convertToBlob();

      this.setState({ snapshotBlob });
    }
    handleSnapshot = (kind: 'show' | 'close') => {
      if (kind === 'show') {
        // eslint-disable-next-line @typescript-eslint/no-misused-promises
        setTimeout(async () => {
          await this.takeSnapshot();
        });
      }
    };

    getUndoRedoBar() {
      const { embedded } = this.props;
      const classes = this.props.classes;
      if (embedded) {
        return undefined;
      }

      const undoEnabled =
        this.state.projectHistory.size > 1 && this.state.projectOffset < this.state.projectHistory.size - 1;
      const redoEnabled = this.state.projectOffset > 0;

      return (
        <div className={classes.undoRedoBar}>
          <Snapshotter onSnapshot={this.handleSnapshot} />
          <UndoRedoBar undoEnabled={undoEnabled} redoEnabled={redoEnabled} onUndoRedo={this.handleUndoRedo} />
        </div>
      );
    }

    getEditorControls() {
      const { embedded } = this.props;
      const classes = this.props.classes;
      const { dialOpen, dialVisible, selectedTool } = this.state;

      if (embedded) {
        return undefined;
      }

      return (
        <SpeedDial
          ariaLabel="SpeedDial openIcon example"
          className={classes.speedDial}
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
            className={selectedTool === 'stock' ? classes.selectedTool : undefined}
          />
          <SpeedDialAction
            icon={<FlowIcon />}
            title="Flow"
            onClick={this.handleSelectFlow}
            className={selectedTool === 'flow' ? classes.selectedTool : undefined}
          />
          <SpeedDialAction
            icon={<AuxIcon />}
            title="Variable"
            onClick={this.handleSelectAux}
            className={selectedTool === 'aux' ? classes.selectedTool : undefined}
          />
          <SpeedDialAction
            icon={<LinkIcon />}
            title="Link"
            onClick={this.handleSelectLink}
            className={selectedTool === 'link' ? classes.selectedTool : undefined}
          />
        </SpeedDial>
      );
    }

    getSnapshot() {
      const { embedded } = this.props;
      const classes = this.props.classes;
      const { snapshotBlob } = this.state;

      if (embedded || !snapshotBlob) {
        return undefined;
      }

      return (
        <Card className={classes.snapshotCard} elevation={2}>
          <CardContent>
            <img src={URL.createObjectURL(snapshotBlob)} className={classes.snapshotImg} alt="profile snapshot" />
          </CardContent>
          <CardActions>
            <Button size="small" color="primary" onClick={this.handleClearSnapshot}>
              Close
            </Button>
          </CardActions>
        </Card>
      );
    }

    handleClearSnapshot = () => {
      this.setState({ snapshotBlob: undefined });
    };

    render() {
      const { embedded, classes } = this.props;

      const classNames = classes.editor + (embedded ? '' : ' ' + classes.editorBg);

      return (
        <div className={classNames}>
          {this.getDrawer()}
          {this.getVariableDetails()}
          {this.getSearchBar()}
          {this.getCanvas()}
          {this.getSnackbar()}
          {this.getEditorControls()}
          {this.getUndoRedoBar()}
          {this.getSnapshot()}
        </div>
      );
    }
  },
);
