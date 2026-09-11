// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// ProjectController against the fake engine (no WASM, no jsdom): the open
// lifecycle, edit items, truncation, stale tokens, maintenance coalescing and
// its starvation bound, viewport items, per-model rendering during navigation,
// undo gating, history, saves and server-version bookkeeping, sim runs, error
// derivation, and the one-executor guarantee.
//
// What this establishes: the controller's queueing, rendering and failure
// rules on scripted engine behavior. What it does not: that the real engine
// accepts the patches the controller builds (view-model-sync.test.ts and
// editor-engine-races.test.ts run those through libsimlin), or any Editor or
// Canvas wiring.
//
// The fake engine's serialization follows applied patches through
// `statefulProject`, which implements only the patch ops these tests send (view
// upserts, aux upserts and deletes). It is a test double for "the engine
// serializes what it was patched to", not a second implementation of patch
// semantics; the real semantics are exercised through libsimlin elsewhere.

import { describe, it, expect, rs } from '@rstest/core';

import { canonicalize } from '@simlin/core/canonicalize';
import {
  ErrorCode,
  projectFromJson,
  type AuxViewElement,
  type StockFlowView,
  type StockViewElement,
  type ViewElement,
} from '@simlin/core/datamodel';
import type { JsonProject, JsonProjectPatch, ErrorDetail } from '@simlin/engine';
import { SimlinErrorKind, SimlinUnitErrorKind } from '@simlin/engine';

import {
  ProjectController,
  MaintenanceEditBound,
  MaxUndoSize,
  convertErrorDetails,
  type ProjectSnapshot,
} from '../project-controller';
import {
  makeFakeEngine,
  makeControllerConfig,
  makeGate,
  validProjectJson,
  fakeRun,
  FAKE_STDLIB_MODEL_NAME,
  type FakeEngine,
  type FakeEngineOptions,
} from './fake-engine';
import { isStdlibModel } from '../module-navigation';

// ---------------------------------------------------------------------------
// Helpers

type JsonModelState = {
  name: string;
  auxiliaries: Array<{ name: string; equation?: string }>;
  stocks: Array<{ name: string }>;
  flows: Array<{ name: string }>;
  views: Array<Record<string, unknown>>;
};

function statefulProject(json: string): {
  json: (includeStdlib: boolean) => string;
  apply: (patch: JsonProjectPatch) => void;
} {
  const state = JSON.parse(json) as { models: JsonModelState[] };
  return {
    json: () => JSON.stringify(state),
    apply: (patch) => {
      for (const modelPatch of patch.models ?? []) {
        const model = state.models.find((m) => m.name === modelPatch.name);
        if (model === undefined) {
          continue;
        }
        for (const op of modelPatch.ops) {
          if (op.type === 'upsertView') {
            model.views[op.payload.index] = op.payload.view as unknown as Record<string, unknown>;
          } else if (op.type === 'upsertAux') {
            const ident = canonicalize(op.payload.aux.name);
            model.auxiliaries = [
              ...model.auxiliaries.filter((a) => canonicalize(a.name) !== ident),
              op.payload.aux as { name: string },
            ];
          } else if (op.type === 'deleteVariable') {
            const ident = canonicalize(op.payload.ident);
            model.auxiliaries = model.auxiliaries.filter((a) => canonicalize(a.name) !== ident);
          }
        }
      }
    },
  };
}

// An aux element as the Canvas stages one and the Editor commits it (a real uid,
// no var ref yet).
function aux(uid: number, name: string, x = 10, y = 10): AuxViewElement {
  return {
    type: 'aux',
    uid,
    name,
    ident: canonicalize(name),
    var: undefined,
    x,
    y,
    labelSide: 'right',
    isZeroRadius: false,
  };
}

function withElements(view: StockFlowView, ...elements: ViewElement[]): StockFlowView {
  const nextUid = Math.max(view.nextUid, ...elements.map((el) => el.uid + 1));
  return { ...view, elements: [...view.elements, ...elements], nextUid };
}

function moved(view: StockFlowView, uid: number, dx: number): StockFlowView {
  return {
    ...view,
    elements: view.elements.map((el) => (el.uid === uid && el.type === 'aux' ? { ...el, x: el.x + dx } : el)),
  };
}

function uids(view: StockFlowView | undefined): number[] {
  return (view?.elements ?? []).map((el) => el.uid).sort((a, b) => a - b);
}

function viewOps(patch: JsonProjectPatch, modelName = 'main') {
  return (patch.models ?? [])
    .filter((m) => m.name === modelName)
    .flatMap((m) => m.ops)
    .filter((op) => op.type === 'upsertView') as Array<{
    type: 'upsertView';
    payload: { view: { elements: Array<{ uid: number }>; viewBox?: { x: number }; zoom?: number } };
  }>;
}

interface Opened {
  controller: ProjectController;
  engine: FakeEngine;
  errors: Error[];
  saves: Array<{ project: { format: string; data: unknown }; currVersion: number }>;
  openedWith: Uint8Array[];
}

async function openController(
  engineOptions: FakeEngineOptions = {},
  configOptions: Partial<Parameters<typeof makeControllerConfig>[0]> = {},
): Promise<Opened> {
  const project = statefulProject(
    validProjectJson({
      auxiliaries: [{ name: 'a', equation: '1' }],
      mainViewElements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
    }),
  );
  const engine = makeFakeEngine({ json: project.json, onApplyPatch: project.apply, ...engineOptions });
  const { config, errors, saves, openedWith } = makeControllerConfig({ engine, format: 'json', ...configOptions });
  const controller = new ProjectController(config);
  await controller.openInitialProject();
  await controller.whenIdle();
  return { controller, engine, errors, saves, openedWith };
}

function view(controller: ProjectController, modelName = 'main'): StockFlowView {
  const v = controller.getSnapshot().project?.models.get(modelName)?.views[0];
  if (v === undefined) {
    throw new Error(`no rendered view for ${modelName}`);
  }
  return v;
}

function snap(n: number): Uint8Array {
  return new Uint8Array([n]);
}

// ---------------------------------------------------------------------------

describe('ProjectController open lifecycle', () => {
  it('opens the initial project and publishes a snapshot', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine, initialData: snap(1), initialVersion: 3 });
    const controller = new ProjectController(config);
    let notifies = 0;
    controller.subscribe(() => {
      notifies++;
    });
    await controller.openInitialProject();
    const s = controller.getSnapshot();
    expect(s.project?.name).toBe('test');
    expect(s.serverVersion).toBe(3);
    expect(notifies).toBeGreaterThan(0);
    await controller.dispose();
  });

  it('surfaces an error (and no project) when the engine open fails', async () => {
    const { config, errors } = makeControllerConfig({ openThrows: new Error('bad bytes') });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    expect(controller.getSnapshot().project).toBeUndefined();
    expect(errors.map((e) => e.message)).toEqual(['opening the project in the engine failed: bad bytes']);
    await controller.dispose();
  });

  it('disposes the opened engine and surfaces an error when serialization fails after open', async () => {
    const engine = makeFakeEngine({
      json: () => {
        throw new Error('engine panic in serializeJson');
      },
    });
    const { config, errors } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    expect(controller.getSnapshot().project).toBeUndefined();
    expect(errors.map((e) => e.message)).toEqual(['opening the project failed: engine panic in serializeJson']);
    expect(engine.disposeCount).toBe(1);
    await controller.dispose();
    expect(engine.disposeCount).toBe(1);
  });

  it('releases the engine an in-flight open produces when dispose races in first', async () => {
    let resolveOpen: (e: FakeEngine) => void = () => {};
    const engine = makeFakeEngine();
    const openPromise = new Promise<FakeEngine>((resolve) => {
      resolveOpen = resolve;
    });
    const controller = new ProjectController({
      initialProjectVersion: 1,
      input: { format: 'protobuf', data: snap(1) },
      openProtobuf: () => openPromise,
      openJson: () => openPromise,
      save: async () => 1,
      onError: () => {},
    });
    const opening = controller.openInitialProject();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const disposing = controller.dispose();
    resolveOpen(engine);
    await opening;
    await disposing;
    expect(engine.disposeCount).toBe(1);
    expect(controller.getSnapshot().project).toBeUndefined();
  });
});

