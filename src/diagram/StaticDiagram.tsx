// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import { renderToString } from 'react-dom/server';
import { toUint8Array } from 'js-base64';
import { Map, Set } from 'immutable';

import './theme.css';

import { Series } from '@simlin/core/common';
import { at, getOrThrow } from '@simlin/core/collections';
import { UID, ViewElement, Project } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';
import type { JsonProject } from '@simlin/engine';
import { Point } from './drawing/common';
import { Canvas } from './drawing/Canvas';

interface DiagramProps {
  isDarkTheme?: boolean;
  projectPbBase64: string;
  project?: Project; // Pre-loaded project for SSR
  data?: Map<string, Series>;
}

interface DiagramState {
  project: Project | undefined;
}

export class StaticDiagram extends React.PureComponent<DiagramProps, DiagramState> {
  constructor(props: DiagramProps) {
    super(props);

    // Use pre-loaded project if provided (for SSR), otherwise undefined
    let project = props.project;
    if (project && props.data !== undefined) {
      project = project.attachData(props.data, 'main');
    }

    this.state = {
      project,
    };
  }

  componentDidMount() {
    // Only load if we don't already have a project (i.e., not SSR)
    if (!this.state.project) {
      this.loadProject();
    }
  }

  async loadProject() {
    const serializedProject = toUint8Array(this.props.projectPbBase64);
    const engineProject = await EngineProject.openProtobuf(serializedProject);
    const json = JSON.parse(await engineProject.serializeJson()) as JsonProject;
    let project = Project.fromJson(json);
    await engineProject.dispose();

    if (this.props.data !== undefined) {
      project = project.attachData(this.props.data, 'main');
    }

    this.setState({
      project,
    });
  }

  render(): React.ReactNode {
    const { project } = this.state;
    if (!project) {
      return null;
    }

    const canUseDOM = !!(typeof window !== 'undefined' && window.document && window.document.createElement);
    const isDarkTheme = this.props.isDarkTheme;

    const model = getOrThrow(project.models, 'main');

    const renameVariable = (_oldName: string, _newName: string): void => {};
    const onSelection = (_selected: Set<UID>): void => {};
    const moveSelection = (_position: Point): void => {};
    const moveFlow = (_element: ViewElement, _target: number, _position: Point): void => {};
    const moveLabel = (_uid: UID, _side: 'top' | 'left' | 'bottom' | 'right'): void => {};
    const attachLink = (_element: ViewElement, _to: string): void => {};
    const createCb = (_element: ViewElement): void => {};
    const nullCb = (): void => {};

    const canvasElement = (
      <Canvas
        embedded={true}
        project={project}
        model={model}
        view={at(model.views, 0)}
        version={1}
        selectedTool={undefined}
        selection={Set()}
        onRenameVariable={renameVariable}
        onSetSelection={onSelection}
        onMoveSelection={moveSelection}
        onMoveFlow={moveFlow}
        onMoveLabel={moveLabel}
        onAttachLink={attachLink}
        onCreateVariable={createCb}
        onClearSelectedTool={nullCb}
        onDeleteSelection={nullCb}
        onShowVariableDetails={nullCb}
        onViewBoxChange={nullCb}
      />
    );

    const themedCanvas = <div data-theme={isDarkTheme ? 'dark' : undefined}>{canvasElement}</div>;

    if (canUseDOM) {
      return themedCanvas;
    } else {
      renderToString(themedCanvas);
      return <>{themedCanvas}</>;
    }
  }
}
