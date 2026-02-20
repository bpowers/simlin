// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Project as EngineProject } from '@simlin/engine';

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
  it('renders PNG with explicit width preserving aspect ratio', async () => {
    // Create a minimal project with a single aux to get a non-empty diagram
    const projectJson = JSON.stringify({
      name: 'test',
      simSpecs: { startTime: 0, endTime: 10, dt: '1' },
      models: [
        {
          name: 'main',
          stocks: [],
          flows: [],
          auxiliaries: [{ name: 'x', equation: '1' }],
          views: [
            {
              elements: [{ type: 'aux', uid: 1, name: 'x', x: 100, y: 100 }],
            },
          ],
        },
      ],
    });

    const project = await EngineProject.openJson(projectJson);
    const intrinsicPng = await project.renderPng('main');
    const scaledPng = await project.renderPng('main', 400);
    await project.dispose();

    expect(intrinsicPng.length).toBeGreaterThan(0);
    expect(scaledPng.length).toBeGreaterThan(0);

    const intrinsicDims = readPngDimensions(intrinsicPng);
    const scaledDims = readPngDimensions(scaledPng);

    expect(scaledDims.width).toBe(400);

    // Aspect ratio should be preserved
    const intrinsicRatio = intrinsicDims.width / intrinsicDims.height;
    const scaledRatio = scaledDims.width / scaledDims.height;
    expect(Math.abs(intrinsicRatio - scaledRatio)).toBeLessThan(0.05);
  });
});