describe('ProjectController initialViewport (a host-carried viewport for the opened view)', () => {
  const carried = { viewBox: { x: -120, y: 35.5, width: 800, height: 450 }, zoom: 1.75 };

  it('renders from the first published snapshot and persists through one view-only patch', async () => {
    const engine = makeFakeEngine();
    const { config, saves } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController({ ...config, initialViewport: carried });
    const seen: Array<{ viewBox: unknown; zoom: number }> = [];
    controller.subscribe(() => {
      const v = controller.getView();
      if (v) {
        seen.push({ viewBox: v.viewBox, zoom: v.zoom });
      }
    });
    await controller.openInitialProject();
    await controller.whenIdle();
    expect(seen[0]).toEqual({ viewBox: carried.viewBox, zoom: carried.zoom });
    expect(engine.appliedPatches).toHaveLength(1);
    const op = viewOps(engine.appliedPatches[0])[0];
    expect(op.payload.view.viewBox).toEqual(carried.viewBox);
    expect(op.payload.view.zoom).toBe(carried.zoom);
    expect(controller.canUndo()).toBe(false);
    expect(saves).toHaveLength(0);
    await controller.dispose();
  });

  it('without an override the stored viewport is opened and no view patch is applied', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    expect(controller.getView()).toMatchObject({ viewBox: { x: 0, y: 0, width: 0, height: 0 }, zoom: 1 });
    expect(engine.appliedPatches).toHaveLength(0);
    await controller.dispose();
  });

  it('an unusable override (non-finite coordinate, non-positive zoom) is ignored, silently', async () => {
    for (const bad of [
      { viewBox: { x: Number.NaN, y: 0, width: 800, height: 450 }, zoom: 1 },
      { viewBox: { x: 0, y: 0, width: Number.POSITIVE_INFINITY, height: 450 }, zoom: 1 },
      { viewBox: { x: 0, y: 0, width: 800, height: 450 }, zoom: 0 },
      { viewBox: { x: 0, y: 0, width: 800, height: 450 }, zoom: -2 },
      { viewBox: { x: 0, y: 0, width: 800, height: 450 }, zoom: Number.NaN },
    ]) {
      const engine = makeFakeEngine();
      const { config, errors } = makeControllerConfig({ engine, format: 'json' });
      const controller = new ProjectController({ ...config, initialViewport: bad });
      await controller.openInitialProject();
      await controller.whenIdle();
      expect(controller.getView()).toMatchObject({ viewBox: { x: 0, y: 0, width: 0, height: 0 }, zoom: 1 });
      expect(engine.appliedPatches).toHaveLength(0);
      expect(errors).toEqual([]);
      await controller.dispose();
    }
  });

  it('a root model without a view opens as usual (nothing to override)', async () => {
    const json = JSON.stringify({
      name: 'noview',
      simSpecs: { startTime: 0, endTime: 10, dt: '1' },
      models: [{ name: 'main', stocks: [], flows: [], auxiliaries: [], views: [] }],
    });
    const engine = makeFakeEngine({ json });
    const { config, errors } = makeControllerConfig({ engine, format: 'json', initialData: json });
    const controller = new ProjectController({ ...config, initialViewport: carried });
    await controller.openInitialProject();
    await controller.whenIdle();
    expect(controller.getSnapshot().project).toBeDefined();
    expect(controller.getView()).toBeUndefined();
    expect(engine.appliedPatches).toHaveLength(0);
    expect(errors).toEqual([]);
    await controller.dispose();
  });
});

describe('ProjectController edit items', () => {
  it('a view edit renders at once, lands as one patch, and records one history entry', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({ applyPatchGate: () => gate.wait() });
    const next = withElements(view(controller), aux(2, 'b'));
    const landed = controller.enqueueViewEdit({ label: 'create', nextView: next });
    // Optimistic: rendered before any engine call.
    expect(uids(view(controller))).toEqual([1, 2]);
    expect(controller.getSnapshot().canUndo).toBe(false);
    gate.open();
    expect(await landed).toBe(true);
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(1);
    expect(engine.appliedPatches[0].models![0].ops.map((op) => op.type)).toEqual(['upsertAux', 'upsertView']);
    expect(uids(view(controller))).toEqual([1, 2]);
    expect(controller.getSnapshot().canUndo).toBe(true);
    await controller.dispose();
  });

  it('each landed edit records exactly one history entry, up to MaxUndoSize', async () => {
    const { controller, engines } = await (async () => {
      const opened = await openController();
      return { controller: opened.controller, engines: [opened.engine] };
    })();
    void engines;
    for (let i = 0; i < 3; i++) {
      await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    }
    await controller.whenIdle();
    // Three edits on top of the open: three undos available, not four.
    let undos = 0;
    // Count by walking the predicate without landing an undo (which reopens).
    const history = (controller as unknown as { projectHistory: unknown[] }).projectHistory;
    undos = history.length - 1;
    expect(undos).toBe(3);
    for (let i = 0; i < MaxUndoSize + 2; i++) {
      await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    }
    await controller.whenIdle();
    expect((controller as unknown as { projectHistory: unknown[] }).projectHistory).toHaveLength(MaxUndoSize);
    await controller.dispose();
  });

  it('a model-only edit builds its payload from the committed project at dequeue, after earlier edits landed', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    void controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(2, 'b')) });
    const seen: string[][] = [];
    const landed = controller.enqueueModelEdit({
      label: 'equation',
      buildPatch: (committed) => {
        seen.push([...committed.models.get('main')!.variables.keys()].sort());
        return {
          models: [{ name: 'main', ops: [{ type: 'upsertAux', payload: { aux: { name: 'b', equation: '2' } } }] }],
        };
      },
    });
    expect(seen).toEqual([]);
    gate.open();
    expect(await landed).toBe(true);
    expect(seen).toEqual([['a', 'b']]);
    expect(engine.appliedPatches).toHaveLength(2);
    await controller.dispose();
  });

  it('a failing model-only edit only reports: a later view edit lands and the token does not move', async () => {
    // Nothing is planned on a model-only edit (it has no next view), so its
    // failure -- a builder throw or an engine rejection -- discards nothing.
    for (const failure of ['builder', 'engine'] as const) {
      const { controller, errors, engine } = await openController({
        applyPatchThrows: (_p, i) => (failure === 'engine' && i === 0 ? new Error('rejected') : undefined),
      });
      const tokenBefore = controller.getSnapshot().token;
      const failing = controller.enqueueModelEdit({
        label: 'equation',
        buildPatch: () => {
          if (failure === 'builder') {
            throw new Error("variable 'gone' does not exist");
          }
          return { models: [] };
        },
      });
      const move = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
      expect(await failing).toBe(false);
      expect(await move).toBe(true);
      await controller.whenIdle();
      expect(errors.map((e) => e.message)).toEqual([
        failure === 'builder' ? "variable 'gone' does not exist" : 'rejected',
      ]);
      expect(controller.getSnapshot().token).toBe(tokenBefore);
      expect(engine.appliedPatches.map((p) => p.models![0].ops[0].type)).toEqual(['upsertView']);
      await controller.dispose();
    }
  });

  it('a model-only builder reads the committed project, never the rendered one', async () => {
    const gate = makeGate();
    const { controller } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    // A create queued behind a model-only edit renders before the builder runs.
    const seen: number[][] = [];
    void controller.enqueueModelEdit({
      label: 'equation',
      buildPatch: (committed) => {
        seen.push(uids(committed.models.get('main')!.views[0]));
        return { models: [] };
      },
    });
    void controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(2, 'b')) });
    expect(uids(view(controller))).toEqual([1, 2]);
    gate.open();
    await controller.whenIdle();
    expect(seen).toEqual([[1]]);
    await controller.dispose();
  });

  it('an edit whose serialized project equals the history head records no history entry', async () => {
    const { controller, engine } = await openController({ protobuf: new Uint8Array([7]) });
    expect(await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) })).toBe(true);
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(1);
    expect(controller.getSnapshot().canUndo).toBe(false);
    await controller.dispose();
  });

  it("an edit's upsertView carries the committed viewport, never the live one", async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 5) });
    void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 5) });
    controller.setViewport('main', { viewBox: { x: 9, y: 9, width: 800, height: 600 }, zoom: 2 });
    // The second edit was planned before the pan, the viewport item queued after it.
    gate.open();
    await controller.whenIdle();
    const [first, second, viewportPatch] = engine.appliedPatches;
    expect(viewOps(first)[0].payload.view.zoom).toBe(1);
    expect(viewOps(second)[0].payload.view.zoom).toBe(1);
    expect(viewOps(viewportPatch)[0].payload.view.zoom).toBe(2);
    await controller.dispose();
  });

  it('a view with a non-finite coordinate is refused before anything renders or patches (#818)', async () => {
    const { controller, engine, errors } = await openController();
    const before = controller.getSnapshot();
    const badStock: StockViewElement = {
      type: 'stock',
      uid: 999,
      name: 'bad',
      ident: 'bad',
      var: undefined,
      x: NaN,
      y: 0,
      labelSide: 'top',
      isZeroRadius: false,
      inflows: [],
      outflows: [],
    };
    expect(
      await controller.enqueueViewEdit({ label: 'move', nextView: withElements(view(controller), badStock) }),
    ).toBe(false);
    expect(controller.getSnapshot()).toBe(before);
    expect(engine.appliedPatches).toHaveLength(0);
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toContain('uid=999');

    controller.setViewport('main', { viewBox: { x: NaN, y: 0, width: 1, height: 1 }, zoom: 1 });
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(0);
    expect(errors).toHaveLength(2);
    await controller.dispose();
  });

  it('newVariableName allocates past pending creates; nameError reports a pending create', async () => {
    const gate = makeGate();
    const { controller } = await openController({ applyPatchGate: () => gate.wait() });
    expect(controller.newVariableName('New Variable')).toBe('New Variable');
    void controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'New Variable')),
    });
    expect(controller.newVariableName('New Variable')).toBe('New Variable 1');
    expect(controller.nameError('new variable', undefined)).toBeDefined();
    expect(controller.nameError('a', 'a')).toBeUndefined();
    gate.open();
    await controller.whenIdle();
    await controller.dispose();
  });

  it('a committed variable with no element on the view still takes its name', async () => {
    const engine = makeFakeEngine({
      json: validProjectJson({
        auxiliaries: [{ name: 'a', equation: '1' }, { name: 'Hidden' }],
        mainViewElements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
      }),
    });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    expect(controller.nameError('HIDDEN', undefined)).toBeDefined();
    expect(controller.newVariableName('hidden')).toBe('hidden 1');
    await controller.dispose();
  });
});

