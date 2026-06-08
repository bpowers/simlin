/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

import * as fs from 'fs';
import * as path from 'path';

import { Project as EngineProject, configureWasm, ready, resetWasm } from '@simlin/engine';
import { Project, projectFromJson, type StockFlowView } from '@simlin/core/datamodel';
import { mapSet } from '@simlin/core/common';

import { preserveLiveView } from '../merge-live-view';

async function loadWasm(): Promise<void> {
  const wasmPath = path.join(__dirname, '..', '..', 'engine', 'core', 'libsimlin.wasm');
  const wasmBuffer = fs.readFileSync(wasmPath);
  await resetWasm();
  configureWasm({ source: wasmBuffer });
  await ready();
}

function loadTeacup(): Uint8Array {
  const xmilePath = path.join(__dirname, '..', '..', 'pysimlin', 'tests', 'fixtures', 'teacup.stmx');
  if (!fs.existsSync(xmilePath)) {
    throw new Error('Required test XMILE model not found: ' + xmilePath);
  }
  return fs.readFileSync(xmilePath);
}

async function loadTeacupProject(): Promise<Project> {
  const project = await EngineProject.open(loadTeacup());
  const json = JSON.parse(await project.serializeJson(undefined, true));
  await project.dispose();
  return projectFromJson(json);
}

function withView(project: Project, modelName: string, view: StockFlowView): Project {
  const model = project.models.get(modelName);
  if (!model) {
    throw new Error(`model ${modelName} not in project`);
  }
  const updatedModel = { ...model, views: [view, ...model.views.slice(1)] };
  return { ...project, models: mapSet(project.models, modelName, updatedModel) };
}

describe('preserveLiveView', () => {
  beforeAll(loadWasm);

  it('returns incoming unchanged when live is undefined', async () => {
    const incoming = await loadTeacupProject();
    const result = preserveLiveView(incoming, undefined, 'main');
    expect(result).toBe(incoming);
  });

  it('returns incoming unchanged when modelName is missing in live', async () => {
    const incoming = await loadTeacupProject();
    const live = { ...incoming, models: new Map() };
    const result = preserveLiveView(incoming, live, 'main');
    expect(result).toBe(incoming);
  });

  it('returns incoming unchanged when modelName is missing in incoming', async () => {
    const live = await loadTeacupProject();
    const incoming = { ...live, models: new Map() };
    const result = preserveLiveView(incoming, live, 'main');
    expect(result).toBe(incoming);
  });

  // The core race: a stale incoming view (from a serialize call that the
  // engine completed before the user's most recent pan applied) would otherwise
  // overwrite the live view -- snap-back -- when updateProject commits.
  it('preserves the live viewBox and zoom over an incoming engine view', async () => {
    const baseline = await loadTeacupProject();
    const baselineView = baseline.models.get('main')!.views[0];

    // Engine snapshot: still at the original viewBox.
    const incoming = baseline;

    // Live (state.activeProject): user has panned and zoomed past the engine.
    const optimisticView: StockFlowView = {
      ...baselineView,
      viewBox: { x: -123, y: -456, width: 800, height: 600 },
      zoom: 1.75,
    };
    const live = withView(baseline, 'main', optimisticView);

    const merged = preserveLiveView(incoming, live, 'main');

    const mergedView = merged.models.get('main')!.views[0];
    expect(mergedView.viewBox).toEqual({ x: -123, y: -456, width: 800, height: 600 });
    expect(mergedView.zoom).toBe(1.75);
  });

  // Element drags use the same code path as pan -- the live view's element
  // positions must win over the engine's snapshot positions.
  it('preserves live element positions over an incoming engine view', async () => {
    const baseline = await loadTeacupProject();
    const baselineView = baseline.models.get('main')!.views[0];

    // Link elements carry x/y as NaN by design; skip those when shifting.
    const hasFiniteXY = (el: { x?: unknown; y?: unknown }): boolean =>
      typeof el.x === 'number' && Number.isFinite(el.x) && typeof el.y === 'number' && Number.isFinite(el.y);
    const movedElements = baselineView.elements.map((el) =>
      hasFiniteXY(el as { x?: unknown; y?: unknown })
        ? { ...el, x: (el as { x: number }).x + 200, y: (el as { y: number }).y - 75 }
        : el,
    );
    const optimisticView: StockFlowView = { ...baselineView, elements: movedElements };
    const live = withView(baseline, 'main', optimisticView);

    const merged = preserveLiveView(baseline, live, 'main');

    const mergedView = merged.models.get('main')!.views[0];
    let anyChecked = false;
    for (const el of mergedView.elements) {
      const original = baselineView.elements.find((b) => b.uid === el.uid);
      if (original && hasFiniteXY(original) && hasFiniteXY(el)) {
        expect((el as { x: number }).x).toBeCloseTo((original as { x: number }).x + 200);
        expect((el as { y: number }).y).toBeCloseTo((original as { y: number }).y - 75);
        anyChecked = true;
      }
    }
    expect(anyChecked).toBe(true);
  });

  // A live view built via setView during/after a structural change can carry
  // var: undefined or stale Variable refs. Re-linking against the incoming
  // (latest) variables keeps the diagram consistent without losing positions.
  it('relinks element var refs against the incoming model variables', async () => {
    const baseline = await loadTeacupProject();
    const baselineView = baseline.models.get('main')!.views[0];

    const elementsWithoutVarRefs = baselineView.elements.map((el) => {
      if (el.type === 'stock' || el.type === 'flow' || el.type === 'aux' || el.type === 'module') {
        return { ...el, var: undefined };
      }
      return el;
    });
    const optimisticView: StockFlowView = { ...baselineView, elements: elementsWithoutVarRefs };
    const live = withView(baseline, 'main', optimisticView);

    const merged = preserveLiveView(baseline, live, 'main');

    const incomingVars = baseline.models.get('main')!.variables;
    const mergedView = merged.models.get('main')!.views[0];
    let anyRelinked = false;
    for (const el of mergedView.elements) {
      if (el.type === 'stock' || el.type === 'flow' || el.type === 'aux') {
        if (incomingVars.has(el.ident)) {
          expect(el.var).toBeDefined();
          anyRelinked = true;
        }
      }
    }
    expect(anyRelinked).toBe(true);
  });

  // The whole point of the merge is that the engine's structural changes
  // (new variables, edited equations) survive while the live view wins.
  it('keeps incoming variables -- only the live view is preserved', async () => {
    const baseline = await loadTeacupProject();
    const baselineModel = baseline.models.get('main')!;

    // Construct a "live" project missing some variables, as could happen if
    // an optimistic setView propagated before a variable-creation patch did.
    const reducedVars = new Map(baselineModel.variables);
    const removedKey = [...reducedVars.keys()][0];
    reducedVars.delete(removedKey);
    const liveModel = { ...baselineModel, variables: reducedVars };
    const live: Project = { ...baseline, models: mapSet(baseline.models, 'main', liveModel) };

    const merged = preserveLiveView(baseline, live, 'main');

    expect(merged.models.get('main')!.variables.has(removedKey)).toBe(true);
  });
});
