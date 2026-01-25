// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { renderToPNG } from '../render-inner';

function readPngDimensions(png: Uint8Array): { width: number; height: number } {
  const buffer = Buffer.from(png);
  if (buffer.length < 24) {
    throw new Error('PNG data too short');
  }
  return {
    width: buffer.readUInt32BE(16),
    height: buffer.readUInt32BE(20),
  };
}

describe('renderToPNG preview scaling', () => {
  it('scales based on the larger viewBox dimension', async () => {
    const viewbox = { width: 200, height: 1000 };
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${viewbox.width} ${viewbox.height}">
  <rect x="0" y="0" width="${viewbox.width}" height="${viewbox.height}" fill="white" />
</svg>`;

    const png = await renderToPNG(svg, viewbox);
    const { width, height } = readPngDimensions(png);

    expect(width).toBe(160);
    expect(height).toBe(800);
  });
});