describe('ProjectController a patch that applied but could not be read back', () => {
  // The resync runs BEFORE the item's fate is decided, and the arm it takes
  // decides it:
  //  - re-read: the patch is kept, so the edit landed -- nothing is discarded,
  //    nothing reported;
  //  - reopen: the patch is lost, so the edit failed, and so did every view edit
  //    planned on its next view, including one enqueued while the reopen ran;
  //  - release: the engine is gone -- everything queued settles quietly, every
  //    later request is refused quietly, and the snapshot says engineUnavailable
  //    for the host's one notice;
  //  - disposed meanwhile: both engines are released.
  // A viewport persist runs the same resync without recording history, and
  // reports nothing (the last two rows).
  const originalJson = () =>
    validProjectJson({
      auxiliaries: [{ name: 'a', equation: '1' }],
      mainViewElements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
    });

  // `failures.remaining` project reads (with stdlib, as a read-back does) throw,
  // on whichever engine is current. A reopen succeeds with a fresh engine that
  // starts from the original project and follows its own patches, fails, or
  // waits on a gate first.
  function armed(reopen: 'succeeds' | 'fails' | { readonly gate: Promise<void> }) {
    const failures = { remaining: 0 };
    const readJson =
      (source: (includeStdlib: boolean) => string) =>
      (includeStdlib: boolean): string => {
        if (includeStdlib && failures.remaining > 0) {
          failures.remaining -= 1;
          throw new Error('injected read failure');
        }
        return source(includeStdlib);
      };
    const project = statefulProject(originalJson());
    const engine = makeFakeEngine({ json: readJson(project.json), onApplyPatch: project.apply });
    const reopened: FakeEngine[] = [];
    const opens: Uint8Array[] = [];
    const { config, errors } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController({
      ...config,
      openProtobuf: async (data) => {
        opens.push(data);
        if (reopen === 'fails') {
          throw new Error('reopen failed');
        }
        if (reopen !== 'succeeds') {
          await reopen.gate;
        }
        const reopenedProject = statefulProject(originalJson());
        const next = makeFakeEngine({ json: readJson(reopenedProject.json), onApplyPatch: reopenedProject.apply });
        reopened.push(next);
        return next;
      },
    });
    return { controller, engine, reopened, failures, opens, errors };
  }

  function history(controller: ProjectController): { projectHistory: Uint8Array[]; projectOffset: number } {
    return controller as unknown as { projectHistory: Uint8Array[]; projectOffset: number };
  }

  async function until(predicate: () => boolean): Promise<void> {
    for (let i = 0; i < 100 && !predicate(); i++) {
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
    expect(predicate()).toBe(true);
  }

  it('re-read: the edit landed -- it stays, an edit planned on it lands, and nothing is reported', async () => {
    const { controller, failures, opens, errors } = armed('succeeds');
    await controller.openInitialProject();
    await controller.whenIdle();
    const tokenBefore = controller.getSnapshot().token;

    failures.remaining = 1;
    const create = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    const later = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 2, 30) });
    expect(await create).toBe(true);
    expect(await later).toBe(true);
    await controller.whenIdle();

    expect(errors).toEqual([]);
    expect(controller.getSnapshot().token).toBe(tokenBefore);
    expect(uids(view(controller))).toEqual([1, 2]);
    // The open, the create (recorded by the re-read) and the move.
    expect(history(controller).projectHistory).toHaveLength(3);
    expect(opens).toEqual([]);
    await controller.dispose();
  });

  it('reopen: the edit failed -- the last recorded snapshot is installed, the edit planned on it is discarded, the live viewport persists again', async () => {
    const { controller, engine, reopened, failures, opens, errors } = armed('succeeds');
    await controller.openInitialProject();
    await controller.whenIdle();
    controller.setViewport('main', { viewBox: { x: 3, y: 0, width: 800, height: 600 }, zoom: 2 });
    await controller.whenIdle();
    const head = history(controller).projectHistory[0];
    const tokenBefore = controller.getSnapshot().token;

    failures.remaining = 2;
    const create = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    const later = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 2, 30) });
    expect(await create).toBe(false);
    expect(await later).toBe(false);
    await controller.whenIdle();

    expect(errors.map((e) => e.message)).toEqual([
      'reading the project back after create failed: injected read failure (1 later edit discarded)',
    ]);
    expect(controller.getSnapshot().token).toBe(tokenBefore + 1);
    expect(opens).toEqual([head]);
    expect(engine.disposeCount).toBe(1);
    expect(uids(view(controller))).toEqual([1]);
    // The live viewport, which the snapshot does not carry, is persisted again.
    expect(viewOps(reopened[0].appliedPatches[0])[0].payload.view.zoom).toBe(2);
    await controller.dispose();
  });

  it('reopen: a view edit enqueued while the reopen runs was planned on the failed edit, and is discarded with it after the swap', async () => {
    let openGate!: () => void;
    const gate = new Promise<void>((resolve) => {
      openGate = resolve;
    });
    const { controller, failures, opens, errors } = armed({ gate });
    await controller.openInitialProject();
    await controller.whenIdle();
    const tokenBefore = controller.getSnapshot().token;

    failures.remaining = 2;
    const create = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    await until(() => opens.length === 1);
    // Nothing is decided yet: the failed create still renders, and the token
    // has not moved, so this is what a handler plans on.
    expect(uids(view(controller))).toEqual([1, 2]);
    expect(controller.getSnapshot().token).toBe(tokenBefore);
    const planned = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 2, 30) });
    openGate();
    expect(await create).toBe(false);
    expect(await planned).toBe(false);
    await controller.whenIdle();

    expect(controller.getSnapshot().token).toBe(tokenBefore + 1);
    expect(uids(view(controller))).toEqual([1]);
    expect(errors.map((e) => e.message)).toEqual([
      'reading the project back after create failed: injected read failure (1 later edit discarded)',
    ]);
    await controller.dispose();
  });

  it('release: the engine is lost -- everything queued settles quietly, every later request is refused quietly, and the snapshot says so', async () => {
    const { controller, engine, failures, errors } = armed('fails');
    await controller.openInitialProject();
    await controller.whenIdle();
    // History to undo, so the refusal below is the unavailable state's doing.
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.whenIdle();
    expect(controller.getSnapshot().canUndo).toBe(true);
    const tokenBefore = controller.getSnapshot().token;

    failures.remaining = 2;
    const create = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    const later = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 2, 30) });
    const query = controller.query(async () => 'answer');
    expect(await create).toBe(false);
    expect(await later).toBe(false);
    expect(await query).toBeUndefined();
    await controller.whenIdle();

    const s = controller.getSnapshot();
    expect(s.engineUnavailable).toBe(true);
    expect(s.status).toBe('disabled');
    expect(s.token).toBe(tokenBefore);
    expect(engine.disposeCount).toBe(1);
    expect(await controller.enqueueModelEdit({ label: 'equation', buildPatch: () => ({ models: [] }) })).toBe(false);
    expect(await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 5) })).toBe(false);
    expect(await controller.query(async () => 1)).toBeUndefined();
    expect(controller.getSnapshot().canUndo).toBe(false);
    controller.undoRedo('undo');
    controller.undoRedo('undo', { afterQueuedEdits: true });
    expect(controller.getSnapshot().undoRedoQueued).toBe(false);
    await controller.whenIdle();
    expect(errors).toEqual([]);
    await controller.dispose();
  });

  it('a dispose while the reopen runs releases both engines and reports nothing', async () => {
    let openGate!: () => void;
    const gate = new Promise<void>((resolve) => {
      openGate = resolve;
    });
    const { controller, engine, reopened, failures, opens, errors } = armed({ gate });
    await controller.openInitialProject();
    await controller.whenIdle();

    failures.remaining = 2;
    const create = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    await until(() => opens.length === 1);
    const disposing = controller.dispose();
    openGate();
    expect(await create).toBe(false);
    await disposing;
    expect(engine.disposeCount).toBe(1);
    expect(reopened).toHaveLength(1);
    expect(reopened[0].disposeCount).toBe(1);
    expect(errors).toEqual([]);
  });

  it('a viewport persist whose read-back fails re-reads without recording history: the redo branch survives, committed catches up, nothing is reported', async () => {
    const { controller, reopened, failures, opens, errors } = armed('succeeds');
    await controller.openInitialProject();
    await controller.whenIdle();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    controller.undoRedo('undo');
    await controller.whenIdle();
    expect(controller.getSnapshot().canRedo).toBe(true);
    const current = reopened[0];
    const historyLength = history(controller).projectHistory.length;

    failures.remaining = 1;
    const viewport = { viewBox: { x: 40, y: 40, width: 800, height: 600 }, zoom: 1.5 };
    controller.setViewport('main', viewport);
    await controller.whenIdle();
    expect(controller.getSnapshot().canRedo).toBe(true);
    expect(history(controller).projectHistory).toHaveLength(historyLength);
    expect(opens).toHaveLength(1);
    expect(errors).toEqual([]);
    // Committed caught up with the engine, so the same viewport patches nothing.
    const patches = current.appliedPatches.length;
    controller.setViewport('main', viewport);
    await controller.whenIdle();
    expect(current.appliedPatches).toHaveLength(patches);
    await controller.dispose();
  });

  it('a viewport persist whose re-read fails too reopens the snapshot at the history cursor, and persists the viewport again', async () => {
    const { controller, reopened, failures, opens, errors } = armed('succeeds');
    await controller.openInitialProject();
    await controller.whenIdle();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    controller.undoRedo('undo');
    await controller.whenIdle();
    const { projectHistory, projectOffset } = history(controller);
    expect(projectOffset).toBe(1);

    failures.remaining = 2;
    controller.setViewport('main', { viewBox: { x: 40, y: 40, width: 800, height: 600 }, zoom: 1.5 });
    await controller.whenIdle();
    // The cursor's snapshot (the undone state), not the head (the redo branch).
    expect(opens).toHaveLength(2);
    expect(opens[1]).toEqual(projectHistory[1]);
    expect(opens[1]).not.toEqual(projectHistory[0]);
    expect(controller.getSnapshot().canRedo).toBe(true);
    expect(errors).toEqual([]);
    expect(viewOps(reopened[1].appliedPatches[0])[0].payload.view.zoom).toBe(1.5);
    await controller.dispose();
  });
});

