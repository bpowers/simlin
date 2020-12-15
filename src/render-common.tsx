// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// eslint-disable @typescript-eslint/no-empty-function

import * as React from 'react';

import { Map, Set } from 'immutable';
import { renderToString } from 'react-dom/server';

import { Project } from './engine/project';
import { UID, ViewElement } from './engine/xmile';

import { createMuiTheme } from '@material-ui/core/styles';
import { ServerStyleSheets, ThemeProvider } from '@material-ui/styles';

import { Project as DmProject } from './app/datamodel';
import { defined, exists, Series } from './app/common';
import { Canvas } from './app/model/drawing/Canvas';
import { Box, Point } from './app/model/drawing/common';

const theme = createMuiTheme({
  palette: {},
});

export function renderSvgToString(
  project: Project,
  dmProject: DmProject,
  modelName: string,
  data?: Map<string, Series>,
): [string, Box] {
  const model = defined(project.model(modelName));
  const dmModel = defined(dmProject.models.get(modelName));

  if (!data) {
    data = Map<string, Series>();
  }

  const renameVariable = (_oldName: string, _newName: string): void => {};
  const onSelection = (_selected: Set<UID>): void => {};
  const moveSelection = (_position: Point): void => {};
  const moveFlow = (_element: ViewElement, _target: number, _position: Point): void => {};
  const moveLabel = (_uid: UID, _side: 'top' | 'left' | 'bottom' | 'right'): void => {};
  const attachLink = (_element: ViewElement, _to: string): void => {};
  const createCb = (_element: ViewElement): void => {};
  const nullCb = (): void => {};

  const sheets = new ServerStyleSheets();

  const canvasElement = (
    <Canvas
      embedded={true}
      project={defined(project)}
      dmProject={dmProject}
      model={model}
      dmModel={dmModel}
      view={defined(model.view(0))}
      dmView={defined(dmModel.views.get(0))}
      data={data}
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
    />
  );

  let svg = renderToString(sheets.collect(<ThemeProvider theme={theme}>{canvasElement}</ThemeProvider>));

  // eslint-disable-next-line @typescript-eslint/prefer-regexp-exec
  const viewboxStr = exists(svg.match(/viewBox="[^"]*"/))[0]
    .split('"')[1]
    .trim();
  const viewboxParts = viewboxStr.split(' ').map(Number);
  const width = viewboxParts[2];
  const height = viewboxParts[3];

  const styles = `<style>\n${sheets.toString()}\n</style>\n<defs>\n`;

  svg = svg.replace('<svg ', `<svg style="width: ${width}; height: ${height};" `);
  svg = svg.replace(/<defs[^>]*>/, styles);
  svg = svg.replace(/^<div[^>]*>/, '');
  svg = svg.replace(/<\/div>$/, '');

  return [svg, { width, height }];
}
