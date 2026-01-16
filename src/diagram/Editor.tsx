// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List, Map, Set, Stack } from 'immutable';

import clsx from 'clsx';
import { styled } from '@mui/material/styles';
import IconButton from '@mui/material/IconButton';
import TextField from '@mui/material/TextField';
import Autocomplete from '@mui/material/Autocomplete';
import Paper from '@mui/material/Paper';
import Snackbar from '@mui/material/Snackbar';
import ClearIcon from '@mui/icons-material/Clear';
import EditIcon from '@mui/icons-material/Edit';
import MenuIcon from '@mui/icons-material/Menu';
import SpeedDial, { CloseReason } from '@mui/material/SpeedDial';
import SpeedDialAction from '@mui/material/SpeedDialAction';
import SpeedDialIcon from '@mui/material/SpeedDialIcon';
import { Card } from '@mui/material';
import Button from '@mui/material/Button';
import CardActions from '@mui/material/CardActions';
import CardContent from '@mui/material/CardContent';
import { canonicalize } from '@system-dynamics/core/canonicalize';

import { Project as Engine2Project, SimlinErrorKind, SimlinUnitErrorKind } from '@system-dynamics/engine2';
import type { JsonProjectPatch, JsonModelOperation, JsonSimSpecs, JsonArrayedEquation } from '@system-dynamics/engine2';
import type { ErrorDetail } from '@system-dynamics/engine2';
import { stockFlowViewToJson } from './view-conversion';
import {
  Project,
  Model,
  Variable,
  UID,
  Stock as StockVar,
  Flow,
  Aux,
  ScalarEquation,
  ApplyToAllEquation,
  ArrayedEquation,
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
  EquationError,
  SimError,
  ModelError,
  ErrorCode,
  Rect,
  UnitError,
} from '@system-dynamics/core/datamodel';
import { defined, exists, Series, toInt, uint8ArraysEqual } from '@system-dynamics/core/common';

import { AuxIcon } from './AuxIcon';
import { Toast } from './ErrorToast';
import { FlowIcon } from './FlowIcon';
import { LinkIcon } from './LinkIcon';
import { ModelPropertiesDrawer } from './ModelPropertiesDrawer';
import { renderSvgToString } from './render-common';
import { Status } from './Status';
import { StockIcon } from './StockIcon';
import { UndoRedoBar } from './UndoRedoBar';
import { VariableDetails } from './VariableDetails';
import { ErrorDetails } from './ErrorDetails';
import { ZoomBar } from './ZoomBar';
import { Canvas, fauxCloudTargetUid, inCreationCloudUid, inCreationUid } from './drawing/Canvas';
import { Point } from './drawing/common';
import { takeoffθ } from './drawing/Connector';
import { UpdateCloudAndFlow, UpdateFlow, UpdateStockAndFlows } from './drawing/Flow';

const MaxUndoSize = 5;
const SearchbarWidthSm = 359;
const SearchbarWidthMd = 420;
const SearchbarWidthLg = 480;

function radToDeg(r: number): number {
  return (r * 180) / Math.PI;
}

function convertErrorDetails(
  errors: ErrorDetail[],
  modelName: string,
): {
  varErrors: Map<string, List<EquationError>>;
  unitErrors: Map<string, List<UnitError>>;
} {
  let varErrors = Map<string, List<EquationError>>();
  let unitErrors = Map<string, List<UnitError>>();

  for (const err of errors) {
    if (err.modelName !== modelName) {
      continue;
    }

    const ident = err.variableName;
    if (!ident) {
      continue;
    }

    const isUnitError = err.kind === SimlinErrorKind.Units;

    if (isUnitError) {
      const unitError = new UnitError({
        start: err.startOffset ?? 0,
        end: err.endOffset ?? 0,
        code: err.code as unknown as ErrorCode,
        isConsistencyError: err.unitErrorKind === SimlinUnitErrorKind.Consistency,
        details: err.message ?? undefined,
      });
      const existing = unitErrors.get(ident) ?? List<UnitError>();
      unitErrors = unitErrors.set(ident, existing.push(unitError));
    } else {
      const eqError = new EquationError({
        start: err.startOffset ?? 0,
        end: err.endOffset ?? 0,
        code: err.code as unknown as ErrorCode,
      });
      const existing = varErrors.get(ident) ?? List<EquationError>();
      varErrors = varErrors.set(ident, existing.push(eqError));
    }
  }

  return { varErrors, unitErrors };
}

class EditorError implements Error {
  name = 'EditorError';
  message: string;
  constructor(msg: string) {
    this.message = msg;
  }
}

interface EditorState {
  modelErrors: List<Error>;
  activeProject: Project;
  projectHistory: Stack<Readonly<Uint8Array>>;
  projectOffset: number;
  modelName: string;
  dialOpen: boolean;
  dialVisible: boolean;
  selectedTool: 'stock' | 'flow' | 'aux' | 'link' | undefined;
  data: Map<string, Series>;
  selection: Set<UID>;
  status: 'ok' | 'error' | 'disabled';
  showDetails: 'variable' | 'errors' | undefined;
  flowStillBeingCreated: boolean;
  drawerOpen: boolean;
  projectVersion: number;
  snapshotBlob: Blob | undefined;
  variableDetailsActiveTab: number;
}