describe('ProjectController truncation when an edit fails', () => {
  // Every kind of item that can wait behind a failing edit. A view edit was
  // planned on the failing edit's optimistic view and is discarded. A model-only
  // edit builds its payload from committed state at dequeue, so it survives and
  // lands. Viewport and query items are not edits and survive. Undo is not a row:
  // it cannot be queued while an edit is pending (see undo gating).
  const LATER = ['viewEdit', 'modelEdit', 'viewport', 'query'] as const;

  for (const later of LATER) {
    const discarded = later === 'viewEdit';
    it(`a failing edit ${discarded ? 'discards' : 'keeps'} a later ${later}`, async () => {
      const gate = makeGate();
      const { controller, engine, errors } = await openController({
        applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
        applyPatchThrows: (_p, i) => (i === 0 ? new Error('boom') : undefined),
      });
      const tokenBefore = controller.getSnapshot().token;
      const failing = controller.enqueueViewEdit({
        label: 'create',
        nextView: withElements(view(controller), aux(2, 'b')),
      });
      let laterResult: Promise<unknown> = Promise.resolve();
      switch (later) {
        case 'viewEdit':
          laterResult = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 2, 30) });
          break;
        case 'modelEdit':
          laterResult = controller.enqueueModelEdit({ label: 'eq', buildPatch: () => ({ models: [] }) });
          break;
        case 'viewport':
          controller.setViewport('main', { viewBox: { x: 4, y: 4, width: 640, height: 480 }, zoom: 1.5 });
          break;
        case 'query':
          laterResult = controller.query(async () => 'answer');
          break;
      }
      gate.open();
      expect(await failing).toBe(false);
      const laterValue = await laterResult;
      await controller.whenIdle();

      // Model and diagram agree: the rendered view is committed.
      expect(uids(view(controller))).toEqual([1]);
      expect(controller.getSnapshot().token).toBe(tokenBefore + 1);
      expect(errors.map((e) => e.message)).toEqual([discarded ? 'boom (1 later edit discarded)' : 'boom']);
      if (discarded) {
        expect(laterValue).toBe(false);
        expect(engine.appliedPatches).toHaveLength(0);
      } else if (later === 'modelEdit') {
        // The model-only edit queued behind the failed view edit lands.
        expect(laterValue).toBe(true);
        expect(engine.appliedPatches).toHaveLength(1);
      } else if (later === 'viewport') {
        expect(engine.appliedPatches).toHaveLength(1);
        // The surviving viewport item persists committed elements only.
        expect(viewOps(engine.appliedPatches[0])[0].payload.view.elements.map((el) => el.uid)).toEqual([1]);
      } else {
        expect(laterValue).toBe('answer');
      }
      await controller.dispose();
    });
  }

  it('a model-only edit to a variable the failed edit would have created survives truncation and fails with its own error', async () => {
    const gate = makeGate();
    const { controller, engine, errors } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
      applyPatchThrows: (_p, i) => (i === 0 ? new Error('boom') : undefined),
    });
    const created = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    // The user types an equation for b while its create is still in flight. The
    // builder reads the committed variable at dequeue, as the Editor's does.
    const equation = controller.enqueueModelEdit({
      label: 'equation update',
      buildPatch: (committed) => {
        if (!committed.models.get('main')!.variables.has('b')) {
          throw new Error("equation update failed: 'b' no longer exists");
        }
        return {
          models: [{ name: 'main', ops: [{ type: 'upsertAux', payload: { aux: { name: 'b', equation: '5' } } }] }],
        };
      },
    });
    gate.open();
    expect(await created).toBe(false);
    expect(await equation).toBe(false);
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(0);
    expect(uids(view(controller))).toEqual([1]);
    expect(errors.map((e) => e.message)).toEqual(['boom', "equation update failed: 'b' no longer exists"]);
    await controller.dispose();
  });

  it('an edit planned on a failed edit is discarded with it (a move of the element it created)', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
      applyPatchThrows: (_p, i) => (i === 0 ? new Error('boom') : undefined),
    });
    const created = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    // Planned on the optimistic view: element 2 exists only there.
    expect(uids(view(controller))).toEqual([1, 2]);
    const moveIt = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 2, 30) });
    gate.open();
    expect(await created).toBe(false);
    expect(await moveIt).toBe(false);
    expect(engine.appliedPatches).toHaveLength(0);
    expect(uids(view(controller))).toEqual([1]);
    await controller.dispose();
  });

  it('a failure in the middle keeps the edits before it and discards those after', async () => {
    const { controller, engine, errors } = await openController({
      applyPatchThrows: (_p, i) => (i === 1 ? new Error('second failed') : undefined),
    });
    const first = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    const second = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(3, 'c')),
    });
    const third = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(4, 'd')),
    });
    const fourth = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(5, 'e')),
    });
    expect(await Promise.all([first, second, third, fourth])).toEqual([true, false, false, false]);
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(1);
    expect(uids(view(controller))).toEqual([1, 2]);
    expect(errors.map((e) => e.message)).toEqual(['second failed (2 later edits discarded)']);
    await controller.dispose();
  });
});

describe('ProjectController stale tokens', () => {
  it('a gesture planned before a failure is refused at enqueue', async () => {
    const { controller, engine, errors } = await openController({
      applyPatchThrows: (_p, i) => (i === 0 ? new Error('boom') : undefined),
    });
    const plannedUnder = controller.getSnapshot().token;
    const plannedOn = view(controller);
    await controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(2, 'b')) });
    expect(controller.getSnapshot().token).toBe(plannedUnder + 1);
    const renderedX: number[] = [];
    controller.subscribe(() => {
      renderedX.push(view(controller).elements.find((el) => el.uid === 1)!.x);
    });
    const landed = await controller.enqueueViewEdit({
      label: 'move',
      nextView: moved(plannedOn, 1, 20),
      token: plannedUnder,
    });
    expect(landed).toBe(false);
    await controller.whenIdle();
    // Refused before it could render: the stale view never flashes.
    expect(renderedX).not.toContain(plannedOn.elements.find((el) => el.uid === 1)!.x + 20);
    expect(engine.appliedPatches).toHaveLength(0);
    expect(errors.map((e) => e.message)).toEqual([
      'boom',
      'move discarded: the project changed while it was being made',
    ]);
    await controller.dispose();
  });

  it('an edit with a next view planned while an undo is queued is refused quietly', async () => {
    const { controller, engine, errors } = await openController();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.whenIdle();
    expect(controller.getSnapshot().canUndo).toBe(true);

    controller.undoRedo('undo');
    const seen: ProjectSnapshot[] = [];
    controller.subscribe(() => {
      seen.push(controller.getSnapshot());
    });
    // Planned now, on the pre-undo view: it could only be dropped once the undo
    // lands, and that is no failure of the user's to report.
    const stale = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    expect(seen).toHaveLength(0);
    expect(await stale).toBe(false);
    await controller.whenIdle();
    expect(errors).toEqual([]);
    // The undo reopened the same fake engine; only the first move reached it.
    expect(engine.appliedPatches).toHaveLength(1);
    await controller.dispose();
  });

  it('a view edit whose token moved before dequeue is dropped with the edits planned on it, and the token moves only once', async () => {
    // No production path moves the token while a view edit stays queued (a
    // truncation discards it, and an undo cannot be queued ahead of one), so the
    // move is made by hand; this pins the dequeue check behind the enqueue check.
    const gate = makeGate();
    const { controller, engine, errors } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    void controller.enqueueModelEdit({ label: 'equation', buildPatch: () => ({ models: [] }) });
    const stale = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    const plannedOnIt = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    const tokenBefore = controller.getSnapshot().token;
    (controller as unknown as { token: number }).token += 1;
    gate.open();
    expect(await stale).toBe(false);
    expect(await plannedOnIt).toBe(false);
    await controller.whenIdle();
    expect(controller.getSnapshot().token).toBe(tokenBefore + 1);
    expect(errors.map((e) => e.message)).toEqual([
      'move discarded: the project changed while it was being made (1 later edit discarded)',
    ]);
    expect(engine.appliedPatches).toHaveLength(1);
    await controller.dispose();
  });

  it('a model-only edit planned before an undo lands is exempt: it derives its payload at dequeue and lands', async () => {
    const { controller, engine, errors } = await openController();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.whenIdle();
    const tokenBefore = controller.getSnapshot().token;

    controller.undoRedo('undo');
    const modelOnly = controller.enqueueModelEdit({
      label: 'equation',
      buildPatch: () => ({
        models: [{ name: 'main', ops: [{ type: 'upsertAux', payload: { aux: { name: 'a', equation: '5' } } }] }],
      }),
    });
    expect(await modelOnly).toBe(true);
    await controller.whenIdle();
    expect(controller.getSnapshot().token).toBe(tokenBefore + 1);
    expect(errors).toEqual([]);
    expect(engine.appliedPatches.map((p) => p.models![0].ops[0].type)).toEqual(['upsertView', 'upsertAux']);
    await controller.dispose();
  });
});

