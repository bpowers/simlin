// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// End-to-end regression for silent variable data-loss through the REAL engine.
//
// A pure fromJson<->toJson test (datamodel.test.ts) can pass while still using
// a wrong JSON key name, because both halves agree on the typo. These tests
// instead drive the actual Rust serializer (src/simlin-engine/src/json.rs) via
// the WASM engine, so a key name that does not match the serializer is caught:
// the engine drops the unknown field and the survival assertion fails.
//
// The scenario mirrors the editor's persistence path exactly. The editor loads
// a serialized project, builds the in-memory datamodel, and -- on any edit --
// re-serializes a variable as a FULL upsert (replace-by-UID, see
// src/simlin-engine/src/patch.rs). Any wire field datamodel.ts fails to
// round-trip is therefore dropped the moment the user edits an unrelated field.

import * as fs from 'fs';
import * as path from 'path';

import type { JsonProjectPatch } from '@simlin/engine';

import { projectFromJson, projectToJson, auxToJson, stockToJson, moduleToJson } from '../datamodel';

// This e2e drives the REAL WASM engine, so it needs `pnpm build` to have
// produced the engine package (lib/) and libsimlin.wasm. Keep `pnpm --filter
// @simlin/core test` runnable standalone on a clean checkout by SKIPPING (not
// failing) when the built engine is absent; CI and the pre-commit hook build
// first, so this integration coverage always runs there. The @simlin/engine
// VALUE import is dynamic (inside loadWasm) so a missing build does not break
// module resolution for the skipped suite -- the pure datamodel.test.ts still
// covers every field without the engine.
const wasmPath = path.join(__dirname, '..', '..', 'engine', 'core', 'libsimlin.wasm');
const engineBuilt = fs.existsSync(wasmPath);
const describeIfEngine = engineBuilt ? describe : describe.skip;
if (!engineBuilt) {
  console.warn(
    `[datamodel-roundtrip-e2e] skipping engine-backed checks: ${wasmPath} not found; run \`pnpm build\` to exercise them.`,
  );
}

type EngineModule = typeof import('@simlin/engine');
let engine: EngineModule;

async function loadWasm(): Promise<void> {
  engine = await import('@simlin/engine');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await engine.resetWasm();
  engine.configureWasm({ source: wasmBuffer });
  await engine.ready();
}

// A project whose variables carry every field that datamodel.ts previously
// dropped. Key names are camelCase to match the Rust serializer.
const BASE_PROJECT = JSON.stringify({
  name: 'roundtrip',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  dimensions: [{ name: 'region', elements: ['north', 'south'] }],
  models: [
    {
      name: 'main',
      stocks: [
        {
          name: 'level',
          inflows: [],
          outflows: [],
          initialEquation: '0',
          // [1] ACTIVE INITIAL on a stock.
          compat: { activeInitial: '5' },
        },
      ],
      flows: [],
      auxiliaries: [
        {
          name: 'imported',
          equation: '0',
          // [5] external-data reference (GET DIRECT DATA).
          compat: {
            dataSource: {
              kind: 'data',
              file: 'data.xlsx',
              tabOrDelimiter: 'Sheet1',
              rowOrCol: 'A',
              cell: 'B2',
            },
          },
        },
        {
          name: 'arrayed',
          arrayedEquation: {
            dimensions: ['region'],
            // [3] EXCEPT default equation + flag.
            equation: '99',
            hasExceptDefault: true,
            elements: [
              {
                subscript: 'north',
                equation: '1',
                // [4] per-element graphical function + per-element ACTIVE INITIAL.
                graphicalFunction: { yPoints: [0, 1, 2], xScale: { min: 0, max: 2 }, yScale: { min: 0, max: 2 } },
                compat: { activeInitial: '3' },
              },
            ],
          },
        },
      ],
      modules: [
        {
          name: 'sub_inst',
          modelName: 'sub',
          // Module compat: the engine reads canBeModuleInput, isPublic, dataSource.
          compat: {
            canBeModuleInput: true,
            isPublic: true,
            dataSource: {
              kind: 'constants',
              file: 'c.csv',
              tabOrDelimiter: ',',
              rowOrCol: '1',
              cell: 'A1',
            },
          },
        },
      ],
    },
    {
      name: 'sub',
      stocks: [],
      flows: [],
      auxiliaries: [{ name: 'out', equation: '1' }],
    },
  ],
});

interface ParsedAux {
  name: string;
  compat?: { activeInitial?: string; dataSource?: Record<string, string> };
  arrayedEquation?: {
    equation?: string;
    hasExceptDefault?: boolean;
    elements?: {
      subscript: string;
      equation: string;
      compat?: { activeInitial?: string };
      graphicalFunction?: { yPoints?: number[] };
    }[];
  };
}
interface ParsedStock {
  name: string;
  compat?: { activeInitial?: string };
}
interface ParsedModule {
  name: string;
  compat?: { canBeModuleInput?: boolean; isPublic?: boolean; dataSource?: Record<string, string> };
}
interface ParsedModel {
  name: string;
  stocks: ParsedStock[];
  auxiliaries: ParsedAux[];
  modules?: ParsedModule[];
}
interface ParsedProject {
  models: ParsedModel[];
}

