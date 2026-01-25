// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { readFileSync } from 'fs';

import { newContext } from 'resvg-wasm';

interface Box {
  readonly width: number;
  readonly height: number;
}

export async function renderToPNG(svgString: string, viewbox: Box): Promise<Uint8Array> {
  const ctx = await newContext();
  const fontData = readFileSync('fonts/Roboto-Light.ttf');

  ctx.registerFontData(fontData);

  const retina = 2; // double the pixels for the same unit of measurement
  const maxPreviewWidth = 400;
  const maxDimension = Math.max(viewbox.width, viewbox.height);
  let scale = (maxPreviewWidth * retina) / maxDimension;
  if (scale > 1) {
    scale = Math.ceil(scale);
  }

  let pngData = ctx.render(svgString, scale, viewbox.width, viewbox.height);
  if (!pngData) {
    pngData = new Uint8Array();
  }
  return pngData;
}