describe('ProjectController undo/redo gating', () => {
  it('undo is unavailable (snapshot and method) and refused while an edit is pending', async () => {
    const gate = makeGate();
    const { controller, openedWith } = await openController({
      applyPatchGate: (_p, i) => (i === 1 ? gate.wait() : Promise.resolve()),
    });
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    expect(controller.getSnapshot().canUndo).toBe(true);

    const pending = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    expect(controller.getSnapshot().canUndo).toBe(false);
    expect(controller.canUndo()).toBe(false);
    controller.undoRedo('undo');
    expect(controller.getSnapshot().undoRedoQueued).toBe(false);
    gate.open();
    await pending;
    await controller.whenIdle();
    expect(openedWith).toHaveLength(0);
    expect(controller.getSnapshot().canUndo).toBe(true);
    await controller.dispose();
  });

  it('a queued undo blocks further undo and flags undoRedoQueued; landing restores, bumps the token, and resets viewports', async () => {
    const original = validProjectJson({
      auxiliaries: [{ name: 'a', equation: '1' }],
      mainViewElements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
    });
    const project = statefulProject(original);
    const engine = makeFakeEngine({ json: project.json, onApplyPatch: project.apply });
    // The reopened engine serializes the pre-edit project, as libsimlin does
    // when it opens the history snapshot.
    const reopened = makeFakeEngine({ json: () => original });
    const { config, openedWith } = makeControllerConfig({ engines: [engine, reopened], format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    const openBytes = (controller as unknown as { projectHistory: Uint8Array[] }).projectHistory[0];
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    controller.setViewport('main', { viewBox: { x: 77, y: 0, width: 800, height: 600 }, zoom: 3 });
    await controller.whenIdle();
    const tokenBefore = controller.getSnapshot().token;

    controller.undoRedo('undo');
    expect(controller.getSnapshot().undoRedoQueued).toBe(true);
    expect(controller.getSnapshot().canUndo).toBe(false);
    expect(controller.getSnapshot().canRedo).toBe(false);
    await controller.whenIdle();

    expect(openedWith[openedWith.length - 1]).toEqual(openBytes);
    const s = controller.getSnapshot();
    expect(s.undoRedoQueued).toBe(false);
    expect(s.token).toBe(tokenBefore + 1);
    expect(s.canRedo).toBe(true);
    // The live viewport map was reset: the rendered view is the restored one.
    expect(view(controller).zoom).toBe(1);
    // The reopen installs the new engine and releases the old one.
    expect(engine.disposeCount).toBe(1);
    await controller.dispose();
  });

  it('an undo submitted together with a draft queues behind the draft edit and undoes it', async () => {
    const gate = makeGate();
    const { controller, errors, openedWith } = await openController({
      applyPatchGate: (_p, i) => (i === 1 ? gate.wait() : Promise.resolve()),
    });
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    const afterMove = (controller as unknown as { projectHistory: Uint8Array[] }).projectHistory[0];
    const draft = controller.enqueueModelEdit({
      label: 'units',
      buildPatch: () => ({
        models: [{ name: 'main', ops: [{ type: 'upsertAux', payload: { aux: { name: 'a', equation: '7' } } }] }],
      }),
    });
    // A plain undo refuses while the draft's edit is queued...
    controller.undoRedo('undo');
    expect(controller.getSnapshot().undoRedoQueued).toBe(false);
    // ...one submitted with it queues behind it.
    controller.undoRedo('undo', { afterQueuedEdits: true });
    expect(controller.getSnapshot().undoRedoQueued).toBe(true);
    gate.open();
    expect(await draft).toBe(true);
    await controller.whenIdle();
    // The draft landed and was undone: the restored snapshot is the one before it.
    expect(openedWith).toEqual([afterMove]);
    expect(controller.getSnapshot().canRedo).toBe(true);
    expect(errors).toEqual([]);
    await controller.dispose();
  });

  it('a failing draft edit discards the undo queued behind it, so no older edit is undone', async () => {
    const { controller, errors, openedWith } = await openController({
      applyPatchThrows: (_p, i) => (i === 1 ? new Error('rejected') : undefined),
    });
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    const draft = controller.enqueueModelEdit({ label: 'units', buildPatch: () => ({ models: [] }) });
    controller.undoRedo('undo', { afterQueuedEdits: true });
    expect(controller.getSnapshot().undoRedoQueued).toBe(true);
    expect(await draft).toBe(false);
    await controller.whenIdle();
    expect(openedWith).toHaveLength(0);
    expect(errors.map((e) => e.message)).toEqual(['rejected']);
    expect(controller.getSnapshot().canUndo).toBe(true);
    await controller.dispose();
  });

  it('afterQueuedEdits still requires history in its own direction', async () => {
    const { controller } = await openController();
    // No history at all.
    controller.undoRedo('undo', { afterQueuedEdits: true });
    expect(controller.getSnapshot().undoRedoQueued).toBe(false);
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.whenIdle();
    // History to undo, none to redo.
    controller.undoRedo('redo', { afterQueuedEdits: true });
    expect(controller.getSnapshot().undoRedoQueued).toBe(false);
    controller.undoRedo('undo', { afterQueuedEdits: true });
    expect(controller.getSnapshot().undoRedoQueued).toBe(true);
    await controller.whenIdle();
    await controller.dispose();
  });

  it('a second undo is refused while one is queued, with or without afterQueuedEdits', async () => {
    const { controller, openedWith } = await openController();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.whenIdle();
    controller.undoRedo('undo');
    controller.undoRedo('undo');
    controller.undoRedo('undo', { afterQueuedEdits: true });
    await controller.whenIdle();
    expect(openedWith).toHaveLength(1);
    expect((controller as unknown as { projectOffset: number }).projectOffset).toBe(1);
    await controller.dispose();
  });

  it('an edit landing after an undo discards the redo branch', async () => {
    const { controller, openedWith } = await openController();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await controller.whenIdle();
    controller.undoRedo('undo');
    await controller.whenIdle();
    expect(controller.getSnapshot().canRedo).toBe(true);
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 30) });
    await controller.whenIdle();
    expect(controller.getSnapshot().canRedo).toBe(false);
    const opens = openedWith.length;
    controller.undoRedo('redo');
    await controller.whenIdle();
    expect(openedWith).toHaveLength(opens);
    // The history is the open, the first move and the new move.
    expect((controller as unknown as { projectHistory: unknown[] }).projectHistory).toHaveLength(3);
    await controller.dispose();
  });

  it('a failed reopen keeps the current engine and history cursor', async () => {
    const engine = makeFakeEngine();
    let opens = 0;
    const { config, errors } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController({
      ...config,
      openProtobuf: async () => {
        opens++;
        throw new Error('reopen failed');
      },
    });
    await controller.openInitialProject();
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(controller.getView()!, 1, 10) });
    await controller.whenIdle();
    controller.undoRedo('undo');
    await controller.whenIdle();
    expect(opens).toBe(1);
    expect(errors.map((e) => e.message)).toEqual(['opening the project in the engine failed: reopen failed']);
    expect(controller.getSnapshot().canUndo).toBe(true);
    expect(engine.disposeCount).toBe(0);
    await controller.dispose();
  });

  it('undo restoring a project lacking the viewed model resets navigation and bumps navResetSeq', async () => {
    let includeChild = true;
    const engine = makeFakeEngine({
      json: () =>
        includeChild
          ? validProjectJson({
              extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
            })
          : validProjectJson(),
    });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.enqueueModelEdit({ label: 'edit', buildPatch: () => ({ models: [] }) });
    await controller.whenIdle();
    expect(
      controller.drillIntoModule('m', 'child', new Set([1]), { x: 0, y: 0, width: 1, height: 1 }, 1).restoredSelection,
    ).toEqual(new Set());
    const navSeqBefore = controller.getSnapshot().navResetSeq;
    includeChild = false;
    controller.undoRedo('undo');
    await controller.whenIdle();
    const s = controller.getSnapshot();
    expect(s.modelName).toBe('main');
    expect(s.modelStack).toHaveLength(0);
    expect(s.navResetSeq).toBe(navSeqBefore + 1);
    await controller.dispose();
  });
});