async function serializeProject(json: string): Promise<ParsedProject> {
  const project = await engine.Project.openJson(json);
  try {
    return JSON.parse(await project.serializeJson()) as ParsedProject;
  } finally {
    await project.dispose();
  }
}

function findAux(parsed: ParsedProject, name: string): ParsedAux {
  const aux = parsed.models[0].auxiliaries.find((a) => a.name === name);
  if (!aux) {
    throw new Error(`aux ${name} not found`);
  }
  return aux;
}

function assertAllFieldsPresent(parsed: ParsedProject): void {
  const stock = parsed.models[0].stocks.find((s) => s.name === 'level');
  expect(stock?.compat?.activeInitial).toBe('5');

  const imported = findAux(parsed, 'imported');
  expect(imported.compat?.dataSource).toEqual({
    kind: 'data',
    file: 'data.xlsx',
    tabOrDelimiter: 'Sheet1',
    rowOrCol: 'A',
    cell: 'B2',
  });

  const arrayed = findAux(parsed, 'arrayed');
  expect(arrayed.arrayedEquation?.equation).toBe('99');
  expect(arrayed.arrayedEquation?.hasExceptDefault).toBe(true);
  const north = arrayed.arrayedEquation?.elements?.find((e) => e.subscript === 'north');
  expect(north?.graphicalFunction?.yPoints).toEqual([0, 1, 2]);
  expect(north?.compat?.activeInitial).toBe('3');

  const module = parsed.models[0].modules?.find((m) => m.name === 'sub_inst');
  expect(module?.compat?.canBeModuleInput).toBe(true);
  expect(module?.compat?.isPublic).toBe(true);
  expect(module?.compat?.dataSource).toEqual({
    kind: 'constants',
    file: 'c.csv',
    tabOrDelimiter: ',',
    rowOrCol: '1',
    cell: 'A1',
  });
}

describeIfEngine('datamodel round-trip through the real engine serializer', () => {
  beforeAll(async () => {
    await loadWasm();
  });

  it('the engine preserves the fields the editor must round-trip (validates key names)', async () => {
    // Sanity: the serializer keeps every field when the JSON uses these keys.
    assertAllFieldsPresent(await serializeProject(BASE_PROJECT));
  });

  it('survives a datamodel fromJson -> toJson cycle re-fed to the engine', async () => {
    const engineJson = await serializeProject(BASE_PROJECT);

    // The exact editor in-memory transform: engine JSON -> datamodel -> JSON.
    const datamodel = projectFromJson(engineJson as never);
    const rebuiltJson = JSON.stringify(projectToJson(datamodel));

    // Re-open the datamodel-produced JSON in the engine and re-serialize: a key
    // datamodel.ts got wrong on read OR write would be gone by now.
    assertAllFieldsPresent(await serializeProject(rebuiltJson));
  });

  it('survives a full upsert replace (the literal data-loss bug)', async () => {
    const project = await engine.Project.openJson(BASE_PROJECT);
    try {
      const datamodel = projectFromJson(JSON.parse(await project.serializeJson()) as never);
      const model = datamodel.models.get('main');
      if (!model) {
        throw new Error('main model missing');
      }

      const level = model.variables.get('level');
      const imported = model.variables.get('imported');
      const arrayed = model.variables.get('arrayed');
      const subInst = model.variables.get('sub_inst');
      if (
        level?.type !== 'stock' ||
        imported?.type !== 'aux' ||
        arrayed?.type !== 'aux' ||
        subInst?.type !== 'module'
      ) {
        throw new Error('expected variables missing');
      }

      // Rebuild each variable as a full upsert -- exactly what the editor sends
      // when the user edits any single field. allowErrors so model-validity
      // diagnostics never mask the survival assertion.
      const patch: JsonProjectPatch = {
        models: [
          {
            name: 'main',
            ops: [
              { type: 'upsertStock', payload: { stock: stockToJson(level) } },
              { type: 'upsertAux', payload: { aux: auxToJson(imported) } },
              { type: 'upsertAux', payload: { aux: auxToJson(arrayed) } },
              { type: 'upsertModule', payload: { module: moduleToJson(subInst) } },
            ],
          },
        ],
      };
      await project.applyPatch(patch, { allowErrors: true });

      assertAllFieldsPresent(JSON.parse(await project.serializeJson()) as ParsedProject);
    } finally {
      await project.dispose();
    }
  });
});
