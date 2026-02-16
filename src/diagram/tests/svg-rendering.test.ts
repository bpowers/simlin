/**
 * @jest-environment node
 */

// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

import { init, reset } from '@simlin/engine/internal/wasm';
import { simlin_project_open_xmile, simlin_project_render_svg } from '../../engine/src/internal/import-export';
import { simlin_project_serialize_json, simlin_project_unref } from '../../engine/src/internal/project';
import { SimlinJsonFormat } from '../../engine/src/internal/types';
import { Project } from '@simlin/core/datamodel';
import { renderSvgToString } from '../render-common';

function loadXmile(relativePath: string): Uint8Array {
  const fullPath = path.join(__dirname, '..', '..', '..', relativePath);
  if (!fs.existsSync(fullPath)) {
    throw new Error(`Required test model not found: ${fullPath}`);
  }
  return fs.readFileSync(fullPath);
}

describe('SVG rendering cross-language comparison', () => {
  beforeAll(async () => {
    const wasmPath = path.join(__dirname, '..', '..', 'engine', 'core', 'libsimlin.wasm');
    if (!fs.existsSync(wasmPath)) {
      throw new Error(`WASM module not found at ${wasmPath}. Run build.sh first.`);
    }
    const wasmBuffer = fs.readFileSync(wasmPath);
    reset();
    await init(wasmBuffer);
  });

  const testModels = [
    'test/test-models/samples/teacup/teacup_w_diagram.xmile',
    'test/test-models/samples/SIR/SIR.xmile',
    'test/alias1/alias1.stmx',
    'test/test-models/samples/bpowers-hares_and_lynxes_modules/model.stmx',
    'test/arrays1/arrays.stmx',
  ];

  for (const modelFile of testModels) {
    it(`produces identical SVG for ${path.basename(modelFile)}`, () => {
      const xmileData = loadXmile(modelFile);

      // Rust rendering via WASM
      const projectPtr = simlin_project_open_xmile(xmileData);

      let rustSvg: string;
      try {
        const svgBytes = simlin_project_render_svg(projectPtr, 'main');
        rustSvg = new TextDecoder().decode(svgBytes);
      } finally {
        simlin_project_unref(projectPtr);
      }

      // TypeScript rendering via React
      const projectPtr2 = simlin_project_open_xmile(xmileData);
      let tsSvg: string;
      try {
        const jsonBytes = simlin_project_serialize_json(projectPtr2, SimlinJsonFormat.Native);
        const jsonStr = new TextDecoder().decode(jsonBytes);
        const jsonProject = JSON.parse(jsonStr);
        const tsProject = Project.fromJson(jsonProject);
        const [svg] = renderSvgToString(tsProject, 'main');
        tsSvg = svg;
      } finally {
        simlin_project_unref(projectPtr2);
      }

      expect(rustSvg).toBe(tsSvg);
    });
  }
});
