// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The flow checker over real imported models, through the real WASM engine.
//
// What this establishes: for a handful of small corpus models covering the
// producers the corpus measurement grouped (Stella, Vensim via xmutil, native
// MDL, Simlin-authored XMILE), the first stock-flow view as the editor loads it
// (engine import -> serializeJson -> projectFromJson) passes tolerant mode and
// holds M1/M3, and the engine's import normalization leaves every flow holding
// strict geometry. Each file's raw geometry carries a violation shape the corpus
// measurement recorded, so a row fails if the importer stops normalizing it
// (M1/M3 are committed-view checks for the elements an edit routes or creates; a
// few other imports carry violations the editor still accepts, e.g. an orphan
// cloud in Query_file.mdl, lookup-only variables in test_lookups*.xmile).
//
// What it does not establish: anything about the other ~480 corpus models, that
// strict mode detects violations at all (flow-invariants.test.ts mutates a valid
// fixture once per arm), or that routing these flows keeps strict geometry
// (gesture-planner-corpus.test.ts).

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
  /** The violation shape the file's raw geometry carries, which the import must normalize. */
  readonly raw: string;
}

const CORPUS: readonly CorpusRow[] = [
  { file: 'test/test-models/samples/teacup/teacup.stmx', producer: 'Stella', raw: 'none (the control)' },
  {
    file: 'test/land_model/land_model.stmx',
    producer: 'Stella',
    raw: 'endpoints on stock corners and off the 45x35 face',
  },
  {
    file: 'test/test-models/tests/abs/test_abs.xmile',
    producer: 'Vensim via xmutil',
    raw: 'a cloud a few px off its endpoint',
  },
  {
    file: 'test/test-models/samples/Roessler_Chaos/roessler_chaos.mdl',
    producer: 'Vensim MDL',
    raw: 'clouds a few px off their endpoints',
  },
  {
    file: 'test/test-models/tests/subscript_mapping_simple/test_subscript_mapping_simple.mdl',
    producer: 'Vensim MDL',
    raw: 'a fallback flow with unattached ends and its valve off the path',
  },
  { file: 'test/cross_element_ltm/cross_element.stmx', producer: 'Simlin XMILE', raw: 'an endpoint off its face' },
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
    it(`${row.producer}: ${row.file} (raw: ${row.raw})`, async () => {
      const model = await loadMainModel(row.file);
      const view = model.views[0];
      expect(view.elements.some((e) => e.type === 'flow')).toBe(true);

      const tolerant = checkFlowInvariants(view, { mode: 'tolerant' });
      const structural = [...checkKindAgreement(view, model.variables), ...checkReferentialIntegrity(view)];
      expect(formatFlowViolations(tolerant) + formatViewViolations(structural)).toBe('');
      expect(formatFlowViolations(checkFlowInvariants(view, { mode: 'strict' }))).toBe('');
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
