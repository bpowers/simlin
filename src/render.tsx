// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as logger from 'winston';

// import { isMainThread, parentPort, Worker, workerData } from 'worker_threads';

import * as canvas from 'canvas';
import { Canvg, presets } from 'canvg';

import fetch from 'node-fetch';
import { DOMParser } from 'xmldom';

import { exists } from './engine/common';
import { stdProject } from './engine/project';
import { FileFromJSON } from './engine/xmile';

import { defined } from './app/common';
import { renderSvgToString } from './render-common';
import { File } from './schemas/file_pb';

const preset = presets.node({
  DOMParser,
  canvas,
  fetch: fetch as any,
});

export async function renderToPNG(fileDoc: File): Promise<Buffer> {
  const sdFile = FileFromJSON(JSON.parse(fileDoc.getJsonContents()));

  const [sdProject, err2] = stdProject.addFile(defined(sdFile) as any);
  if (err2) {
    throw new Error(`stdProject.addFile: ${err2.message}`);
  }

  const [svg, viewbox] = renderSvgToString(defined(sdProject), 'main');

  canvas.registerFont('fonts/Roboto-Light.ttf', { family: 'Roboto' });

  const scale = Math.ceil((2 * 800) / viewbox.width);
  logger.error(`scale ${scale} (w:${viewbox.width * scale})`);
  const c = canvas.createCanvas(viewbox.width * scale, viewbox.height * scale);
  const ctx = c.getContext('2d');
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
  const cvg = Canvg.fromString(
    exists(ctx),
    svg,
    Object.assign({}, preset, {
      ignoreMouse: true,
      ignoreAnimation: true,
      // ignoreDimensions: false,
    }),
  );

  await cvg.render();

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
