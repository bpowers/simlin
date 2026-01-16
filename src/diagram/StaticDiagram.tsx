// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import { renderToString } from 'react-dom/server';
import { toUint8Array } from 'js-base64';
import { Map, Set } from 'immutable';
import { createTheme, ThemeProvider } from '@mui/material/styles';

import { defined, Series } from '@system-dynamics/core/common';
import { UID, ViewElement, Project } from '@system-dynamics/core/datamodel';
import { Point } from './drawing/common.js';
import { Canvas } from './drawing/Canvas.js';

interface DiagramProps {
  isDarkTheme?: boolean;
  projectPbBase64: string;
  data?: Map<string, Series>;
}

interface DiagramState {
  project: Project;
}

export class StaticDiagram extends React.PureComponent<DiagramProps, DiagramState> {
  constructor(props: DiagramProps) {
    super(props);

    const serializedProject = toUint8Array(this.props.projectPbBase64);
    let project = Project.deserializeBinary(serializedProject);
    if (props.data !== undefined) {
      project = project.attachData(props.data, 'main');
    }

    this.state = {
      project,
    };
  }

  render(): React.ReactNode {
    const canUseDOM = !!(typeof window !== 'undefined' && window.document && window.document.createElement);
    const isDarkTheme = this.props.isDarkTheme;
    const theme = createTheme({
      palette: {
        mode: isDarkTheme ? 'dark' : 'light',
        common: {
          white: isDarkTheme ? '#222222' : '#ffffff',
          black: isDarkTheme ? '#bbbbbb' : '#000000',
        },
      },
    });

    const model = defined(this.state.project.models.get('main'));

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
        project={this.state.project}
        model={model}
        view={defined(model.views.get(0))}
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

    const themedCanvas = <ThemeProvider theme={theme}>{canvasElement}</ThemeProvider>;

    if (canUseDOM) {
      return themedCanvas;
    } else {
      renderToString(themedCanvas);
      return <>{themedCanvas}</>;
    }
  }
}