describe('ProjectController maintenance', () => {
  function countBefore(calls: readonly string[], call: string, marker: string): number {
    const end = calls.indexOf(marker);
    return (end === -1 ? calls : calls.slice(0, end)).filter((c) => c === call).length;
  }

  it('a burst of edits costs one save and one sim run', async () => {
    const gate = makeGate();
    const { controller, engine, saves } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    const runsBefore = engine.runCalls.length;
    for (let i = 0; i < 3; i++) {
      void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    }
    gate.open();
    await controller.whenIdle();
    expect(saves).toHaveLength(1);
    expect(engine.runCalls.length - runsBefore).toBe(1);
    // Nothing ran between the queued edits.
    expect(
      countBefore(engine.calls.slice(engine.calls.indexOf('applyPatch')), 'applyPatch', 'serializeJson:save'),
    ).toBe(3);
    await controller.dispose();
  });

  it(`maintenance runs after ${MaintenanceEditBound} consecutive edit items even while more are queued`, async () => {
    const { controller, engine, saves } = await openController();
    const start = engine.calls.length;
    for (let i = 0; i < MaintenanceEditBound + 2; i++) {
      void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    }
    await controller.whenIdle();
    const calls = engine.calls.slice(start);
    expect(countBefore(calls, 'applyPatch', 'serializeJson:save')).toBe(MaintenanceEditBound);
    // One save at the bound, one after the burst drains.
    expect(saves).toHaveLength(2);
    // The bound runs every pending kind, not just the first.
    const patchAt = calls.flatMap((c, i) => (c === 'applyPatch' ? [i] : []));
    const atBound = calls.slice(patchAt[MaintenanceEditBound - 1] + 1, patchAt[MaintenanceEditBound]);
    for (const call of ['serializeJson:save', 'getErrors', 'getIncomingLinks', 'run']) {
      expect(atBound).toContain(call);
    }
    await controller.dispose();
  });

  it('a landed edit refreshes errors and connector dependencies', async () => {
    let errorList: ErrorDetail[] = [];
    const { controller, engine } = await openController({
      errors: () => errorList,
      onApplyPatch: () => {
        errorList = [
          { modelName: 'main', variableName: 'a', kind: SimlinErrorKind.Variable, code: 1 } as unknown as ErrorDetail,
        ];
      },
    });
    expect(controller.getSnapshot().cachedErrors.varErrors.has('a')).toBe(false);
    await controller.enqueueModelEdit({ label: 'equation', buildPatch: () => ({ models: [] }) });
    await controller.whenIdle();
    expect(controller.getSnapshot().cachedErrors.varErrors.has('a')).toBe(true);
    expect(engine.calls.lastIndexOf('getIncomingLinks')).toBeGreaterThan(engine.calls.lastIndexOf('applyPatch'));
    await controller.dispose();
  });

  it('maintenance runs once continuous edit work passes the time bound', async () => {
    let clock = 0;
    const { controller, engine } = await openController(
      {
        applyPatchGate: async (_p, i) => {
          if (i === 0) {
            clock += 6000;
          }
        },
      },
      { now: () => clock },
    );
    const start = engine.calls.length;
    for (let i = 0; i < 3; i++) {
      void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    }
    await controller.whenIdle();
    expect(countBefore(engine.calls.slice(start), 'applyPatch', 'serializeJson:save')).toBe(1);
    await controller.dispose();
  });

  it('a save serializes the committed state: never while an edit is in flight', async () => {
    const gate = makeGate();
    const { controller, engine, saves } = await openController({ applyPatchGate: () => gate.wait() });
    const landed = controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller), aux(2, 'b')),
    });
    controller.requestSave();
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(engine.calls.filter((c) => c === 'serializeJson:save')).toHaveLength(0);
    gate.open();
    await landed;
    await controller.whenIdle();
    expect(saves).toHaveLength(1);
    const saved = JSON.parse(saves[0].project.data as string) as JsonProject;
    const savedUids = (saved.models[0].views![0] as { elements: Array<{ uid: number }> }).elements.map((el) => el.uid);
    expect(savedUids.sort()).toEqual([1, 2]);
    await controller.dispose();
  });

  it('every engine call runs through one executor: no two ever overlap', async () => {
    const { controller, engine } = await openController();
    for (let i = 0; i < 4; i++) {
      void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
      controller.setViewport('main', { viewBox: { x: i, y: 0, width: 800, height: 600 }, zoom: 1 });
      void controller.query((e) => e.getErrors());
      controller.requestSave();
    }
    controller.drillIntoModule('m', 'main', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    await controller.whenIdle();
    expect(engine.maxConcurrentCalls).toBe(1);
    await controller.dispose();
  });
});

describe('ProjectController viewport items', () => {
  it('render at once, persist once with no history and no save', async () => {
    const { controller, engine, saves } = await openController();
    controller.setViewport('main', { viewBox: { x: 5, y: 6, width: 800, height: 600 }, zoom: 2 });
    expect(view(controller).zoom).toBe(2);
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(1);
    expect(controller.getSnapshot().canUndo).toBe(false);
    expect(saves).toHaveLength(0);
    await controller.dispose();
  });

  it('coalesce: two settles while an item runs persist once, with the latest viewport', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    controller.setViewport('main', { viewBox: { x: 1, y: 0, width: 800, height: 600 }, zoom: 1.5 });
    controller.setViewport('main', { viewBox: { x: 2, y: 0, width: 800, height: 600 }, zoom: 2.5 });
    gate.open();
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(2);
    expect(viewOps(engine.appliedPatches[1])[0].payload.view.zoom).toBe(2.5);
    await controller.dispose();
  });

  it('a rejected viewport persist renders the committed viewport again, and an edit planned on it upserts the committed one', async () => {
    const { controller, engine, errors } = await openController({
      applyPatchThrows: (_p, i) => (i === 0 ? new Error('view rejected') : undefined),
    });
    controller.setViewport('main', { viewBox: { x: 9, y: 9, width: 800, height: 600 }, zoom: 2 });
    expect(view(controller).zoom).toBe(2);
    // Planned while the live viewport rendered.
    const move = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    expect(await move).toBe(true);
    await controller.whenIdle();
    expect(errors.map((e) => e.message)).toEqual(['view rejected']);
    expect(view(controller).zoom).toBe(1);
    expect(engine.appliedPatches).toHaveLength(1);
    expect(viewOps(engine.appliedPatches[0])[0].payload.view.zoom).toBe(1);
    await controller.dispose();
  });

  it('a rejected viewport persist keeps a newer viewport set while it ran, and that one persists', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
      applyPatchThrows: (_p, i) => (i === 0 ? new Error('view rejected') : undefined),
    });
    controller.setViewport('main', { viewBox: { x: 1, y: 0, width: 800, height: 600 }, zoom: 2 });
    await new Promise((resolve) => setTimeout(resolve, 10));
    controller.setViewport('main', { viewBox: { x: 2, y: 0, width: 800, height: 600 }, zoom: 3 });
    gate.open();
    await controller.whenIdle();
    expect(view(controller).zoom).toBe(3);
    expect(engine.appliedPatches).toHaveLength(1);
    expect(viewOps(engine.appliedPatches[0])[0].payload.view.zoom).toBe(3);
    await controller.dispose();
  });

  it('never carry optimistic elements of an edit queued behind them', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    void controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    controller.setViewport('main', { viewBox: { x: 3, y: 0, width: 800, height: 600 }, zoom: 1.25 });
    void controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(2, 'b')) });
    expect(uids(view(controller))).toEqual([1, 2]);
    gate.open();
    await controller.whenIdle();
    const [, viewportPatch, createPatch] = engine.appliedPatches;
    expect(viewOps(viewportPatch)[0].payload.view.elements.map((el) => el.uid)).toEqual([1]);
    expect(
      viewOps(createPatch)[0]
        .payload.view.elements.map((el) => el.uid)
        .sort(),
    ).toEqual([1, 2]);
    await controller.dispose();
  });
});

describe('ProjectController pending renames', () => {
  const json = validProjectJson({
    auxiliaries: [
      { name: 'a', equation: '1' },
      { name: 'b', equation: 'a' },
    ],
    mainViewElements: [
      { type: 'aux', uid: 1, name: 'a', x: 0, y: 0 },
      { type: 'aux', uid: 2, name: 'b', x: 50, y: 0 },
      { type: 'link', uid: 3, fromUid: 1, toUid: 2 },
    ],
    extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
  });

  function renamed(v: StockFlowView, uid: number, name: string): StockFlowView {
    return {
      ...v,
      elements: v.elements.map((el) =>
        el.uid === uid && el.type === 'aux' ? { ...el, name, ident: canonicalize(name) } : el,
      ),
    };
  }

  it("the rendered model names a pending rename's variable by its new ident, with its committed content, errors and connectors", async () => {
    const gate = makeGate();
    const engine = makeFakeEngine({
      json,
      errors: [
        {
          modelName: 'main',
          variableName: 'a',
          kind: SimlinErrorKind.Units,
          code: 1,
          unitErrorKind: 1,
        } as unknown as ErrorDetail,
      ],
      incomingLinks: { a: [], b: ['a'] },
      applyPatchGate: () => gate.wait(),
      applyPatchThrows: true,
    });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    const committedA = controller.getModel()!.variables.get('a')!;
    expect(committedA.unitErrors).toBeDefined();

    const renaming = controller.enqueueViewEdit({ label: 'rename', nextView: renamed(view(controller), 1, 'Alpha') });
    const model = controller.getModel()!;
    expect(model.variables.has('a')).toBe(false);
    const alpha = model.variables.get('alpha')!;
    expect(alpha).toMatchObject({ ident: 'alpha', rawName: 'Alpha', equation: committedA.equation });
    expect(alpha.unitErrors).toEqual(committedA.unitErrors);
    // b's dependency on a is the link from the renamed element: no drift.
    expect(model.variables.get('b')!.connectorErrors).toBeUndefined();
    // Renaming back is free while the rename is pending; the new name is taken.
    expect(controller.nameError('A', 'alpha')).toBeUndefined();
    expect(controller.nameError('alpha', undefined)).toBeDefined();
    // Every model renders its pending renames, not just the one being viewed.
    controller.drillIntoModule('m', 'child', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    expect(controller.getSnapshot().project!.models.get('main')!.variables.has('alpha')).toBe(true);
    controller.navigateBack();

    // The rename fails: the committed ident renders again.
    gate.open();
    expect(await renaming).toBe(false);
    await controller.whenIdle();
    expect(controller.getModel()!.variables.has('a')).toBe(true);
    expect(controller.getModel()!.variables.has('alpha')).toBe(false);
    await controller.dispose();
  });
});

describe('ProjectController per-model rendering during navigation', () => {
  const childJson = validProjectJson({
    auxiliaries: [{ name: 'a', equation: '1' }],
    mainViewElements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
    extraModels: [
      {
        name: 'child',
        stocks: [],
        flows: [],
        auxiliaries: [{ name: 'c', equation: '1' }],
        views: [
          {
            elements: [{ type: 'aux', uid: 1, name: 'c', x: 0, y: 0 }],
            viewBox: { x: 5, y: 6, width: 300, height: 200 },
            zoom: 2,
          },
        ],
      },
    ],
  });

  async function openWithChild(engineOptions: FakeEngineOptions = {}) {
    const project = statefulProject(childJson);
    const engine = makeFakeEngine({ json: project.json, onApplyPatch: project.apply, ...engineOptions });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    return { controller, engine };
  }

  it('a pending main edit keeps rendering main while the child is viewed; each edit targets its own model', async () => {
    const gate = makeGate();
    const { controller, engine } = await openWithChild({
      applyPatchGate: (_p, i) => (i === 0 ? gate.wait() : Promise.resolve()),
    });
    void controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(2, 'b')) });
    const main = view(controller);
    controller.drillIntoModule('m', 'child', new Set(), main.viewBox, main.zoom);
    expect(controller.getSnapshot().modelName).toBe('child');
    expect(uids(view(controller, 'main'))).toEqual([1, 2]);
    expect(uids(view(controller, 'child'))).toEqual([1]);

    void controller.enqueueViewEdit({
      label: 'create',
      nextView: withElements(view(controller, 'child'), aux(3, 'd')),
    });
    expect(uids(view(controller, 'child'))).toEqual([1, 3]);
    expect(uids(view(controller, 'main'))).toEqual([1, 2]);
    gate.open();
    await controller.whenIdle();
    expect(engine.appliedPatches.map((p) => p.models![0].name)).toEqual(['main', 'child']);
    expect(
      viewOps(engine.appliedPatches[1], 'child')[0]
        .payload.view.elements.map((el) => el.uid)
        .sort(),
    ).toEqual([1, 3]);
    await controller.dispose();
  });

  it("navigating back restores the parent's selection and viewport through a viewport item", async () => {
    const { controller, engine } = await openWithChild();
    const parentSelection = new Set([42]);
    controller.drillIntoModule('m', 'child', parentSelection, { x: 11, y: 12, width: 640, height: 480 }, 1.5);
    const back = controller.navigateBack();
    expect(back.restoredSelection).toEqual(parentSelection);
    expect(view(controller, 'main')).toMatchObject({ viewBox: { x: 11, y: 12, width: 640, height: 480 }, zoom: 1.5 });
    await controller.whenIdle();
    expect(engine.appliedPatches).toHaveLength(1);
    expect(engine.appliedPatches[0].models![0].name).toBe('main');
    expect(viewOps(engine.appliedPatches[0])[0].payload.view.zoom).toBe(1.5);
    await controller.dispose();
  });

  it('refuses to drill into a missing model', async () => {
    const { controller } = await openWithChild();
    expect(
      controller.drillIntoModule('m', 'nope', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1).restoredSelection,
    ).toBeUndefined();
    expect(controller.getModelName()).toBe('main');
    expect(controller.navigateBack().restoredSelection).toBeUndefined();
    expect(controller.navigateToLevel(0).restoredSelection).toBeUndefined();
    await controller.dispose();
  });
});