interface EditorProps {
  initialProjectBinary: Readonly<Uint8Array>;
  initialProjectVersion: number;
  name: string; // used when saving
  embedded?: boolean;
  onSave: (project: Readonly<Uint8Array>, currVersion: number) => Promise<number | undefined>;
  readOnlyMode?: boolean;
}

export const Editor = styled(
  class InnerEditor extends React.PureComponent<EditorProps & { className?: string }, EditorState> {
    engine2Project?: Engine2Project;
    newEngineShouldPullView = false;
    newEngineQueuedView?: StockFlowView;

    inSave = false;
    queuedModelToSave?: Readonly<Uint8Array>;

    constructor(props: EditorProps) {
      super(props);

      const activeProject = Project.deserializeBinary(props.initialProjectBinary);

      this.state = {
        activeProject,
        projectHistory: Stack<Readonly<Uint8Array>>([props.initialProjectBinary]),
        projectOffset: 0,
        modelErrors: List<Error>(),
        modelName: 'main',
        dialOpen: false,
        dialVisible: true,
        selectedTool: undefined,
        data: Map<string, Series>(),
        selection: Set<number>(),
        status: 'disabled',
        showDetails: undefined,
        flowStillBeingCreated: false,
        drawerOpen: false,
        projectVersion: props.initialProjectVersion,
        snapshotBlob: undefined,
        variableDetailsActiveTab: 0,
      };

      setTimeout(async () => {
        await this.openEngine2Project(props.initialProjectBinary, activeProject);
        this.scheduleSimRun();
      });
    }

    componentDidMount() {
      if (this.props.readOnlyMode)
        this.setState({
          modelErrors: this.state.modelErrors.push(
            new Error("This is a read-only version. Any changes you make won't be saved."),
          ),
        });
    }

    project(): Project | undefined {
      return this.state.activeProject;
    }

    engine2(): Engine2Project | undefined {
      return this.engine2Project;
    }

    scheduleSimRun(): void {
      setTimeout(() => {
        const engine2 = this.engine2();
        if (!engine2) {
          return;
        }
        this.loadSim(engine2);
      });
    }

    loadSim(engine2: Engine2Project) {
      this.recalculateStatus();

      if (!engine2.isSimulatable()) {
        return;
      }
      try {
        const model = engine2.mainModel;
        const run = model.run();
        const idents = run.varNames;
        const time = run.getSeries('time') ?? new Float64Array(0);
        const data = Map<string, Series>(
          idents.map((ident) => {
            const values = run.getSeries(ident) ?? new Float64Array(0);
            return [ident, { name: ident, time, values }];
          }),
        );
        const project = defined(this.project());
        this.setState({
          activeProject: project.attachData(data, this.state.modelName),
          data,
        });
      } catch (e) {
        this.setState({
          modelErrors: this.state.modelErrors.push(e as Error),
        });
      }
    }

    updateProject(serializedProject: Readonly<Uint8Array>, scheduleSave = true) {
      if (this.state.projectHistory.size > 0) {
        const current = this.state.projectHistory.get(this.state.projectOffset);
        if (uint8ArraysEqual(serializedProject, current)) {
          return;
        }
      }

      let activeProject = this.updateVariableErrors(Project.deserializeBinary(serializedProject));
      if (this.state.data) {
        activeProject = activeProject.attachData(this.state.data, this.state.modelName);
      }

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

    scheduleSave(project: Readonly<Uint8Array>): void {
      const { projectVersion } = this.state;

      setTimeout(async () => {
        await this.save(project, toInt(projectVersion));
      });
    }

    async save(project: Readonly<Uint8Array>, currVersion: number): Promise<void> {
      if (this.inSave) {
        this.queuedModelToSave = project;
        return;
      }

      this.inSave = true;

      let version: number | undefined;
      try {
        version = await this.props.onSave(project, currVersion);
        if (version) {
          this.setState({ projectVersion: version });
        }
      } catch (err) {
        this.setState({
          modelErrors: this.state.modelErrors.push(err as Error),
        });
        return;
      }

      this.inSave = false;

      if (this.queuedModelToSave) {
        const project = this.queuedModelToSave;
        this.queuedModelToSave = undefined;
        if (version) {
          await this.save(project, version);
        } else {
          this.setState({
            modelErrors: this.state.modelErrors.push(new Error('last save failed, please reload')),
          });
        }
      }
    }

    appendModelError(msg: string) {
      this.setState((prevState: EditorState) => ({
        modelErrors: prevState.modelErrors.push(new EditorError(msg)),
      }));
    }

    handleDialClick = (_event: React.MouseEvent<HTMLDivElement, MouseEvent>) => {
      this.setState({
        dialOpen: !this.state.dialOpen,
        selectedTool: this.state.dialOpen ? undefined : this.state.selectedTool,
      });
    };

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

      const engine2 = this.engine2();
      if (!engine2) {
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
        if (namedElement.ident !== oldIdent) {
          return element;
        }

        return namedElement.set('name', newName);
      });

      const updatedView = view.set('elements', elements);

      const ops: JsonModelOperation[] = [
        {
          type: 'rename_variable',
          payload: { from: oldIdent, to: canonicalize(newName) },
        },
        {
          type: 'upsert_view',
          payload: { index: 0, view: stockFlowViewToJson(updatedView) },
        },
      ];

      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops }],
      };

      try {
        engine2.applyPatch(patch);
      } catch (e: any) {
        const msg = e?.message ?? 'Unknown error during rename';
        this.appendModelError(msg);
        return;
      }

      this.setState({
        flowStillBeingCreated: false,
      });
      this.updateProject(engine2.serializeProtobuf());
      this.scheduleSimRun();
    };

    handleSelection = (selection: Set<UID>) => {
      this.setState({
        selection,
        flowStillBeingCreated: false,
        variableDetailsActiveTab: 0,
      });
      if (selection.isEmpty()) {
        this.setState({ showDetails: undefined });
      }
    };

    handleShowVariableDetails = () => {
      this.setState({ showDetails: 'variable' });
    };

    getLatexEquation = (ident: string): string | undefined => {
      const engine2 = this.engine2();
      if (!engine2) return undefined;
      try {
        const model = engine2.getModel(this.state.modelName);
        return model.getLatexEquation(ident) ?? undefined;
      } catch {
        return undefined;
      }
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

      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }

      const deleteOps: JsonModelOperation[] = this.getSelectionIdents().map((ident) => ({
        type: 'delete_variable' as const,
        payload: { ident },
      }));

      if (deleteOps.length > 0) {
        const patch: JsonProjectPatch = {
          models: [{ name: modelName, ops: deleteOps }],
        };
        try {
          engine2.applyPatch(patch);
        } catch (e: any) {
          this.appendModelError(e?.message ?? 'Unknown error during delete');
        }
      }

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

    handleFlowAttach = (
      flow: FlowViewElement,
      targetUid: number,
      cursorMoveDelta: Point,
      fauxTargetCenter: Point | undefined,
      inCreation: boolean,
    ) => {
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
            stockDetachingIdent = oldTo.ident;
          }
          if (to instanceof StockViewElement) {
            stockAttachingIdent = to.ident;
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
          const sourceStock = getUid(sourceUid) as StockViewElement;
          sourceStockIdent = defined(sourceStock.ident);
        }
        const lastPt = defined(flow.points.last());
        if (lastPt.attachedToUid === fauxCloudTargetUid) {
          let newCloud = false;
          let to: StockViewElement | CloudViewElement;
          if (targetUid) {
            to = getUid(targetUid) as StockViewElement | CloudViewElement;
            stockAttachingIdent = defined(to.ident);
            cursorMoveDelta = {
              x: 0,
              y: 0,
            };
          } else {
            to = new CloudViewElement({
              uid: nextUid++,
              x: defined(fauxTargetCenter).x,
              y: defined(fauxTargetCenter).y,
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

      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }

      const ops: JsonModelOperation[] = [];

      if (isCreatingNew) {
        ops.push({
          type: 'upsert_flow',
          payload: {
            flow: {
              name: (flow as NamedViewElement).name,
              equation: '',
            },
          },
        });
      }

      if (sourceStockIdent) {
        const model = defined(this.getModel());
        const stockVar = model.variables.get(sourceStockIdent);
        if (stockVar instanceof StockVar) {
          ops.push({
            type: 'upsert_stock',
            payload: {
              stock: {
                name: stockVar.ident,
                inflows: stockVar.inflows.toArray(),
                outflows: stockVar.outflows.push(flow.ident).toArray(),
              },
            },
          });
        }
      }

      if (stockAttachingIdent) {
        const model = defined(this.getModel());
        const stockVar = model.variables.get(stockAttachingIdent);
        if (stockVar instanceof StockVar) {
          ops.push({
            type: 'upsert_stock',
            payload: {
              stock: {
                name: stockVar.ident,
                inflows: stockVar.inflows.push(flow.ident).toArray(),
                outflows: stockVar.outflows.toArray(),
              },
            },
          });
        }
      }

      if (stockDetachingIdent) {
        const model = defined(this.getModel());
        const stockVar = model.variables.get(stockDetachingIdent);
        if (stockVar instanceof StockVar) {
          ops.push({
            type: 'upsert_stock',
            payload: {
              stock: {
                name: stockVar.ident,
                inflows: stockVar.inflows.filter((f) => f !== flow.ident).toArray(),
                outflows: stockVar.outflows.toArray(),
              },
            },
          });
        }
      }

      if (ops.length > 0) {
        const patch: JsonProjectPatch = {
          models: [{ name: this.state.modelName, ops }],
        };
        try {
          engine2.applyPatch(patch);
        } catch (e: any) {
          this.appendModelError(e?.message ?? 'Unknown error during flow attach');
        }
      }

      this.updateView(view.merge({ nextUid, elements }));
      this.setState({
        selection,
        flowStillBeingCreated: inCreation,
      });
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
          if (e.isNamed() && e.ident === ident) {
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
      const engine2 = this.engine2();
      if (engine2) {
        const ops: JsonModelOperation[] = [
          {
            type: 'upsert_view',
            payload: { index: 0, view: stockFlowViewToJson(view) },
          },
        ];
        const patch: JsonProjectPatch = {
          models: [{ name: this.state.modelName, ops }],
        };
        try {
          engine2.applyPatch(patch);
        } catch (e: any) {
          const msg = e?.message ?? 'Unknown error during view update';
          this.appendModelError(msg);
          return;
        }
        this.updateProject(engine2.serializeProtobuf());
      }
    }

    handleCreateVariable = (element: ViewElement) => {
      const view = defined(this.getView());
      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }

      let nextUid = view.nextUid;
      const elements = view.elements.push(element.set('uid', nextUid++));
      const elementType = viewElementType(element);
      const name = (element as NamedViewElement).name;

      let op: JsonModelOperation;
      if (elementType === 'stock') {
        op = {
          type: 'upsert_stock',
          payload: {
            stock: {
              name,
              inflows: [],
              outflows: [],
              initial_equation: '',
            },
          },
        };
      } else if (elementType === 'flow') {
        op = {
          type: 'upsert_flow',
          payload: {
            flow: {
              name,
              equation: '',
            },
          },
        };
      } else {
        op = {
          type: 'upsert_aux',
          payload: {
            aux: {
              name,
              equation: '',
            },
          },
        };
      }

      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops: [op] }],
      };

      try {
        engine2.applyPatch(patch);
      } catch (e: any) {
        this.appendModelError(e?.message ?? 'Unknown error during variable creation');
      }

      this.updateView(view.merge({ nextUid, elements }));
      this.setState({
        selection: Set<number>(),
      });
    };

    handleSelectionMove = (delta: Point, arcPoint?: Point) => {
      const view = defined(this.getView());
      const selection = this.state.selection;

      const getName = (ident: string) => {
        for (const e of view.elements) {
          if (e.isNamed() && e.ident === ident) {
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
          const stock = defined(defined(this.getModel()).variables.get(element.ident)) as StockVar;
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
      let nonSelectedElements = Map<number, ViewElement>();
      let selectedElements = Map<number, ViewElement>();
      for (const e of elements) {
        if (selection.has(e.uid)) {
          selectedElements = selectedElements.set(e.uid, e);
        } else {
          nonSelectedElements = nonSelectedElements.set(e.uid, e);
        }
        if (e.isNamed()) {
          const ident = defined(e.ident);
          namedElements = namedElements.set(ident, selectedElements.get(e.uid, e));
        }
      }

      elements = elements.map((element: ViewElement) => {
        if (!(element instanceof LinkViewElement)) {
          return element.isNamed() ? defined(namedElements.get(defined(element.ident))) : element;
        }
        // TODO: this could be an alias, which doesn't have a name.  Why are we doing this by name anyway?
        // const fromName = defined(getUid(element.fromUid).ident);
        // const toName = defined(getUid(element.toUid).ident);
        // if it hasn't been updated, nothing to do
        if (!(selectedElements.has(element.fromUid) || selectedElements.has(element.toUid))) {
          return element;
        }
        const from = selectedElements.get(element.fromUid) || nonSelectedElements.get(element.fromUid);
        if (!from) {
          return element;
        }
        const to = selectedElements.get(element.toUid) || nonSelectedElements.get(element.toUid);
        if (!to) {
          return element;
        }
        const atan2 = Math.atan2;
        const oldTo = defined(getUid(element.toUid));
        const oldFrom = defined(getUid(element.fromUid));
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

    applySimSpecChange(updates: Partial<JsonSimSpecs>) {
      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }

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
        start_time: updates.start_time ?? simSpec.start,
        end_time: updates.end_time ?? simSpec.stop,
        dt: updates.dt ?? dt,
        time_units: updates.time_units ?? simSpec.timeUnits,
        save_step: updates.save_step ?? saveStep,
        method: updates.method ?? simSpec.simMethod,
      };

      const patch: JsonProjectPatch = {
        project_ops: [
          {
            type: 'set_sim_specs',
            payload: { sim_specs: simSpecs },
          },
        ],
      };

      try {
        engine2.applyPatch(patch);
      } catch (e: any) {
        this.appendModelError(e?.message ?? 'Unknown error updating sim specs');
        return;
      }

      this.updateProject(engine2.serializeProtobuf());
      this.scheduleSimRun();
    }

    handleStartTimeChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const value = Number(event.target.value);
      this.applySimSpecChange({ start_time: value });
    };

    handleStopTimeChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const value = Number(event.target.value);
      this.applySimSpecChange({ end_time: value });
    };

    handleDtChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const value = Number(event.target.value);
      this.applySimSpecChange({ dt: `${value}` });
    };

    handleTimeUnitsChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const value = event.target.value;
      this.applySimSpecChange({ time_units: value });
    };

    handleDownloadXmile = () => {
      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }
      try {
        const xmile = engine2.toXmileString();
        const encoder = new TextEncoder();
        const xmileBytes = encoder.encode(xmile);
        const blob = new Blob([xmileBytes], {
          type: 'application/octet-stream',
        });
        const url = window.URL.createObjectURL(blob);
        const a = document.createElement('a');
        document.body.appendChild(a);
        try {
          (a as unknown as any).style = 'display: none';
        } catch {
          // oh well
        }
        a.href = url;
        a.download = `${this.props.name}-${this.state.projectVersion | 0}.stmx`;
        a.click();
        window.URL.revokeObjectURL(url);
      } catch (err: any) {
        if (err && err.message) {
          this.appendModelError(err.message);
        }
      }
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

    setView(view: StockFlowView): void {
      const project = defined(this.project());
      const activeProject = project.setIn(['models', this.state.modelName, 'views', 0], view);
      this.setState({ activeProject });
    }

    queueViewUpdate(view: StockFlowView): void {
      const engine2 = this.engine2();
      if (engine2) {
        const ops: JsonModelOperation[] = [
          {
            type: 'upsert_view',
            payload: { index: 0, view: stockFlowViewToJson(view) },
          },
        ];
        const patch: JsonProjectPatch = {
          models: [{ name: this.state.modelName, ops }],
        };
        try {
          engine2.applyPatch(patch);
        } catch (e: any) {
          const msg = e?.message ?? 'Unknown error during view update';
          this.appendModelError(msg);
          return;
        }

        this.updateProject(engine2.serializeProtobuf(), false);
      } else {
        // there exists a race where we need to center/update the viewBox when
        // displaying a newly imported model, but the async wasm stuff doesn't
        // complete before we want to save the viewBox change.  In this case update
        // the view in place, and set a flag we check when finalizing installation
        // of the new engine.
        this.newEngineShouldPullView = true;
        this.newEngineQueuedView = view;
        this.setView(view);
      }
    }

    handleViewBoxChange = (viewBox: Rect, zoom: number) => {
      const view = defined(this.getView());
      this.queueViewUpdate(view.merge({ viewBox, zoom }));
    };

    centerVariable(element: ViewElement): void {
      const view = defined(this.getView());
      const zoom = view.zoom;

      const cx = element.cx;
      const cy = element.cy;

      const viewCy = view.viewBox.height / 2 / zoom;
      const viewCx = (view.viewBox.width - SearchbarWidthSm) / 2 / zoom;

      const viewBox = view.viewBox.merge({
        x: viewCx - cx,
        y: viewCy - cy,
      });

      this.queueViewUpdate(view.merge({ viewBox }));
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
      const onShowVariableDetails = !embedded ? this.handleShowVariableDetails : () => {};
      const onViewBoxChange = !embedded ? this.handleViewBoxChange : () => {};

      return (
        <Canvas
          embedded={!!embedded}
          project={project}
          model={model}
          view={view}
          version={this.state.projectVersion}
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
          onShowVariableDetails={onShowVariableDetails}
          onViewBoxChange={onViewBoxChange}
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
            horizontal: 'center',
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

    getNamedElement(ident: string): ViewElement | undefined {
      const view = this.getView();
      if (!view) {
        return;
      }

      for (const e of view.elements) {
        if (e.isNamed() && e.ident === ident) {
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

    handleSearchChange = (_event: any, newValue: string | null) => {
      if (!newValue) {
        this.handleSelection(Set());
        return;
      }
      const element = this.getNamedElement(canonicalize(newValue));
      this.handleSelection(element ? Set([element.uid]) : Set());
      this.setState({
        showDetails: 'variable',
      });
      if (element) {
        this.centerVariable(element);
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
          .filter((e) => e.isNamed())
          .map((e) => (e as NamedViewElement).name.replace('\\n', ' '))
          .toArray();
      }

      const namedElement = this.getNamedSelectedElement();
      let name;
      let placeholder: string | undefined = 'Find in Model';
      if (namedElement) {
        name = defined((namedElement as NamedViewElement).name).replace('\\n', ' ');
        placeholder = undefined;
      }

      const status = this.state.status;

      return (
        <Paper className="simlin-editor-searchbar" elevation={2}>
          <IconButton
            className="simlin-editor-menubutton"
            color="inherit"
            aria-label="Menu"
            onClick={this.handleShowDrawer}
            style={{ color: '#666', display: 'block', marginLeft: 4, position: 'absolute', marginTop: 4, height: 40 }}
          >
            <MenuIcon />
          </IconButton>
          <div className="simlin-editor-searchbox">
            <Autocomplete
              key={name}
              value={name}
              onChange={this.handleSearchChange}
              clearOnEscape={true}
              defaultValue={name}
              options={autocompleteOptions}
              renderInput={(params: any) => {
                if (params.InputProps) {
                  params.InputProps.disableUnderline = true;
                }
                return <TextField {...params} variant="standard" placeholder={placeholder} />;
              }}
            />
          </div>
          <div className="simlin-editor-divider" />
          <Status status={status} onClick={this.handleStatusClick} />
        </Paper>
      );
    }

    handleClearSelected = (e: React.MouseEvent<SVGSVGElement>) => {
      e.preventDefault();
      this.handleSelection(Set());
    };

    // Returns the equation fields for a JSON patch operation.
    // For scalar equations, returns { equation: string }.
    // For arrayed equations, returns { arrayed_equation: JsonArrayedEquation }.
    getEquationFields(variable: Variable): { equation?: string; arrayed_equation?: JsonArrayedEquation } {
      const eq = variable.equation;
      if (eq instanceof ScalarEquation) {
        return { equation: eq.equation };
      } else if (eq instanceof ApplyToAllEquation) {
        return {
          arrayed_equation: {
            dimensions: eq.dimensionNames.toArray(),
            equation: eq.equation,
          },
        };
      } else if (eq instanceof ArrayedEquation) {
        return {
          arrayed_equation: {
            dimensions: eq.dimensionNames.toArray(),
            elements: eq.elements
              .entrySeq()
              .map(([subscript, eqStr]) => ({
                subscript,
                equation: eqStr,
              }))
              .toArray(),
          },
        };
      }
      return { equation: '' };
    }

    handleEquationChange = (
      ident: string,
      newEquation: string | undefined,
      newUnits: string | undefined,
      newDocs: string | undefined,
    ) => {
      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }

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
      if (variable instanceof StockVar) {
        op = {
          type: 'upsert_stock',
          payload: {
            stock: {
              name: variable.ident,
              inflows: variable.inflows.toArray(),
              outflows: variable.outflows.toArray(),
              // For stocks, the scalar equation field is initial_equation
              initial_equation: newEquation ?? existingEqFields.equation,
              arrayed_equation: newEquation !== undefined ? undefined : existingEqFields.arrayed_equation,
              units: newUnits ?? variable.units ?? undefined,
              documentation: newDocs ?? variable.documentation ?? undefined,
            },
          },
        };
      } else if (variable instanceof Flow) {
        const gf = variable.gf
          ? {
              y_points: variable.gf.yPoints?.toArray(),
              kind: variable.gf.kind,
              x_scale: variable.gf.xScale ? { min: variable.gf.xScale.min, max: variable.gf.xScale.max } : undefined,
              y_scale: variable.gf.yScale ? { min: variable.gf.yScale.min, max: variable.gf.yScale.max } : undefined,
            }
          : undefined;
        op = {
          type: 'upsert_flow',
          payload: {
            flow: {
              name: variable.ident,
              equation: newEquation ?? existingEqFields.equation,
              arrayed_equation: newEquation !== undefined ? undefined : existingEqFields.arrayed_equation,
              units: newUnits ?? variable.units ?? undefined,
              documentation: newDocs ?? variable.documentation ?? undefined,
              graphical_function: gf,
            },
          },
        };
      } else {
        const auxVar = variable as Aux;
        const gf = auxVar.gf
          ? {
              y_points: auxVar.gf.yPoints?.toArray(),
              kind: auxVar.gf.kind,
              x_scale: auxVar.gf.xScale ? { min: auxVar.gf.xScale.min, max: auxVar.gf.xScale.max } : undefined,
              y_scale: auxVar.gf.yScale ? { min: auxVar.gf.yScale.min, max: auxVar.gf.yScale.max } : undefined,
            }
          : undefined;
        op = {
          type: 'upsert_aux',
          payload: {
            aux: {
              name: auxVar.ident,
              equation: newEquation ?? existingEqFields.equation,
              arrayed_equation: newEquation !== undefined ? undefined : existingEqFields.arrayed_equation,
              units: newUnits ?? auxVar.units ?? undefined,
              documentation: newDocs ?? auxVar.documentation ?? undefined,
              graphical_function: gf,
            },
          },
        };
      }

      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops: [op] }],
      };

      try {
        engine2.applyPatch(patch);
      } catch (e: any) {
        this.appendModelError(e?.message ?? 'Unknown error during equation update');
        return;
      }

      this.updateProject(engine2.serializeProtobuf());
      this.scheduleSimRun();
    };

    handleTableChange = (ident: string, newTable: GraphicalFunction | null) => {
      const engine2 = this.engine2();
      if (!engine2) {
        return;
      }

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
            y_points: newTable.yPoints?.toArray(),
            kind: newTable.kind,
            x_scale: newTable.xScale ? { min: newTable.xScale.min, max: newTable.xScale.max } : undefined,
            y_scale: newTable.yScale ? { min: newTable.yScale.min, max: newTable.yScale.max } : undefined,
          }
        : undefined;

      // Preserve the existing equation structure when updating the graphical function
      const existingEqFields = this.getEquationFields(variable);

      let op: JsonModelOperation;
      if (variable instanceof Flow) {
        op = {
          type: 'upsert_flow',
          payload: {
            flow: {
              name: variable.ident,
              equation: existingEqFields.equation,
              arrayed_equation: existingEqFields.arrayed_equation,
              units: variable.units ?? undefined,
              documentation: variable.documentation ?? undefined,
              graphical_function: gf,
            },
          },
        };
      } else {
        const auxVar = variable as Aux;
        op = {
          type: 'upsert_aux',
          payload: {
            aux: {
              name: auxVar.ident,
              equation: existingEqFields.equation,
              arrayed_equation: existingEqFields.arrayed_equation,
              units: auxVar.units ?? undefined,
              documentation: auxVar.documentation ?? undefined,
              graphical_function: gf,
            },
          },
        };
      }

      const patch: JsonProjectPatch = {
        models: [{ name: this.state.modelName, ops: [op] }],
      };

      try {
        engine2.applyPatch(patch);
      } catch (e: any) {
        this.appendModelError(e?.message ?? 'Unknown error during table update');
        return;
      }

      this.updateProject(engine2.serializeProtobuf());
      this.scheduleSimRun();
    };

    getErrorDetails() {
      let simError: SimError | undefined;
      let modelErrors = List<ModelError>();
      let varErrors = Map<string, List<EquationError>>();
      let unitErrors = Map<string, List<UnitError>>();

      const engine2 = this.engine2();
      if (engine2) {
        const modelName = this.state.modelName;
        const errors = engine2.getErrors();

        for (const err of errors) {
          // Skip errors from other models
          if (err.modelName && err.modelName !== modelName) {
            continue;
          }

          if (err.kind === SimlinErrorKind.Simulation) {
            simError = new SimError({
              code: err.code as unknown as ErrorCode,
              details: err.message ?? undefined,
            });
          } else if (!err.variableName) {
            // Errors without a variable name (including Model/Project/Variable/Units kinds)
            // are shown as model-level errors. In the old engine API, variable errors were
            // always keyed by variable name; this handles any edge cases in the new API.
            modelErrors = modelErrors.push(
              new ModelError({
                code: err.code as unknown as ErrorCode,
                details: err.message ?? undefined,
              }),
            );
          }
        }

        const converted = convertErrorDetails(errors, modelName);
        varErrors = converted.varErrors;
        unitErrors = converted.unitErrors;
      }

      return (
        <div className="simlin-editor-vardetails">
          <ErrorDetails
            status={this.state.status}
            simError={simError}
            modelErrors={modelErrors}
            varErrors={varErrors}
            varUnitErrors={unitErrors}
          />
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
      const variable = defined(model.variables.get(ident));

      const activeTab = this.state.variableDetailsActiveTab;

      return (
        <div className="simlin-editor-vardetails">
          <VariableDetails
            key={`vd-${this.state.projectVersion}-${this.state.projectOffset}-${ident}`}
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
      const engine2 = this.engine2();
      if (!engine2) {
        return project;
      }

      const modelName = this.state.modelName;
      const errors = engine2.getErrors();
      const { varErrors, unitErrors } = convertErrorDetails(errors, modelName);

      if (varErrors.size > 0) {
        const model = defined(project.models.get(modelName));

        // if all the errors are 'just' that we have no equations,
        // don't scream "error" at the user -- they are starting from
        // scratch on a new model and don't expect it to be running yet.
        if (varErrors.size === model.variables.size && Set(varErrors.keys()).equals(Set(model.variables.keys()))) {
          let foundOtherError = false;

          for (const [, errs] of varErrors) {
            if (errs.size !== 1 || defined(errs.first()).code !== ErrorCode.EmptyEquation) {
              foundOtherError = true;
              break;
            }
          }
          if (!foundOtherError) {
            return project.set('hasNoEquations', true);
          }
        }

        for (const [ident, errs] of varErrors) {
          project = project.updateIn(
            ['models', modelName, 'variables', ident],
            ((v: Variable): Variable => v.set('errors', errs)) as (value: unknown) => unknown,
          );
        }
      }

      if (unitErrors.size > 0) {
        for (const [ident, errs] of unitErrors) {
          project = project.updateIn(
            ['models', modelName, 'variables', ident],
            ((v: Variable): Variable => v.set('unitErrors', errs)) as (value: unknown) => unknown,
          );
        }
      }

      return project;
    }

    async openEngine2Project(
      serializedProject: Readonly<Uint8Array>,
      project: Project,
    ): Promise<Engine2Project | undefined> {
      this.engine2Project?.dispose();
      this.engine2Project = undefined;

      let engine2: Engine2Project;
      try {
        engine2 = await Engine2Project.openProtobuf(serializedProject as Uint8Array);
      } catch (e: any) {
        this.appendModelError(`opening the project in the engine failed: ${e?.message ?? 'Unknown error'}`);
        return;
      }
      this.engine2Project = engine2;

      if (this.newEngineShouldPullView) {
        const queuedView = defined(this.newEngineQueuedView);
        this.newEngineShouldPullView = false;
        this.newEngineQueuedView = undefined;
        project = project.setIn(['models', this.state.modelName, 'views', 0], queuedView);
        this.queueViewUpdate(queuedView);
      }

      this.setState({
        activeProject: this.updateVariableErrors(project),
      });

      return engine2;
    }

    recalculateStatus() {
      const project = this.project();
      const engine2 = this.engine2();

      let status: 'ok' | 'error' | 'disabled';
      if (!engine2 || !project || project.hasNoEquations) {
        status = 'disabled';
      } else if (!engine2.isSimulatable()) {
        status = 'error';
      } else {
        status = 'ok';
      }

      this.setState({
        status,
      });
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

      setTimeout(async () => {
        await this.openEngine2Project(serializedProject, activeProject);
        this.scheduleSimRun();
        this.scheduleSave(serializedProject);
      });
    };

    handleZoomChange = (newZoom: number) => {
      const view = defined(this.getView());
      const oldViewBox = view.viewBox;

      const widthAdjust = this.state.showDetails ? SearchbarWidthLg : 0;

      const oldViewWidth = (oldViewBox.width - widthAdjust) / view.zoom;
      const oldViewHeight = oldViewBox.height / view.zoom;

      const newViewWidth = (oldViewBox.width - widthAdjust) / newZoom;
      const newViewHeight = oldViewBox.height / newZoom;

      const diffX = (newViewWidth - oldViewWidth) / 2;
      const diffY = (newViewHeight - oldViewHeight) / 2;

      const newViewBox = oldViewBox.merge({
        x: oldViewBox.x + diffX,
        y: oldViewBox.y + diffY,
      });
      this.handleViewBoxChange(newViewBox, newZoom);
    };

    takeSnapshot() {
      const project = this.project();
      if (!project || !this.state.modelName) {
        return;
      }
      const { modelName } = this.state;

      const [svg, viewbox] = renderSvgToString(project, modelName);
      const osCanvas = document.createElement('canvas');
      osCanvas.width = viewbox.width * 4;
      osCanvas.height = viewbox.height * 4;
      const ctx = exists(osCanvas.getContext('2d'));
      const svgBlob = new Blob([svg], { type: 'image/svg+xml;charset=utf-8' });
      const svgUrl = URL.createObjectURL(svgBlob);

      const image = new Image();
      image.onload = () => {
        ctx.drawImage(image, 0, 0, viewbox.width * 4, viewbox.height * 4);

        osCanvas.toBlob((snapshotBlob) => {
          if (snapshotBlob) {
            this.setState({ snapshotBlob });
          } else {
            this.setState({
              modelErrors: this.state.modelErrors.push(new Error('snapshot creation failed (1).')),
            });
          }
        });
      };
      image.onerror = () => {
        this.setState({
          modelErrors: this.state.modelErrors.push(new Error('snapshot creation failed (2).')),
        });
      };

      image.src = svgUrl;
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

      const undoEnabled =
        this.state.projectHistory.size > 1 && this.state.projectOffset < this.state.projectHistory.size - 1;
      const redoEnabled = this.state.projectOffset > 0;

      return (
        <div className="simlin-editor-undoredobar">
          <UndoRedoBar undoEnabled={undoEnabled} redoEnabled={redoEnabled} onUndoRedo={this.handleUndoRedo} />
          {/*<Snapshotter onSnapshot={this.handleSnapshot} />*/}
          <ZoomBar zoom={zoom} onChangeZoom={this.handleZoomChange} />
        </div>
      );
    }

    getEditorControls() {
      const { embedded } = this.props;
      const { dialOpen, dialVisible, selectedTool } = this.state;

      if (embedded) {
        return undefined;
      }

      return (
        <SpeedDial
          ariaLabel="hide or show editor tools"
          className="simlin-editor-speeddial"
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
            className={selectedTool === 'stock' ? 'simlin-editor-selectedtool' : undefined}
          />
          <SpeedDialAction
            icon={<FlowIcon />}
            title="Flow"
            onClick={this.handleSelectFlow}
            className={selectedTool === 'flow' ? 'simlin-editor-selectedtool' : undefined}
          />
          <SpeedDialAction
            icon={<AuxIcon />}
            title="Variable"
            onClick={this.handleSelectAux}
            className={selectedTool === 'aux' ? 'simlin-editor-selectedtool' : undefined}
          />
          <SpeedDialAction
            icon={<LinkIcon />}
            title="Link"
            onClick={this.handleSelectLink}
            className={selectedTool === 'link' ? 'simlin-editor-selectedtool' : undefined}
          />
        </SpeedDial>
      );
    }

    getSnapshot() {
      const { embedded } = this.props;
      const { snapshotBlob } = this.state;

      if (embedded || !snapshotBlob) {
        return undefined;
      }

      return (
        <Card className="simlin-editor-snapshotcard" elevation={2}>
          <CardContent>
            <img src={URL.createObjectURL(snapshotBlob)} className="simlin-editor-snapshotimg" alt="diagram snapshot" />
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

    render(): React.ReactNode {
      const { embedded, className } = this.props;

      const classNames = clsx(className, 'simlin-editor', embedded ? '' : 'simlin-editor-bg');

      return (
        <div className={classNames}>
          {this.getDrawer()}
          {this.getDetails()}
          {this.getSearchBar()}
          {this.getCanvas()}
          {this.getSnackbar()}
          {this.getEditorControls()}
          {this.getMetaActionsBar()}
          {this.getSnapshot()}
        </div>
      );
    }
  },
)(({ theme }) => ({
  '.simlin-editor-undoredobar': {
    display: 'flex',
    position: 'absolute',
    bottom: theme.spacing(3),
    left: theme.spacing(10),
  },
  '.simlin-editor-speeddial': {
    position: 'absolute',
    bottom: theme.spacing(2),
    left: theme.spacing(1.5),
  },
  '.simlin-editor-snapshotcard': {
    position: 'absolute',
    top: 0,
    left: 0,
    width: 240,
    marginTop: 12,
    marginLeft: 12,
  },
  '.simlin-editor-snapshotimg': {
    width: '100%',
    objectFit: 'scale-down',
  },
  '.simlin-editor-searchbox': {
    position: 'relative',
    top: 0,
    left: 0,
    paddingLeft: 52,
    paddingRight: 64,
    paddingTop: 8,
    border: 0,
  },
  '.simlin-editor-menubutton': {
    marginLeft: 4,
    position: 'absolute',
    zIndex: 100,
    left: 0,
    top: 0,
    display: 'block',
    color: '#666',
  },
  '.simlin-editor-searchbar': {
    position: 'absolute',
    top: 8,
    right: 8,
    height: 48,
    [theme.breakpoints.up('lg')]: {
      width: SearchbarWidthLg,
    },
    [theme.breakpoints.between('md', 'lg')]: {
      width: SearchbarWidthMd,
    },
    [theme.breakpoints.down('md')]: {
      width: SearchbarWidthSm,
    },
  },
  '.simlin-editor-searchbarmd': {
    position: 'absolute',
    top: 8,
    right: 8,
    height: 48,
    width: SearchbarWidthSm,
  },
  '.simlin-editor-vardetails': {
    position: 'absolute',
    top: 8,
    right: 8,
  },
  '.simlin-editor-searchbutton': {
    color: '#aaa',
  },
  '.simlin-editor-clearsearchbutton': {
    color: '#aaa',
    cursor: 'pointer',
  },
  '.simlin-editor-divider': {
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
  '&.simlin-editor': {
    boxSizing: 'border-box',
    overflow: 'hidden',
  },
  '&.simlin-editor-bg': {
    width: '100%',
    height: '100%',
  },
  '.simlin-editor-selectedtool': {
    backgroundColor: theme.palette.secondary.main,
  },
  // https://github.com/mui-org/mui/issues/19692
  '@global': {
    '.MuiAutocomplete-option[data-focus="true"]': {
      background: '#ADD8E6',
    },
  },
}));
