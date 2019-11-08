// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Map, Set } from 'immutable';
import { renderToString } from 'react-dom/server';

import { Project } from './engine/project';
import { UID, ViewElement } from './engine/xmile';

import { createMuiTheme } from '@material-ui/core/styles';
import { ServerStyleSheets, ThemeProvider } from '@material-ui/styles';

import { defined, exists, Series } from './app/common';
import { Canvas } from './app/model/drawing/Canvas';
import { Box, Point } from './app/model/drawing/common';

const theme = createMuiTheme({
  palette: {},
});

export function renderSvgToString(project: Project, modelName: string, data?: Map<string, Series>): [string, Box] {
  const model = defined(project.model(modelName));

  if (!data) {
    data = Map<string, Series>();
  }

  const renameVariable = (oldName: string, newName: string) => {};
  const onSelection = (selected: Set<UID>) => {};
  const moveSelection = (position: Point) => {};
  const moveFlow = (element: ViewElement, target: number, position: Point) => {};
  const moveLabel = (uid: UID, side: 'top' | 'left' | 'bottom' | 'right') => {};
  const attachLink = (element: ViewElement, to: string) => {};
  const createCb = (element: ViewElement) => {};
  const nullCb = () => {};

  const sheets = new ServerStyleSheets();

  const canvasElement = (
    <Canvas
      embedded={true}
      project={defined(project)}
      model={model}
      view={defined(model.view(0))}
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

  const viewboxStr = exists(svg.match(/viewBox=\"[^"]*\"/))[0]
    .split('"')[1]
    .trim();
  const [x, y, w, h] = viewboxStr.split(' ').map(Number);
  const width = w;
  const height = h;

  const styles = `<style>\n${sheets.toString()}\n</style>\n<defs>\n`;

  svg = svg.replace('<svg ', `<svg style="width: ${width}; height: ${height};" `);
  svg = svg.replace(/<defs[^>]*>/, styles);
  svg = svg.replace(/^<div[^>]*>/, '');
  svg = svg.replace(/<\/div>$/, '');

  return [svg, { width, height }];
}
