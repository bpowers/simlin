// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

import { Project as EngineProject } from '@simlin/engine';
import { JsonProject } from '@simlin/engine';
import { Project as DmProject } from '@simlin/core/datamodel';
import { renderSvgToString } from '@simlin/diagram/render-common';
import { renderToPNG } from '../render-inner';

function loadDefaultProject(name: string): string {
  const modelPath = path.join(__dirname, '..', '..', '..', 'default_projects', name, 'model.xmile');
  if (!fs.existsSync(modelPath)) {
    throw new Error(`Default project model not found: ${modelPath}`);
  }
  return fs.readFileSync(modelPath, 'utf8');
}

// Simulate the server's preview generation pipeline:
// XMILE -> engine -> protobuf -> engine -> JSON -> DataModel -> SVG -> PNG
async function generatePreview(modelName: string): Promise<{ svg: string; png: Uint8Array; viewbox: { width: number; height: number } }> {
  const xmile = loadDefaultProject(modelName);

  // Step 1: Import from XMILE and serialize to protobuf (same as new-user.ts)
  const importProject = await EngineProject.open(xmile);
  const protobuf = await importProject.serializeProtobuf();
  await importProject.dispose();

  // Step 2: Load from protobuf and serialize to JSON (same as render.ts)
  const engineProject = await EngineProject.openProtobuf(protobuf);
  const json = JSON.parse(await engineProject.serializeJson()) as JsonProject;
  const project = DmProject.fromJson(json);
  await engineProject.dispose();

  // Step 3: Render to SVG
  const [svgString, viewbox] = renderSvgToString(project, 'main');

  // Step 4: Convert to PNG
  const png = await renderToPNG(svgString, viewbox);

  return { svg: svgString, png, viewbox };
}

function stripTextElements(svg: string): string {
  return svg.replace(/<text[^>]*>[\s\S]*?<\/text>/g, '');
}

describe('model preview rendering', () => {
  it('population model text is actually rendered in PNG', async () => {
    const { svg, viewbox } = await generatePreview('population');

    // Verify SVG has text content
    expect(svg).toContain('>population<');
    expect(svg).toContain('>births<');

    // Render the full SVG and a version with text stripped
    const pngWithText = await renderToPNG(svg, viewbox);
    const svgNoText = stripTextElements(svg);
    const pngWithoutText = await renderToPNG(svgNoText, viewbox);

    // PNG with text should be meaningfully larger than without text,
    // proving text is actually being rendered (not just present in SVG source)
    const sizeDiff = pngWithText.length - pngWithoutText.length;
    expect(sizeDiff).toBeGreaterThan(500);
  });
});
