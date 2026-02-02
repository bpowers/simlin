/**
 * @jest-environment node
 *
 * Copyright 2025 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

import * as fs from 'fs';
import * as path from 'path';

import { Project as Engine2Project, configureWasm, ready } from '@system-dynamics/engine2';
import { reset } from '@system-dynamics/engine2/internal/wasm';
import { JsonProjectPatch } from '../json-types';

async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', '..', 'engine2', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  reset();
  configureWasm({ source: wasmBuffer });
  await ready();
}

function loadTestXmile(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

describe('applyPatch with variable creation', () => {
  beforeAll(async () => {
    await loadWasm();
  });

  it('should reject patch with empty equation when allowErrors is false', async () => {
    const project = await Engine2Project.open(loadTestXmile());

    const patch: JsonProjectPatch = {
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertAux',
              payload: { aux: { name: 'new_var', equation: '' } },
            },
          ],
        },
      ],
    };

    // Default behavior (allowErrors: false): throws when project has errors
    expect(() => project.applyPatch(patch)).toThrow();

    project.dispose();
  });

  it('should accept patch with empty equation when allowErrors is true', async () => {
    const project = await Engine2Project.open(loadTestXmile());

    const patch: JsonProjectPatch = {
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertAux',
              payload: { aux: { name: 'new_var', equation: '' } },
            },
          ],
        },
      ],
    };

    // With allowErrors: true, the patch should succeed
    const errors = project.applyPatch(patch, { allowErrors: true });

    // Variable should be created
    const vars = project.mainModel.variables.map((v) => v.name);
    expect(vars).toContain('new_var');

    // Should return collected errors (empty equation warning)
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.variableName === 'new_var')).toBe(true);

    project.dispose();
  });

  it('should provide descriptive error message when patch is rejected', async () => {
    const project = await Engine2Project.open(loadTestXmile());

    const patch: JsonProjectPatch = {
      models: [
        {
          name: 'main',
          ops: [
            {
              type: 'upsertAux',
              payload: { aux: { name: 'bad_var', equation: '' } },
            },
          ],
        },
      ],
    };

    try {
      project.applyPatch(patch); // allowErrors: false
      throw new Error('Should have thrown');
    } catch (e: unknown) {
      const error = e as { message?: string; details?: Array<{ variableName?: string }> };
      // Should NOT be "Unknown error"
      if (error.message === 'Should have thrown') {
        throw e;
      }
      expect(error.message).toBeDefined();
      expect(error.message).not.toBe('Unknown error');
      expect(error.message!.length).toBeGreaterThan(0);
      // Should have details
      expect(error.details).toBeDefined();
      expect(error.details!.length).toBeGreaterThan(0);
    }

    project.dispose();
  });
});