describe('ProjectController saves and server-version bookkeeping (#958)', () => {
  it('keeps sending the last server-acknowledged version across many edits whose saves fail', async () => {
    const sent: number[] = [];
    const { controller } = await openController(
      {},
      {
        initialVersion: 5,
        save: async (_project, currVersion) => {
          sent.push(currVersion);
          return undefined;
        },
      },
    );
    for (let i = 0; i < 12; i++) {
      await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 1) });
      await controller.whenIdle();
    }
    expect(sent.length).toBeGreaterThan(0);
    expect(sent.every((v) => v === 5)).toBe(true);
    expect(controller.getSnapshot().serverVersion).toBe(5);
    await controller.dispose();
  });

  it('a successful save advances serverVersion and later saves send it', async () => {
    const sent: number[] = [];
    const responses = [9, undefined];
    const { controller } = await openController(
      {},
      {
        initialVersion: 5,
        save: async (_project, currVersion) => {
          sent.push(currVersion);
          return responses.shift();
        },
      },
    );
    controller.requestSave();
    await controller.whenIdle();
    expect(controller.getSnapshot().serverVersion).toBe(9);
    controller.requestSave();
    await controller.whenIdle();
    expect(sent).toEqual([5, 9]);
    await controller.dispose();
  });

  it('a save requested while one is in flight queues exactly one flush of the latest state', async () => {
    const hostGate = makeGate();
    const saved: Array<{ version: number; uids: number[] }> = [];
    let first = true;
    const { controller } = await openController(
      {},
      {
        save: async (project, currVersion) => {
          const data = JSON.parse(project.data as string) as JsonProject;
          saved.push({
            version: currVersion,
            uids: (data.models[0].views![0] as { elements: Array<{ uid: number }> }).elements
              .map((el) => el.uid)
              .sort(),
          });
          if (first) {
            first = false;
            await hostGate.wait();
          }
          return currVersion + 1;
        },
      },
    );
    controller.requestSave();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(saved).toHaveLength(1);
    // Two more edits land while the first save is in flight: each requests a
    // save; only the latest serialization is flushed, once.
    await controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(2, 'b')) });
    await new Promise((resolve) => setTimeout(resolve, 20));
    await controller.enqueueViewEdit({ label: 'create', nextView: withElements(view(controller), aux(3, 'c')) });
    await new Promise((resolve) => setTimeout(resolve, 20));
    hostGate.open();
    await controller.whenIdle();
    expect(saved).toEqual([
      { version: 1, uids: [1] },
      { version: 2, uids: [1, 2, 3] },
    ]);
    await controller.dispose();
  });

  it('a thrown host save releases the latch and the queued flush still goes out', async () => {
    let calls = 0;
    const { controller, errors } = await openController(
      {},
      {
        save: async () => {
          calls++;
          if (calls === 1) {
            await new Promise((resolve) => setTimeout(resolve, 10));
            throw new Error('network failure');
          }
          return 5;
        },
      },
    );
    controller.requestSave();
    await new Promise((resolve) => setTimeout(resolve, 5));
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 1) });
    await controller.whenIdle();
    expect(calls).toBe(2);
    expect(errors.map((e) => e.message)).toEqual(['network failure']);
    await controller.dispose();
  });

  it('saves without stdlib models while the rendered project includes them', async () => {
    const engine = makeFakeEngine({ json: (includeStdlib) => validProjectJson({ includeStdlib }) });
    const saved: string[] = [];
    const { config } = makeControllerConfig({
      engine,
      format: 'json',
      save: async (project) => {
        saved.push(project.data as string);
        return 2;
      },
    });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    expect([...(controller.getSnapshot().project?.models.keys() ?? [])].some(isStdlibModel)).toBe(true);
    expect(controller.getSnapshot().project?.models.has(FAKE_STDLIB_MODEL_NAME)).toBe(true);
    controller.requestSave();
    await controller.whenIdle();
    expect(saved).toHaveLength(1);
    expect(
      (JSON.parse(saved[0]) as { models: Array<{ name: string }> }).models.some((m) => isStdlibModel(m.name)),
    ).toBe(false);
    await controller.dispose();
  });

  it('the viewport stream advances the render key but never saves or moves the server version', async () => {
    const { controller, saves } = await openController({}, { initialVersion: 5 });
    const before = controller.getSnapshot();
    for (let i = 0; i < 5; i++) {
      controller.setViewport('main', { viewBox: { x: i, y: 0, width: 800, height: 600 }, zoom: 1 + i });
      await controller.whenIdle();
    }
    const after = controller.getSnapshot();
    expect(after.projectVersion).toBeGreaterThan(before.projectVersion);
    expect(saves).toHaveLength(0);
    expect(after.serverVersion).toBe(5);
    await controller.dispose();
  });

  it('a save acknowledgment republishes without replacing the rendered project (no Canvas re-cache)', async () => {
    const { controller } = await openController({}, { initialVersion: 5, save: async () => 9 });
    const before = controller.getSnapshot();
    controller.requestSave();
    await controller.whenIdle();
    const after = controller.getSnapshot();
    expect(after.serverVersion).toBe(9);
    expect(after.project).toBe(before.project);
    expect(after.projectVersion).toBe(before.projectVersion);
    await controller.dispose();
  });

  it('undo does not rewind the server-acknowledged version', async () => {
    const sent: number[] = [];
    let acked = false;
    const { controller } = await openController(
      {},
      {
        initialVersion: 5,
        save: async (_project, currVersion) => {
          sent.push(currVersion);
          if (!acked) {
            acked = true;
            return 6;
          }
          return undefined;
        },
      },
    );
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 1) });
    await controller.whenIdle();
    expect(controller.getSnapshot().serverVersion).toBe(6);
    await controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 1) });
    await controller.whenIdle();
    controller.undoRedo('undo');
    await controller.whenIdle();
    expect(controller.getSnapshot().serverVersion).toBe(6);
    expect(sent[sent.length - 1]).toBe(6);
    await controller.dispose();
  });
});

describe('ProjectController sim runs', () => {
  it('attaches sim data to main and falls back to non-LTM on first-run failure', async () => {
    let firstRun = true;
    const { controller, engine, errors } = await openController({
      run: () => {
        if (firstRun) {
          firstRun = false;
          throw new Error('LTM blew up');
        }
        return fakeRun({ time: [0, 1], a: [10, 20] });
      },
    });
    expect(errors.map((e) => e.message)).toEqual(['LTM blew up']);
    expect(engine.runCalls).toHaveLength(2);
    expect(engine.runCalls[1].analyzeLtm).toBe(false);
    expect(controller.getSnapshot().data.has('a')).toBe(true);
    expect(controller.getSnapshot().project?.models.get('main')?.variables.get('a')?.data).toBeDefined();
    await controller.dispose();
  });

  it('does not run an unsimulatable project and reports status error', async () => {
    const { controller, engine } = await openController({ simulatable: false });
    expect(engine.runCalls).toHaveLength(0);
    expect(controller.getSnapshot().status).toBe('error');
    await controller.dispose();
  });
});

