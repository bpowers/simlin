// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// eslint-disable @typescript-eslint/no-empty-function

import * as React from 'react';

import { Set } from 'immutable';
import { renderToString } from 'react-dom/server';

import { UID, ViewElement, Project } from '@system-dynamics/core/datamodel';

import { createMuiTheme } from '@material-ui/core/styles';
import { ServerStyleSheets, ThemeProvider } from '@material-ui/styles';

import { defined } from '@system-dynamics/core/common';
import { Canvas } from './drawing/Canvas';
import { Box, Point } from './drawing/common';

const theme = createMuiTheme({
  palette: {},
});

export function renderSvgToString(project: Project, modelName: string): [string, Box] {
  const model = defined(project.models.get(modelName));

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
      project={project}
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

  let svg = renderToString(sheets.collect(<ThemeProvider theme={theme}>{canvasElement}</ThemeProvider>));

  let width = 100;
  let height = 100;
  // eslint-disable-next-line @typescript-eslint/prefer-regexp-exec
  const viewboxMatch = svg.match(/viewBox="[^"]*"/);
  if (viewboxMatch) {
    const viewboxStr = viewboxMatch[0].split('"')[1].trim();
    const viewboxParts = viewboxStr.split(' ').map(Number);
    width = viewboxParts[2];
    height = viewboxParts[3];
  }

  const styles = `<style>\n${sheets.toString()}\n</style>\n<defs>\n`;

  svg = svg.replace('<svg ', `<svg style="width: ${width}; height: ${height};" xmlns="http://www.w3.org/2000/svg" `);
  svg = svg.replace(/<defs[^>]*>/, styles);
  svg = svg.replace(/^<div[^>]*>/, '');
  svg = svg.replace(/<\/div>$/, '');

  return [svg, { width, height }];
}
