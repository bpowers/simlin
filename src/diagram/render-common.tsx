// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// eslint-disable @typescript-eslint/no-empty-function

import * as React from 'react';

import { Set } from 'immutable';
import { renderToString } from 'react-dom/server';

import { UID, ViewElement, Project } from '@system-dynamics/core/datamodel';

import { createTheme, ThemeProvider } from '@mui/material/styles';

import { defined } from '@system-dynamics/core/common';
import { Canvas } from './drawing/Canvas';
import { Box, Point } from './drawing/common';

const theme = createTheme({
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

  // material ui returns two tags: the <style> tag, then the <svg>
  let svg = renderToString(<ThemeProvider theme={theme}>{canvasElement}</ThemeProvider>);
  let contents = '';

  // our svg is wrapped in a div, which is handled below.
  const divStart = svg.indexOf('<div');
  if (divStart > 0) {
    let svgTag = svg.slice(0, divStart);
    svgTag = svgTag.replace(/<style[^>]*>/, '');
    svgTag = svgTag.replace(/<\/style>/, '');
    contents += svgTag;
    contents += '\n';
    svg = svg.slice(divStart);
  }

  const origSvg = svg;
  let consumedLen = 0;
  svg = '';
  const styleRe = /<style.*?<\/style>/g;
  for (const match of origSvg.matchAll(styleRe)) {
    let svgTag = match[0];
    const svgTagLen = svgTag.length;
    svgTag = svgTag.replace(/<style[^>]*>/, '');
    svgTag = svgTag.replace(/<\/style>/, '');
    contents += svgTag;
    contents += '\n';
    svg += origSvg.slice(consumedLen, match.index);
    consumedLen = (match.index || 0) + svgTagLen;
  }
  svg += origSvg.slice(consumedLen);

  const styles = `<style>\n${contents}\n</style>\n<defs>\n`;

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

  let rootClass = '';
  const svgStart = svg.indexOf('<svg');
  if (svgStart > 0) {
    const divTag = svg.slice(0, svgStart);
    const match = /class="(?<className>[^"]*)"/.exec(divTag);
    if (match && match.groups) {
      rootClass = match.groups['className'];
    }

    svg = svg.slice(svgStart);
  }

  svg = svg.replace(/^<div[^>]*>/, '');
  svg = svg.replace(/<\/div>$/, '');
  svg = svg.replace('class="', `class="${rootClass} `);
  svg = svg.replace('<svg ', `<svg style="width: ${width}; height: ${height};" xmlns="http://www.w3.org/2000/svg" `);
  svg = svg.replace(/<defs[^>]*>/, styles);

  // generate a random string like 'qaqb3rusiha'
  const prefix = Math.random().toString(36).substr(2);
  svg = svg.replace(/jss/g, 'simlin-' + prefix);

  return [svg, { width, height }];
}
