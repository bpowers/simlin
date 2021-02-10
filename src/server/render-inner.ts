// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as canvas from 'canvas';
import { Canvg, presets } from 'canvg';

import { DOMParser } from 'xmldom';

import { exists } from '@system-dynamics/core/common';

function fakeFetch(_input: any, _config?: any): Promise<any> {
  throw new Error('no fetching from SVGs');
}

const preset = presets.node({
  DOMParser,
  canvas,
  fetch: fakeFetch,
});

interface Box {
  readonly width: number;
  readonly height: number;
}

export async function renderToPNG(svgString: string, viewbox: Box): Promise<Uint8Array> {
  canvas.registerFont('fonts/Roboto-Light.ttf', { family: 'Roboto' });

  const retina = 2; // double the pixels for the same unit of measurement
  const maxPreviewWidth = 400;
  let scale = (maxPreviewWidth * retina) / viewbox.width;
  if (scale > 1) {
    scale = Math.ceil(scale);
  }
  // console.log(`scale ${scale} (w:${viewbox.width * scale})`);
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
    svgString,
    Object.assign({}, preset, {
      window: undefined,
      ignoreMouse: true,
      ignoreAnimation: true,
      // ignoreDimensions: false,
    }),
  );

  await cvg.render();

  return c.toBuffer('image/png', {
    compressionLevel: 6,
    filters: c.PNG_ALL_FILTERS,
    resolution: 192,
  });
}