// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

import { Project as EngineProject } from '@simlin/engine';

function loadDefaultProject(name: string): string {
  const modelPath = path.join(__dirname, '..', '..', '..', 'default_projects', name, 'model.xmile');
  if (!fs.existsSync(modelPath)) {
    throw new Error(`Default project model not found: ${modelPath}`);
  }
  return fs.readFileSync(modelPath, 'utf8');
}

// Simulate the server's preview generation pipeline:
// XMILE -> engine -> protobuf -> engine -> PNG
async function generatePreview(modelName: string): Promise<Uint8Array> {
  const xmile = loadDefaultProject(modelName);

  // Step 1: Import from XMILE and serialize to protobuf (same as new-user.ts)
  const importProject = await EngineProject.open(xmile);
  const protobuf = await importProject.serializeProtobuf();
  await importProject.dispose();

  // Step 2: Load from protobuf and render PNG (same as render.ts)
  const engineProject = await EngineProject.openProtobuf(protobuf);
  try {
    return await engineProject.renderPng('main', 800);
  } finally {
    await engineProject.dispose();
  }
}

describe('model preview rendering', () => {
  it('population model generates a valid PNG', async () => {
    const png = await generatePreview('population');

    expect(png).toBeInstanceOf(Uint8Array);
    expect(png.length).toBeGreaterThan(100);

    // Verify PNG signature
    expect(png[0]).toBe(137);
    expect(png[1]).toBe(80); // P
    expect(png[2]).toBe(78); // N
    expect(png[3]).toBe(71); // G
  });
});
