// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

// import { isMainThread, parentPort, Worker, workerData } from 'worker_threads';

import * as canvas from 'canvas';
import * as canvg from 'canvg';
import { Map, Set } from 'immutable';
import { renderToString } from 'react-dom/server';

import { stdProject } from './engine/project';
import { FileFromJSON, UID, ViewElement } from './engine/xmile';

import { createMuiTheme } from '@material-ui/core/styles';
import { ServerStyleSheets, ThemeProvider } from '@material-ui/styles';

import { defined, exists, Series } from './app/common';
import { Canvas } from './app/model/drawing/Canvas';
import { Point } from './app/model/drawing/common';
import { FileDocument } from './models/file';

const theme = createMuiTheme({
  palette: {},
});

export async function renderToPNG(fileDoc: FileDocument): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    try {
      const png = renderToPNGSync(fileDoc);
      resolve(png);
    } catch (err) {
      reject(err);
    }
    // const worker = new Worker(__filename, {
    //   workerData: fileDoc,
    // });
    // worker.on('message', resolve);
    // worker.on('error', reject);
    // worker.on('exit', code => {
    //   if (code !== 0) {
    //     reject(new Error(`Worker stopped with exit code ${code}`));
    //   }
    // });
  });
}

function renderToPNGSync(fileDoc: FileDocument): Buffer {
  const sdFile = FileFromJSON(JSON.parse(fileDoc.contents));

  const [sdProject, err2] = stdProject.addFile(defined(sdFile) as any);
  if (err2) {
    throw new Error(`stdProject.addFile: ${err2.message}`);
  }

  const model = defined(defined(sdProject).model('main'));

  const renameVariable = (oldName: string, newName: string) => {};
  const onSelection = (selected: Set<UID>) => {};
  const moveSelection = (position: Point) => {};
  const moveFlow = (element: ViewElement, target: number, position: Point) => {};
  const attachLink = (element: ViewElement, to: string) => {};
  const createCb = (element: ViewElement) => {};
  const nullCb = () => {};

  const sheets = new ServerStyleSheets();

  const canvasElement = (
    <Canvas
      embedded={true}
      project={defined(sdProject)}
      model={model}
      view={defined(model.view(0))}
      data={Map<string, Series>()}
      selectedTool={undefined}
      selection={Set()}
      onRenameVariable={renameVariable}
      onSetSelection={onSelection}
      onMoveSelection={moveSelection}
      onMoveFlow={moveFlow}
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
  const canvasWidth = w;
  const canvasHeight = h;

  const styles = `<style>\n${sheets.toString()}\n</style>\n<defs>\n`;

  svg = svg.replace('<svg ', `<svg style="width: ${canvasWidth}; height: ${canvasHeight};" `);
  svg = svg.replace(/<defs[^>]*>/, styles);
  svg = svg.replace(/^<div[^>]*>/, '');
  svg = svg.replace(/<\/div>$/, '');

  canvas.registerFont('fonts/Roboto-Light.ttf', { family: 'Roboto' });

  const c = canvas.createCanvas(canvasWidth, canvasHeight);
  // const ctx = c.getContext('2d');
  // (ctx as any).parentNode = {
  //   clientWidth: w,
  //   clientHeight: h,
  // };

  // const img = new canvas.Image();
  // img.src = `data:image/svg+xml;utf8,${svg}`;
  // if (!img.complete) {
  //   return [undefined, new Error(`expected Image to be complete w/ data URI`)];
  // }

  // ctx.drawImage(img, 0, 0);

  // console.log(`!! img complete: ${img.complete}`);
  //
  canvg(c as any, svg, {
    ignoreMouse: true,
    ignoreAnimation: true,
    // ignoreDimensions: false,
  });

  const pngBuf = c.toBuffer('image/png', {
    compressionLevel: 6,
    filters: c.PNG_ALL_FILTERS,
    resolution: 192,
  });

  return pngBuf;
}

// TODO: revisit whe this works.
// if (!isMainThread) {
//   console.log('HELLO FROM WORKER');
//   const fileDoc: FileDocument = workerData;
//   const png = renderToPNGSync(fileDoc);
//   exists(parentPort).postMessage(png);
// }
