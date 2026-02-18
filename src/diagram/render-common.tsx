// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// eslint-disable @typescript-eslint/no-empty-function

import * as React from 'react';

import { renderToString } from 'react-dom/server';

import { UID, ViewElement, Project } from '@simlin/core/datamodel';

import { at, getOrThrow } from '@simlin/core/collections';
import { Canvas } from './drawing/Canvas';
import { Box, Point } from './drawing/common';
import { renderStyles } from './drawing/render-styles';

export function renderSvgToString(project: Project, modelName: string): [string, Box] {
  const model = getOrThrow(project.models, modelName);

  const renameVariable = (_oldName: string, _newName: string): void => {};
  const onSelection = (_selected: ReadonlySet<UID>): void => {};
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
      selection={new Set()}
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

  // Render the canvas to an SVG string
  let svg = renderToString(canvasElement);

  // Extract dimensions from viewBox
  let width = 100;
  let height = 100;
  const viewboxMatch = svg.match(/viewBox="[^"]*"/);
  if (viewboxMatch) {
    const viewboxStr = viewboxMatch[0].split('"')[1].trim();
    const viewboxParts = viewboxStr.split(' ').map(Number);
    width = viewboxParts[2];
    height = viewboxParts[3];
  }

  // Extract root class from wrapper div if present
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

  // Remove wrapper div tags
  svg = svg.replace(/^<div[^>]*>/, '');
  svg = svg.replace(/<\/div>$/, '');

  // Add root class and SVG attributes
  if (rootClass) {
    svg = svg.replace('class="', `class="${rootClass} `);
  }
  svg = svg.replace('<svg ', `<svg style="width: ${width}; height: ${height};" xmlns="http://www.w3.org/2000/svg" `);

  // Inject our static CSS styles into the SVG defs section
  const styles = `<style>\n${renderStyles}\n</style>\n<defs>\n`;
  svg = svg.replace(/<defs[^>]*>/, styles);

  // Strip CSS module class names (keep only simlin-* classes), deduplicate
  svg = svg.replace(/class="([^"]*)"/g, (_match: string, classes: string) => {
    const filtered = classes.split(' ').filter((c: string) => c.startsWith('simlin-'));
    const seen: Record<string, boolean> = {};
    const unique = filtered.filter((c: string) => {
      if (seen[c]) return false;
      seen[c] = true;
      return true;
    });
    const simlinClasses = unique.join(' ');
    return simlinClasses ? `class="${simlinClasses}"` : '';
  });
  // Remove empty class attributes left over from stripping
  svg = svg.replace(/ class=""/g, '');

  return [svg, { width, height }];
}
