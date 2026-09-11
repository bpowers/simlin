// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The tolerant checker over real imported models, through the real WASM engine.
//
// What this establishes: for a handful of small corpus models covering the
// producers the corpus measurement grouped (Stella, Vensim via xmutil, native
// MDL, Simlin-authored XMILE), the first stock-flow view as the editor loads it
// (engine import -> serializeJson -> projectFromJson) passes tolerant mode, and
// these six also hold M1/M3 (M1/M3 are committed-view checks for the elements an
// edit routes or creates; a few other imports carry violations the editor still
// accepts, e.g. an orphan cloud in Query_file.mdl, lookup-only variables in
// test_lookups*.xmile); and strict mode reports exactly the
// imported violations each model carries, flow by flow, so the tolerant pass is
// not vacuous and the tolerant/strict split is exercised by real data.
//
// What it does not establish: anything about the other ~480 corpus models, or
// that routing these flows yields strict geometry (the corpus routing test
// arrives with flow-geometry.ts). The strict expectations characterize today's
// importers; the planned engine import fixes (MDL endpoints on larger stocks,
// MDL fallback flows) are expected to change them.

import { describe, it, expect, beforeAll } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import type { Model } from '@simlin/core/datamodel';

import {
  describeWithEngine,
  editorModel,
  engineSuiteMode,
  loadEngine,
  WASM_PATH,
  type EngineModule,
} from './support/engine';
import { checkFlowInvariants, formatFlowViolations } from './support/flow-invariants';
import { checkKindAgreement, checkReferentialIntegrity, formatViewViolations } from './support/view-invariants';

const repoRoot = path.join(__dirname, '..', '..', '..');

interface CorpusRow {
  readonly file: string;
  readonly producer: string;
  /** Every strict violation, as `uid:arm`, one entry per occurrence. */
  readonly strict: readonly string[];
}

// Each row's strict violations are the shapes the corpus measurement recorded for
// that file: a clean Stella model as the control; Stella endpoints on corners
// and off the 45x35 face; xmutil and MDL clouds a few px off their endpoints;
// MDL fallback flows with no attachment and valves off the path; a
// Simlin-authored XMILE endpoint off its face.
const CORPUS: readonly CorpusRow[] = [
  { file: 'test/test-models/samples/teacup/teacup.stmx', producer: 'Stella', strict: [] },
  {
    file: 'test/land_model/land_model.stmx',
    producer: 'Stella',
    strict: [
      '110:G4.cornerClearance',
      '110:G4.cornerClearance',
      '111:G4.cornerClearance',
      '112:G4.cornerClearance',
      '112:G4.offFace',
      '113:G4.cornerClearance',
      '113:G4.offFace',
      '114:G4.cornerClearance',
      '201:G4.cornerClearance',
      '202:G4.cornerClearance',
    ],
  },
  {
    file: 'test/test-models/tests/abs/test_abs.xmile',
    producer: 'Vensim via xmutil',
    strict: ['2:G7.cloudOffEndpoint'],
  },
  {
    file: 'test/test-models/samples/Roessler_Chaos/roessler_chaos.mdl',
    producer: 'Vensim MDL',
    strict: ['4:G7.cloudOffEndpoint', '11:G7.cloudOffEndpoint'],
  },
  {
    file: 'test/test-models/tests/subscript_mapping_simple/test_subscript_mapping_simple.mdl',
    producer: 'Vensim MDL',
    strict: ['8:G1.unattachedEndpoint', '8:G1.unattachedEndpoint', '8:G8.valveOffPath'],
  },
  { file: 'test/cross_element_ltm/cross_element.stmx', producer: 'Simlin XMILE', strict: ['4:G4.offFace'] },
];

describeWithEngine('flow invariants over imported corpus models', () => {
  let engine: EngineModule;

  beforeAll(async () => {
    engine = await loadEngine();
  });

  async function loadMainModel(file: string): Promise<Model> {
    const bytes = new Uint8Array(fs.readFileSync(path.join(repoRoot, file)));
    const project = file.endsWith('.mdl') ? await engine.Project.openVensim(bytes) : await engine.Project.open(bytes);
    try {
      return await editorModel(project);
    } finally {
      await project.dispose();
    }
  }

  for (const row of CORPUS) {
    it(`${row.producer}: ${row.file}`, async () => {
      const model = await loadMainModel(row.file);
      const view = model.views[0];
      expect(view.elements.some((e) => e.type === 'flow')).toBe(true);

      const tolerant = checkFlowInvariants(view, { mode: 'tolerant' });
      const structural = [...checkKindAgreement(view, model.variables), ...checkReferentialIntegrity(view)];
      expect(formatFlowViolations(tolerant) + formatViewViolations(structural)).toBe('');

      const strict = checkFlowInvariants(view, { mode: 'strict' })
        .map((v) => `${v.uid}:${v.arm}`)
        .sort();
      expect(strict).toEqual([...row.strict].sort());
    });
  }
});

describe('corpus fixtures', () => {
  it('every corpus model exists', () => {
    for (const row of CORPUS) {
      expect(`${row.file}: ${fs.existsSync(path.join(repoRoot, row.file))}`).toBe(`${row.file}: true`);
    }
  });
});

describe('engine-backed suites', () => {
  // Every arm of the run-or-skip decision: a built engine always runs, and CI
  // (GitHub Actions sets CI=true) runs even without one, so a missing build fails.
  const CASES: ReadonlyArray<{ built: boolean; ci: string | undefined; mode: 'run' | 'skip' }> = [
    { built: true, ci: undefined, mode: 'run' },
    { built: true, ci: 'true', mode: 'run' },
    { built: false, ci: 'true', mode: 'run' },
    { built: false, ci: '1', mode: 'run' },
    { built: false, ci: undefined, mode: 'skip' },
    { built: false, ci: '', mode: 'skip' },
    { built: false, ci: 'false', mode: 'skip' },
  ];
  for (const c of CASES) {
    it(`built=${c.built} CI=${JSON.stringify(c.ci)}: ${c.mode}`, () => {
      expect(engineSuiteMode(c.built, c.ci)).toBe(c.mode);
    });
  }

  it('this process uses that decision', () => {
    const mode = engineSuiteMode(fs.existsSync(WASM_PATH), process.env.CI);
    expect(describeWithEngine).toBe(mode === 'run' ? describe : describe.skip);
  });
});
