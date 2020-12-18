// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { toUint8Array, fromUint8Array } from 'js-base64';

import { List, Map, Set, Stack } from 'immutable';

import { History } from 'history';

import { Canvg } from 'canvg';

import { Project as ProjectPB } from '../../system-dynamics-engine/src/project_io_pb';

import { Engine as IEngine } from '../../engine-interface';

import {
  Project,
  Model,
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
} from '../datamodel';

import { uint8ArraysEqual } from '../common';

import { Canvas, /* fauxTargetUid, inCreationCloudUid, */ inCreationUid } from './drawing/Canvas';
import { Point } from './drawing/common';
import { takeoffθ } from './drawing/Connector';
import { UpdateCloudAndFlow, UpdateFlow, UpdateStockAndFlows } from './drawing/Flow';

import { baseURL, defined, exists, Series } from '../common';

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
import { renderSvgToString } from '../../render-common';

import { Card } from '@material-ui/core';
import Button from '@material-ui/core/Button';
import CardActions from '@material-ui/core/CardActions';
import CardContent from '@material-ui/core/CardContent';

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
      // if (this.state.projectHistory.size === 0) {
      //   return undefined;
      // }
      //
      // const off = optionalOffset !== undefined ? optionalOffset : this.state.projectOffset;
      // return this.state.projectHistory.get(off);
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

      const activeProjectPB = ProjectPB.deserializeBinary(serializedProject as Uint8Array);
      const activeProject = new Project(activeProjectPB);

      const priorHistory = this.state.projectHistory.slice();

      this.setState({
        projectHistory: priorHistory.unshift(serializedProject).slice(0, MaxUndoSize),
        activeProject,
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
        await this.save(project, projectVersion);
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

      let projectBinary: Uint8Array;
      let projectPB: ProjectPB;
      try {
        projectBinary = toUint8Array(projectResponse.pb);
        projectPB = ProjectPB.deserializeBinary(projectBinary);
      } catch (err) {
        this.appendModelError(`project protobuf: ${err.message}`);
        return;
      }

      const project = new Project(projectPB);
      const engine = await this.openEngine(projectBinary);
      if (!engine) {
        return;
      }

      this.activeEngine = engine;

      // we don't call updateProject here because we don't want to
      // POST a new version up when we've just downloaded it.
      this.setState({
        activeProject: project,
        projectVersion: defined(projectResponse.version) as number,
        projectHistory: Stack([projectBinary]),
        projectOffset: 0,
      });

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
      const engine = this.engine();
      if (!engine) {
        return;
      }
      let newProject;
      try {
        const err = engine.rename(this.state.modelName, oldName, newName);
        if (err) {
          throw err;
        }
        newProject = engine.serializeToProtobuf();
      } catch (err) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
        if (err.hasOwnProperty('code')) {
          // eslint-disable-next-line @typescript-eslint/no-unsafe-call
          this.appendModelError(`error code ${err.code}: ${err.get_details()}`);
        } else {
          this.appendModelError(err.message);
        }
        return;
      }
      this.scheduleSimRun();
      this.updateProject(newProject);
    };

    handleSelection = (selection: Set<UID>) => {
      this.setState({
        selection,
        variableDetailsActiveTab: 0,
      });
    };

    handleSelectionDelete = () => {
      /*
      const selection = this.state.selection;
      const { modelName } = this.state;
      const updatePath = ['models', modelName, 'views', 0];
      let project = defined(this.project()).updateIn(
        updatePath,
        (view: StockFlowView): StockFlowView => {
          const isSelected = (ident: string | undefined): boolean => {
            if (ident === undefined) {
              return false;
            }
            for (const e of view.elements) {
              if (e.hasName && e.ident === ident) {
                return selection.contains(e.uid);
              }
            }
            return false;
          };

          // this will remove the selected elements, clouds, and connectors
          let elements = view.elements.filter((element: ViewElement) => {
            const remove =
              selection.contains(element.uid) ||
              (element.type === 'cloud' && selection.contains(defined(element.flowUid))) ||
              (element.type === 'connector' && (isSelected(element.to) || isSelected(element.from)));
            return !remove;
          });

          // next we have to potentially make new clouds if we've deleted a stock
          let { nextUid } = view;
          const clouds: ViewElement[] = [];
          elements = elements.map((element: ViewElement) => {
            if (element.type !== 'flow' || !element.pts) {
              return element;
            }
            const pts = element.pts.map((pt) => {
              if (!pt.uid || !selection.contains(pt.uid)) {
                return pt;
              }

              const cloud = new ViewElement({
                type: 'cloud',
                uid: nextUid++,
                x: pt.x,
                y: pt.y,
                flowUid: element.uid,
              });

              clouds.push(cloud);

              return pt.set('uid', cloud.uid);
            });
            element = element.set('pts', pts);
            return element;
          });
          elements = elements.concat(clouds);
          return view.merge({ elements, nextUid });
        },
      );
      project = project.deleteVariables(this.state.modelName, this.getSelectionIdents());
       */
      this.setState({
        selection: Set<number>(),
      });
      // this.updateProject(project);
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

    handleFlowAttach = (_flow: ViewElement, _targetUid: number, _cursorMoveDelta: Point) => {
      /*
      let { selection } = this.state;
      const { modelName } = this.state;
      const updatePath = ['models', modelName, 'xModel', 'views', 0];
      let isCreatingNew = false;
      let stockDetachingIdent: string | undefined;
      let stockAttachingIdent: string | undefined;
      let sourceStockIdent: string | undefined;
      let uidToDelete: number | undefined;
      let updatedCloud: ViewElement | undefined;
      let newClouds = List<ViewElement>();
      let project = defined(this.project()).updateIn(
        updatePath,
        (view: View): View => {
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

            const oldTo = getUid(defined(defined(defined(element.pts).last()).uid));
            let newCloud = false;
            let updateCloud = false;
            let to: ViewElement;
            if (targetUid) {
              if (oldTo.type === 'cloud') {
                uidToDelete = oldTo.uid;
              }
              to = getUid(targetUid);
            } else if (oldTo.type === 'cloud') {
              updateCloud = true;
              to = oldTo.merge({
                x: oldTo.cx - cursorMoveDelta.x,
                y: oldTo.cy - cursorMoveDelta.y,
              });
            } else {
              newCloud = true;
              to = new ViewElement({
                uid: nextUid++,
                type: 'cloud',
                x: oldTo.cx - cursorMoveDelta.x,
                y: oldTo.cy - cursorMoveDelta.y,
                flowUid: flow.uid,
              });
            }

            if (oldTo.uid !== to.uid) {
              if (oldTo.type === 'stock') {
                stockDetachingIdent = oldTo.ident;
              }
              if (to.type === 'stock') {
                stockAttachingIdent = to.ident;
              }
            }

            const moveDelta = {
              x: oldTo.cx - to.cx,
              y: oldTo.cy - to.cy,
            };
            const pts = (element.pts || List<XmilePoint>()).map((point, _i) => {
              if (point.uid !== oldTo.uid) {
                return point;
              }
              return point.set('uid', to.uid);
            });
            to = to.merge({
              x: oldTo.cx,
              y: oldTo.cy,
              width: undefined,
              height: undefined,
            });
            element = element.set('pts', pts);

            [to, element] = UpdateCloudAndFlow(to, element, moveDelta);
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
            const firstPt = defined(defined(flow.pts).first());
            const sourceUid = firstPt.uid;
            if (sourceUid === inCreationCloudUid) {
              const newCloud = new ViewElement({
                type: 'cloud',
                uid: nextUid++,
                x: firstPt.x,
                y: firstPt.y,
                flowUid: flow.uid,
              });
              elements = elements.push(newCloud);
              flow = flow.set(
                'pts',
                (flow.pts || List<XmilePoint>()).map((pt) => {
                  if (pt.uid === inCreationCloudUid) {
                    return pt.set('uid', newCloud.uid);
                  }
                  return pt;
                }),
              );
            } else if (sourceUid) {
              const sourceStock = getUid(sourceUid);
              sourceStockIdent = sourceStock.ident;
            }
            const lastPt = defined(defined(flow.pts).last());
            if (lastPt.uid === fauxTargetUid) {
              let newCloud = false;
              let to: ViewElement;
              if (targetUid) {
                to = getUid(targetUid);
                stockAttachingIdent = to.ident;
                cursorMoveDelta = {
                  x: 0,
                  y: 0,
                };
              } else {
                to = new ViewElement({
                  type: 'cloud',
                  uid: nextUid++,
                  x: lastPt.x,
                  y: lastPt.y,
                  flowUid: flow.uid,
                });
                newCloud = true;
              }
              flow = flow.set(
                'pts',
                (flow.pts || List<XmilePoint>()).map((pt) => {
                  if (pt.uid === fauxTargetUid) {
                    return pt.set('uid', to.uid);
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
          return view.merge({ nextUid, elements });
        },
      );
      if (isCreatingNew) {
        project = project.addNewVariable(this.state.modelName, flow.type, defined(flow.name));
        if (sourceStockIdent) {
          project = project.addStocksFlow(this.state.modelName, sourceStockIdent, flow.ident, 'out');
        }
      }
      if (stockAttachingIdent) {
        project = project.addStocksFlow(this.state.modelName, stockAttachingIdent, flow.ident, 'in');
      }
      if (stockDetachingIdent) {
        project = project.removeStocksFlow(this.state.modelName, stockDetachingIdent, flow.ident, 'in');
      }
      this.setState({ selection });
      this.updateProject(project);
      this.scheduleSimRun();

       */
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
          this.appendModelError(`updating the view failed (code ${err.code}, details: '${err.getDetails()}')`);
        }
        this.updateProject(engine.serializeToProtobuf());
      }
    }

    handleCreateVariable = (_element: ViewElement) => {
      /*
      const updatePath = ['models', this.state.modelName, 'xModel', 'views', 0];
      let project = defined(this.project()).updateIn(
        updatePath,
        (view: View): View => {
          let nextUid = view.nextUid;
          element = element.set('uid', nextUid++);
          const elements = view.elements.push(element);
          return view.merge({ nextUid, elements });
        },
      );

      project = project.addNewVariable(this.state.modelName, element.type, defined(element.name));

      this.setState({
        selection: Set<number>(),
      });
      this.updateProject(project);
       */
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

      let dt = simSpec.dt.dt;
      if (simSpec.dt.isReciprocal) {
        dt = 1 / dt;
      }

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

      return (
        <Canvas
          embedded={!!embedded}
          dmProject={project}
          dmModel={model}
          dmView={view}
          data={this.state.data}
          selectedTool={this.state.selectedTool}
          selection={this.state.selection}
          onRenameVariable={this.handleRename}
          onSetSelection={this.handleSelection}
          onMoveSelection={this.handleSelectionMove}
          onMoveFlow={this.handleFlowAttach}
          onAttachLink={this.handleLinkAttach}
          onCreateVariable={this.handleCreateVariable}
          onClearSelectedTool={this.handleClearSelectedTool}
          onDeleteSelection={this.handleSelectionDelete}
          onMoveLabel={this.handleMoveLabel}
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
      const engine = this.engine();
      if (!engine) {
        return;
      }

      if (newTable) {
        const gf = GraphicalFunction.toPb(newTable);
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

      const model = this.getModel();
      if (!model) {
        return;
      }

      const ident = defined(namedElement.ident());
      const variable = defined(model.variables.get(ident));
      const series = this.state.data.get(ident);
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

    async openEngine(serializedProject: Readonly<Uint8Array>): Promise<IEngine | undefined> {
      const { open } = await import('../../engine-v2/pkg');

      const engine = open(serializedProject as Uint8Array);
      if (!engine) {
        this.appendModelError(`opening the project in the engine failed`);
        return;
      }

      return engine;
    }

    handleUndoRedo = (kind: 'undo' | 'redo') => {
      const delta = kind === 'undo' ? 1 : -1;
      let projectOffset = this.state.projectOffset + delta;
      // ensure our offset is always valid
      projectOffset = Math.min(projectOffset, this.state.projectHistory.size - 1);
      projectOffset = Math.max(projectOffset, 0);
      const serializedProject = defined(this.state.projectHistory.get(projectOffset));
      const activeProjectPB = ProjectPB.deserializeBinary(serializedProject as Uint8Array);
      const activeProject = new Project(activeProjectPB);
      this.setState({ activeProject, projectOffset });

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(async () => {
        this.activeEngine?.free();
        this.activeEngine = await this.openEngine(serializedProject);
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