describe('convertErrorDetails', () => {
  it('maps a unit error to its bare details, not the formatted message', () => {
    const errors: ErrorDetail[] = [
      {
        modelName: 'main',
        variableName: 'inflow',
        kind: SimlinErrorKind.Units,
        unitErrorKind: SimlinUnitErrorKind.Consistency,
        code: 33,
        startOffset: 0,
        endOffset: 6,
        message: "    2*aux1\n    ~~~~~~\nunits warning in model 'main' variable 'inflow': unit_mismatch",
        details: "computed units 'blerz' don't match specified units",
      } as unknown as ErrorDetail,
    ];
    const { unitErrors } = convertErrorDetails(errors, 'main');
    const errs = unitErrors.get('inflow');
    expect(errs).toHaveLength(1);
    expect(errs![0].details).toBe("computed units 'blerz' don't match specified units");
    expect(errs![0].kind).toBe('consistency');
    expect(errs![0].start).toBe(0);
    expect(errs![0].end).toBe(6);
  });

  it('maps the three-valued unit error kind through to core UnitError', () => {
    const errAt = (unitErrorKind: SimlinUnitErrorKind, variableName: string): ErrorDetail =>
      ({
        modelName: 'main',
        variableName,
        kind: SimlinErrorKind.Units,
        unitErrorKind,
        code: 33,
        startOffset: 0,
        endOffset: 3,
        message: null,
        details: null,
      }) as unknown as ErrorDetail;
    const { unitErrors } = convertErrorDetails(
      [
        errAt(SimlinUnitErrorKind.Definition, 'a'),
        errAt(SimlinUnitErrorKind.Consistency, 'b'),
        errAt(SimlinUnitErrorKind.Inference, 'c'),
      ],
      'main',
    );
    expect(unitErrors.get('a')![0].kind).toBe('definition');
    expect(unitErrors.get('b')![0].kind).toBe('consistency');
    expect(unitErrors.get('c')![0].kind).toBe('inference');
  });

  it('leaves details undefined when the engine provides none', () => {
    const errors: ErrorDetail[] = [
      {
        modelName: 'main',
        variableName: 'x',
        kind: SimlinErrorKind.Units,
        unitErrorKind: SimlinUnitErrorKind.Definition,
        code: 35,
        message: 'units error in model ...',
        details: null,
      } as unknown as ErrorDetail,
    ];
    const { unitErrors } = convertErrorDetails(errors, 'main');
    expect(unitErrors.get('x')![0].details).toBeUndefined();
    expect(unitErrors.get('x')![0].kind).toBe('definition');
  });
});

describe('ProjectController error derivation', () => {
  const errorList: ErrorDetail[] = [
    {
      modelName: 'main',
      variableName: 'a',
      kind: SimlinErrorKind.Variable,
      code: 1,
      startOffset: 0,
      endOffset: 1,
    } as unknown as ErrorDetail,
    { modelName: 'child', variableName: 'c', kind: SimlinErrorKind.Variable, code: 1 } as unknown as ErrorDetail,
    {
      modelName: 'main',
      variableName: null,
      kind: SimlinErrorKind.Model,
      code: 33,
      message: "warning in model 'main': unit_mismatch -- unit checking failed",
      details: "the units of 'a' and 'b' are inconsistent with each other",
    } as unknown as ErrorDetail,
    {
      modelName: 'main',
      variableName: null,
      kind: SimlinErrorKind.Model,
      code: 1,
      message: 'error in model main: something broke',
      details: null,
    } as unknown as ErrorDetail,
  ];

  it('scopes the error panel and variable annotations to the active model, and re-scopes on drill-in with no engine call', async () => {
    const project = statefulProject(
      validProjectJson({
        auxiliaries: [{ name: 'a', equation: '1' }],
        mainViewElements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
        extraModels: [
          {
            name: 'child',
            stocks: [],
            flows: [],
            auxiliaries: [{ name: 'c', equation: '1' }],
            views: [{ elements: [] }],
          },
        ],
      }),
    );
    const engine = makeFakeEngine({ json: project.json, errors: errorList });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    const s = controller.getSnapshot();
    expect(s.cachedErrors.varErrors.has('a')).toBe(true);
    expect(s.cachedErrors.varErrors.has('c')).toBe(false);
    expect(s.project?.models.get('main')?.variables.get('a')?.errors).toEqual([{ start: 0, end: 1, code: 1 }]);
    expect(s.cachedErrors.modelErrors.map((e) => e.details)).toEqual([
      "the units of 'a' and 'b' are inconsistent with each other",
      'error in model main: something broke',
    ]);

    const callsBefore = engine.calls.filter((c) => c === 'getErrors').length;
    controller.drillIntoModule('m', 'child', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    const child = controller.getSnapshot();
    expect(child.cachedErrors.varErrors.has('c')).toBe(true);
    expect(child.cachedErrors.varErrors.has('a')).toBe(false);
    expect(child.project?.models.get('child')?.variables.get('c')?.errors).toBeDefined();
    expect(engine.calls.filter((c) => c === 'getErrors').length).toBe(callsBefore);
    await controller.dispose();
  });

  it('flags an all-empty starter model hasNoEquations without annotating errors', async () => {
    const json = validProjectJson({ auxiliaries: [{ name: 'a' }, { name: 'b' }] });
    const empty = (variableName: string): ErrorDetail =>
      ({
        modelName: 'main',
        variableName,
        kind: SimlinErrorKind.Variable,
        code: ErrorCode.EmptyEquation,
        startOffset: 0,
        endOffset: 0,
      }) as unknown as ErrorDetail;
    const engine = makeFakeEngine({ json, errors: [empty('a'), empty('b')] });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    const s = controller.getSnapshot();
    expect(s.project?.hasNoEquations).toBe(true);
    expect(s.project?.models.get('main')?.variables.get('a')?.errors).toBeUndefined();
    expect(s.status).toBe('disabled');
    await controller.dispose();
  });
});

describe('ProjectController snapshots and disposal', () => {
  it('produces a fresh snapshot object on each change and never mutates prior ones', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    const s0 = controller.getSnapshot();
    await controller.openInitialProject();
    const s1 = controller.getSnapshot();
    expect(s1).not.toBe(s0);
    expect(s0.project).toBeUndefined();
    await controller.dispose();
  });

  it('coalesces a synchronous navigation into a single notification', async () => {
    const project = statefulProject(
      validProjectJson({
        extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
      }),
    );
    const engine = makeFakeEngine({ json: project.json });
    const { config } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    await controller.whenIdle();
    const seen: ProjectSnapshot[] = [];
    controller.subscribe(() => {
      seen.push(controller.getSnapshot());
    });
    controller.drillIntoModule('m', 'child', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    expect(seen).toHaveLength(1);
    expect(seen[0].modelName).toBe('child');
    await controller.dispose();
  });

  it('dispose settles queued items as not landed, releases the engine after the running item, and stops notifying', async () => {
    const gate = makeGate();
    const { controller, engine } = await openController({ applyPatchGate: () => gate.wait() });
    let notifies = 0;
    controller.subscribe(() => {
      notifies++;
    });
    const running = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    const queued = controller.enqueueViewEdit({ label: 'move', nextView: moved(view(controller), 1, 10) });
    await new Promise((resolve) => setTimeout(resolve, 10));
    notifies = 0;
    const disposing = controller.dispose();
    expect(await queued).toBe(false);
    expect(engine.disposeCount).toBe(0);
    gate.open();
    await running;
    await disposing;
    expect(engine.disposeCount).toBe(1);
    expect(notifies).toBe(0);
    // Post-dispose calls are inert.
    expect(await controller.enqueueModelEdit({ label: 'x', buildPatch: () => ({ models: [] }) })).toBe(false);
    controller.setViewport('main', { viewBox: { x: 1, y: 1, width: 1, height: 1 }, zoom: 1 });
    controller.undoRedo('undo');
    expect(await controller.query(async () => 1)).toBeUndefined();
    rs.restoreAllMocks();
  });

  it('dispose of an idle controller releases the engine', async () => {
    const { controller, engine } = await openController();
    expect(engine.disposeCount).toBe(0);
    await controller.dispose();
    expect(engine.disposeCount).toBe(1);
  });

  it('dispose in the same tick as an enqueue settles the item the executor has not started, and whenIdle resolves', async () => {
    const engine = makeFakeEngine();
    const { config, openedEngines } = makeControllerConfig({ engine, format: 'json' });
    const controller = new ProjectController(config);
    const opening = controller.openInitialProject();
    await controller.dispose();
    const hung = (ms: number) => new Promise<string>((resolve) => setTimeout(() => resolve('hung'), ms));
    expect(await Promise.race([opening.then(() => 'settled'), hung(100)])).toBe('settled');
    expect(await Promise.race([controller.whenIdle().then(() => 'idle'), hung(100)])).toBe('idle');
    expect(openedEngines).toHaveLength(0);
    // Anything pushed from here on settles at once, too.
    expect(await Promise.race([controller.openInitialProject().then(() => 'settled'), hung(100)])).toBe('settled');
  });
});

// Sanity that the JSON fixture round-trips through the real datamodel parser
// the controller relies on -- catches fixture drift independent of the engine.
describe('fake-engine fixture', () => {
  it('validProjectJson parses through projectFromJson', () => {
    const project = projectFromJson(JSON.parse(validProjectJson()) as JsonProject);
    expect(project.name).toBe('test');
    expect(project.models.has('main')).toBe(true);
  });
});
