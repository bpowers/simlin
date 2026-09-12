// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect, beforeAll } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import { init, reset } from '@simlin/engine/internal/wasm';
import { simlin_project_open_xmile, simlin_project_render_svg } from '../../engine/src/internal/import-export';
import { simlin_project_serialize_json, simlin_project_unref } from '../../engine/src/internal/project';
import { SimlinJsonFormat } from '../../engine/src/internal/types';
import { Project, projectFromJson } from '@simlin/core/datamodel';
import { AuxRadius } from '../drawing/default';
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

  function expectIdenticalSvg(xmileData: Uint8Array): void {
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
      const tsProject = projectFromJson(jsonProject);
      const [svg] = renderSvgToString(tsProject, 'main');
      tsSvg = svg;
    } finally {
      simlin_project_unref(projectPtr2);
    }

    expect(rustSvg).toBe(tsSvg);
  }

  for (const modelFile of testModels) {
    it(`produces identical SVG for ${path.basename(modelFile)}`, () => {
      expectIdenticalSvg(loadXmile(modelFile));
    });
  }

  // The parity rows above cannot catch a bound both renderers leave out, so
  // the viewBox is checked against the alias itself. alias1's alias sits above
  // and left of every other node, which is where a viewBox bounded without
  // aliases cuts it off.
  it('holds an alias inside the static SVG viewBox', () => {
    const projectPtr = simlin_project_open_xmile(loadXmile('test/alias1/alias1.stmx'));
    let tsProject: Project;
    try {
      const jsonBytes = simlin_project_serialize_json(projectPtr, SimlinJsonFormat.Native);
      tsProject = projectFromJson(JSON.parse(new TextDecoder().decode(jsonBytes)));
    } finally {
      simlin_project_unref(projectPtr);
    }

    const alias = tsProject.models.get('main')?.views[0]?.elements.find((element) => element.type === 'alias');
    const [svg] = renderSvgToString(tsProject, 'main');
    const viewBox = /viewBox="([^"]*)"/.exec(svg);
    if (alias === undefined || viewBox === null) {
      throw new Error('alias1 must hold an alias and render a viewBox');
    }

    const [left, top, width, height] = viewBox[1].split(' ').map(Number);
    expect(left).toBeLessThanOrEqual(alias.x - AuxRadius);
    expect(top).toBeLessThanOrEqual(alias.y - AuxRadius);
    expect(left + width).toBeGreaterThanOrEqual(alias.x + AuxRadius);
    expect(top + height).toBeGreaterThanOrEqual(alias.y + AuxRadius);
  });

  // A center side is stored as an absent labelSide in the native JSON (the
  // engine's serialization of Center), so this is also the absent-side case.
  // None of the corpus models above uses one.
  it('produces identical SVG for elements whose label side is center', () => {
    const xmile = `<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>center labels</name><vendor>simlin</vendor><product version="1.0">simlin</product></header>
  <sim_specs><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="Level"><eqn>1</eqn></stock>
      <aux name="Constant Rate"><eqn>1</eqn></aux>
    </variables>
    <views>
      <view>
        <stock name="Level" x="200" y="100" label_side="center"/>
        <aux name="Constant Rate" x="100" y="200" label_side="center"/>
      </view>
    </views>
  </model>
</xmile>`;
    expectIdenticalSvg(new TextEncoder().encode(xmile));
  });
});
