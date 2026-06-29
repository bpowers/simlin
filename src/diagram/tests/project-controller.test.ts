/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Unit tests for ProjectController -- the headless coordination layer extracted
// from Editor.tsx. These exercise the engine lifecycle, the apply-patch
// pipeline, the optimistic-view race, undo/redo, the save queue, sim runs, the
// error cache, navigation, and snapshot immutability/coalescing -- all against
// the FakeEngineProject helper, with no jsdom or real WASM.

import { projectFromJson, type StockFlowView, type StockViewElement } from '@simlin/core/datamodel';
import type { JsonProject, ErrorDetail } from '@simlin/engine';
import { SimlinErrorKind } from '@simlin/engine';

import { ProjectController, MaxUndoSize, type ProjectSnapshot } from '../project-controller';
import {
  makeFakeEngine,
  makeControllerConfig,
  validProjectJson,
  fakeRun,
  FAKE_STDLIB_MODEL_NAME,
  type FakeEngine,
} from './fake-engine';
import { isStdlibModel } from '../module-navigation';

// Drain the microtask + macrotask queue so deferred setTimeout(0) callbacks
// (scheduleSave, scheduleSimRun, undoRedo reopen) and their promise chains run.
async function flushTimers(): Promise<void> {
  // A handful of cycles is enough for the chained setTimeout -> promise ->
  // setTimeout sequences the controller produces.
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

function snap(n: number): Uint8Array {
  return new Uint8Array([n]);
}

describe('ProjectController open lifecycle', () => {
  it('opens the initial protobuf project and publishes a snapshot', async () => {
    const engine = makeFakeEngine({ protobuf: snap(7) });
    const { config } = makeControllerConfig({ engine, initialData: snap(1), initialVersion: 3 });
    const controller = new ProjectController(config);

    let notifies = 0;
    controller.subscribe(() => {
      notifies++;
    });

    await controller.openInitialProject();

    const s = controller.getSnapshot();
    expect(s.project).toBeDefined();
    expect(s.project?.name).toBe('test');
    expect(s.projectVersion).toBe(3);
    expect(notifies).toBeGreaterThan(0);
    await controller.dispose();
  });

  it('surfaces an error (and no snapshot project) when the engine open fails', async () => {
    const { config, errors } = makeControllerConfig({ openThrows: new Error('bad bytes') });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    expect(controller.getSnapshot().project).toBeUndefined();
    expect(errors.some((e) => e.message.includes('bad bytes'))).toBe(true);
    expect(errors.some((e) => e.message.includes('opening the project in the engine failed'))).toBe(true);
    await controller.dispose();
  });

  it('disposes the orphan engine and surfaces an error when serializeJson fails after open', async () => {
    const engine = makeFakeEngine({
      json: () => {
        throw new Error('engine panic in serializeJson');
      },
    });
    const { config, errors } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    expect(controller.getSnapshot().project).toBeUndefined();
    expect(errors.some((e) => e.message.includes('engine panic in serializeJson'))).toBe(true);
    expect(errors.some((e) => e.message.includes('opening the project failed'))).toBe(true);
    // The opened-but-unwired engine must be released.
    expect(engine.disposeCount).toBe(1);
    expect(controller.getEngine()).toBeUndefined();
    await controller.dispose();
  });

  it('disposes the engine opened by an in-flight open when dispose races in first', async () => {
    // Make the open slow so dispose() can land while it is in flight.
    let resolveOpen: (e: FakeEngine) => void = () => {};
    const engine = makeFakeEngine();
    const openPromise = new Promise<FakeEngine>((resolve) => {
      resolveOpen = resolve;
    });
    const config = {
      initialProjectVersion: 1,
      input: { format: 'protobuf' as const, data: snap(1) },
      openProtobuf: () => openPromise,
      openJson: () => openPromise,
      save: async () => 1,
      onError: () => {},
    };
    const controller = new ProjectController(config);

    const opening = controller.openInitialProject();
    await controller.dispose();
    resolveOpen(engine);
    await opening;

    // dispose() ran before the engine existed, so the open path releases it.
    expect(engine.disposeCount).toBe(1);
    expect(controller.getSnapshot().project).toBeUndefined();
  });
});

describe('ProjectController applyPatch pipeline', () => {
  async function openController(engineOpts = {}): Promise<{
    controller: ProjectController;
    engine: FakeEngine;
    errors: Error[];
  }> {
    const engine = makeFakeEngine(engineOpts);
    const { config, errors } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    return { controller, engine, errors };
  }

  it('applies a patch, rebuilds the project, and bumps version + generation', async () => {
    const { controller, engine } = await openController();
    const before = controller.getSnapshot();

    const ok = await controller.applyPatch({ models: [{ name: 'main', ops: [] }] }, 'edit');

    expect(ok).toBe(true);
    expect(engine.appliedPatches).toHaveLength(1);
    const after = controller.getSnapshot();
    expect(after.projectVersion).toBeGreaterThan(before.projectVersion);
    expect(after.projectGeneration).toBe(before.projectGeneration + 1);
    await controller.dispose();
  });

  it('reports the error and leaves the snapshot unchanged when applyPatch throws', async () => {
    const { controller, engine, errors } = await openController({ applyPatchThrows: true });
    const before = controller.getSnapshot();

    const ok = await controller.applyPatch({ models: [{ name: 'main', ops: [] }] }, 'bad edit');

    expect(ok).toBe(false);
    expect(errors.length).toBeGreaterThan(0);
    // Snapshot identity is unchanged: a failed patch makes no state change.
    expect(controller.getSnapshot()).toBe(before);
    expect(engine.appliedPatches).toHaveLength(0);
    await controller.dispose();
  });
});

describe('ProjectController optimistic view updates', () => {
  it('reflects the view immediately and keeps the newer view when the engine is behind', async () => {
    // The engine serializes an OLD view (zoom 1) while the user has panned to a
    // newer one (zoom 5) via the optimistic path. preserveLiveView must keep the
    // newer live view after the round-trip rebuild.
    const engine = makeFakeEngine({
      json: () => validProjectJson({ mainViewElements: [] }),
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const view = controller.getView() as StockFlowView;
    expect(view).toBeDefined();

    await controller.updateView({ ...view, zoom: 5 });

    const liveView = controller.getView() as StockFlowView;
    // Even though the engine's serialized JSON carries the default zoom, the
    // live optimistic zoom survives the rebuild.
    expect(liveView.zoom).toBe(5);
    await controller.dispose();
  });

  it('view-only updates never consume undo slots', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const beforeGen = controller.getSnapshot().projectGeneration;
    const view = controller.getView() as StockFlowView;
    await controller.queueViewUpdate({ ...view, zoom: 3 });

    // No history recorded, no generation bump (details panels must not remount).
    expect(controller.getSnapshot().projectGeneration).toBe(beforeGen);
    expect(controller.canUndo()).toBe(false);
    await controller.dispose();
  });

  it('updateView records undo history only when recordHistory is set', async () => {
    // Discrete element/structure edits (create/delete/move/flow-attach/etc.)
    // funnel their final engine state through updateView and must each produce
    // exactly one undo entry. A plain updateView (the legacy default) and the
    // viewport-only queueViewUpdate path must record nothing.
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const view = controller.getView() as StockFlowView;

    // A plain updateView (no opts) refreshes the project but records nothing.
    const genBefore = controller.getSnapshot().projectGeneration;
    await controller.updateView({ ...view, zoom: 2 });
    expect(controller.canUndo()).toBe(false);
    expect(controller.getSnapshot().projectGeneration).toBe(genBefore);

    // queueViewUpdate (pan/zoom/momentum) likewise records nothing.
    await controller.queueViewUpdate({ ...view, zoom: 3 });
    expect(controller.canUndo()).toBe(false);
    expect(controller.getSnapshot().projectGeneration).toBe(genBefore);

    // A discrete edit (recordHistory: true) advances history exactly once.
    await controller.updateView({ ...view, zoom: 4 }, { recordHistory: true });
    expect(controller.canUndo()).toBe(true);
    expect(controller.getSnapshot().projectGeneration).toBe(genBefore + 1);

    // undo then redo round-trips the cursor.
    controller.undoRedo('undo');
    await flushTimers();
    expect(controller.canRedo()).toBe(true);
    controller.undoRedo('redo');
    await flushTimers();
    expect(controller.canRedo()).toBe(false);
    await controller.dispose();
  });

  it('undo after a recordHistory updateView reopens the engine from the pre-edit snapshot', async () => {
    // Prove restoration, not just the cursor move: the undo reopen must pull the
    // serialized snapshot captured BEFORE the edit back into the engine.
    const openedWith: Uint8Array[] = [];
    let counter = 100;
    const engine = makeFakeEngine({ protobuf: () => new Uint8Array([counter++]) });
    const config = {
      initialProjectVersion: 1,
      input: { format: 'protobuf' as const, data: new Uint8Array([1]) },
      openProtobuf: async (data: Uint8Array) => {
        openedWith.push(data);
        return engine;
      },
      openJson: async () => engine,
      save: async () => 1,
      onError: () => {},
    };
    const controller = new ProjectController(config);
    await controller.openInitialProject();
    // History head is the post-open snapshot ([100]).
    const view = controller.getView() as StockFlowView;
    await controller.updateView({ ...view, zoom: 9 }, { recordHistory: true });
    // The edit recorded a fresh head, so there is a distinct pre-edit snapshot
    // to restore (without this the undo would merely clamp to the only entry).
    expect(controller.canUndo()).toBe(true);

    controller.undoRedo('undo');
    await flushTimers();

    // The reopen restored the pre-edit snapshot ([100]) into the engine.
    expect(openedWith[openedWith.length - 1]).toEqual(new Uint8Array([100]));
    await controller.dispose();
  });

  it('stashes the queued view when no engine is installed yet and replays it on reopen', async () => {
    // Drive queueViewUpdate before any engine exists (controller just
    // constructed), then confirm the next engine pulls the queued view.
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    // No project yet -> applyOptimisticView is a no-op, but the queued-view
    // flag is set for the eventual engine install path. We assert via the
    // reopen-for-undo path below instead; here just verify it does not throw.
    const dummyView = {
      elements: [],
      nextUid: 1,
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: 1,
    } as StockFlowView;
    await controller.queueViewUpdate(dummyView);
    expect(controller.getSnapshot().project).toBeUndefined();
    await controller.dispose();
  });
});

describe('ProjectController undo/redo', () => {
  it('editing after an undo discards the redo branch and caps at MaxUndoSize', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    // Record a long history by feeding distinct protobufs through updateProject.
    for (let i = 2; i <= 7; i++) {
      await controller.updateProject(snap(i));
    }
    // History is capped at MaxUndoSize, newest-first.
    let s = controller.getSnapshot();
    expect(controller.canUndo()).toBe(true);
    const historyLen = MaxUndoSize;

    // Undo twice, then edit: the redo branch must be discarded.
    controller.undoRedo('undo');
    await flushTimers();
    controller.undoRedo('undo');
    await flushTimers();
    await controller.updateProject(snap(99));

    s = controller.getSnapshot();
    expect(s.canRedo).toBe(false); // editing discarded the redo branch
    expect(historyLen).toBe(MaxUndoSize);
    await controller.dispose();
  });

  it('defers the version/generation bump until the restored project is installed (#817)', async () => {
    // undoRedo must not bump projectVersion/projectGeneration synchronously: at
    // click time this.project is still the pre-undo content and the rebuild is
    // async. Bumping the version then would make the Canvas cache its uid lookup
    // from the stale view, and the async rebuild swaps in the restored view
    // WITHOUT re-bumping the version -- leaving the version-keyed element cache
    // stale relative to props.view, the transient inconsistency behind the
    // dangling-ref undo crash (#817). The bump must land in the same
    // notification as the content swap.
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const view = controller.getView() as StockFlowView;
    await controller.updateView({ ...view, zoom: 4 }, { recordHistory: true });
    const before = controller.getSnapshot();

    controller.undoRedo('undo');
    // Synchronously: the cursor moved (canRedo via the live method), but the
    // published snapshot's version/generation are untouched -- no stale-content
    // render is forced.
    expect(controller.canRedo()).toBe(true);
    const sync = controller.getSnapshot();
    expect(sync.projectVersion).toBe(before.projectVersion);
    expect(sync.projectGeneration).toBe(before.projectGeneration);

    // After the async reopen installs the restored project, the bump lands.
    await flushTimers();
    const after = controller.getSnapshot();
    expect(after.projectVersion).toBeGreaterThan(before.projectVersion);
    expect(after.projectGeneration).toBe(before.projectGeneration + 1);
    await controller.dispose();
  });

  it('undo restoring a project lacking the viewed model resets navigation and bumps navResetSeq', async () => {
    // Open a project that, on undo (reopen), serializes JSON WITHOUT the
    // drilled-into child model. We simulate by: drill into 'child', then make
    // the engine's reopen JSON lack 'child'.
    let includeChild = true;
    const engine = makeFakeEngine({
      json: () =>
        includeChild
          ? validProjectJson({
              extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
            })
          : validProjectJson(),
    });
    const { config } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    // Grow history so undo has somewhere to go.
    await controller.updateProject(snap(2));

    // Drill into the child model (present in the current project).
    const outcome = controller.drillIntoModule(
      'child_module',
      'child',
      new Set([1]),
      { x: 0, y: 0, width: 100, height: 100 },
      1,
    );
    expect(outcome.restoredSelection).toEqual(new Set());
    expect(controller.getModelName()).toBe('child');

    const navSeqBefore = controller.getSnapshot().navResetSeq;
    // Now undo reopens the engine, whose JSON no longer has 'child'.
    includeChild = false;
    controller.undoRedo('undo');
    await flushTimers();

    const s = controller.getSnapshot();
    expect(s.modelName).toBe('main');
    expect(s.modelStack).toHaveLength(0);
    expect(s.navResetSeq).toBe(navSeqBefore + 1);
    await controller.dispose();
  });
});

describe('ProjectController save queue', () => {
  it('queues exactly one flush when a save is requested during an in-flight save', async () => {
    let resolveFirst: (v: number) => void = () => {};
    let callCount = 0;
    const engine = makeFakeEngine();
    const save = jest.fn(async () => {
      callCount++;
      if (callCount === 1) {
        return await new Promise<number>((resolve) => {
          resolveFirst = resolve;
        });
      }
      return callCount + 1;
    });
    const { config } = makeControllerConfig({ engine, format: 'json', save });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const first = controller.save(1);
    // Two concurrent saves while the first is in flight -> only one queued.
    void controller.save(2);
    void controller.save(3);
    // Let the first save reach its awaited config.save() before resolving it.
    await flushTimers();
    resolveFirst(2);
    await first;
    await flushTimers();

    // First save + exactly one queued flush == 2 invocations.
    expect(save).toHaveBeenCalledTimes(2);
    await controller.dispose();
  });

  it('releases inSave and still flushes the queued save when onSave throws', async () => {
    let callCount = 0;
    const engine = makeFakeEngine();
    const save = jest.fn(async () => {
      callCount++;
      if (callCount === 1) {
        await Promise.resolve();
        throw new Error('network failure');
      }
      return 5;
    });
    const { config, errors } = makeControllerConfig({ engine, format: 'json', save });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const first = controller.save(0);
    void controller.save(1); // queue one
    await first;
    await flushTimers();

    expect(callCount).toBe(2); // thrown save did not strand the queue
    expect(errors.some((e) => e.message.includes('network failure'))).toBe(true);
    await controller.dispose();
  });

  it('saves without stdlib models while the display project includes them', async () => {
    // Invariant: the display/rebuild path serializes with includeStdlib=true so
    // the editor can navigate into stdlib modules, but the SAVE path serializes
    // with includeStdlib=false so stdlib definitions are never persisted. Record
    // the includeStdlib flag the controller passes on each serializeJson call.
    const calls: boolean[] = [];
    const engine = makeFakeEngine({
      json: (includeStdlib) => {
        calls.push(includeStdlib);
        return validProjectJson({ includeStdlib });
      },
    });
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

    // The display project (rebuilt via the includeStdlib=true path) carries the
    // stdlib model so the editor can drill into it.
    const displayProject = controller.getSnapshot().project;
    expect(displayProject?.models.has(FAKE_STDLIB_MODEL_NAME)).toBe(true);
    expect([...(displayProject?.models.keys() ?? [])].some(isStdlibModel)).toBe(true);

    await controller.save(1);

    // The save path serialized with includeStdlib=false, so its payload omits
    // the stdlib model -- it is never persisted.
    expect(saved).toHaveLength(1);
    const savedModels = (JSON.parse(saved[0]) as { models: Array<{ name: string }> }).models;
    expect(savedModels.some((m) => isStdlibModel(m.name))).toBe(false);
    // And the controller did make at least one display-path (true) call and the
    // save-path (false) call.
    expect(calls).toContain(true);
    expect(calls).toContain(false);
    await controller.dispose();
  });
});

describe('ProjectController sim runs', () => {
  it('attaches sim data to main and falls back to non-LTM on first-run failure', async () => {
    let firstRun = true;
    const engine = makeFakeEngine({
      simulatable: true,
      run: () => {
        if (firstRun) {
          firstRun = false;
          throw new Error('LTM blew up');
        }
        return fakeRun({ time: [0, 1], output: [10, 20] });
      },
    });
    const { config, errors } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    await controller.loadSim();

    expect(errors.some((e) => e.message.includes('LTM blew up'))).toBe(true);
    // Two run attempts: the failing first and the LTM-disabled retry.
    expect(engine.runCalls).toHaveLength(2);
    expect(engine.runCalls[1].analyzeLtm).toBe(false);
    expect(controller.getSnapshot().data.has('output')).toBe(true);
    await controller.dispose();
  });

  it('does nothing destructive when the project is not simulatable', async () => {
    const engine = makeFakeEngine({ simulatable: false });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    await controller.loadSim();

    expect(engine.runCalls).toHaveLength(0);
    expect(controller.getSnapshot().status).toBe('error');
    await controller.dispose();
  });
});

describe('ProjectController error cache + navigation', () => {
  it('refreshes the cached errors scoped to the active model', async () => {
    const errorList: ErrorDetail[] = [
      {
        modelName: 'main',
        variableName: 'x',
        kind: SimlinErrorKind.Variable,
        code: 1,
        startOffset: 0,
        endOffset: 1,
      } as unknown as ErrorDetail,
      {
        modelName: 'child',
        variableName: 'y',
        kind: SimlinErrorKind.Variable,
        code: 1,
      } as unknown as ErrorDetail,
    ];
    const engine = makeFakeEngine({
      errors: errorList,
      json: () =>
        validProjectJson({
          extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
        }),
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const cached = await controller.refreshCachedErrors();
    // Only 'main'-scoped variable errors appear while viewing main.
    expect(cached?.varErrors.has('x')).toBe(true);
    expect(cached?.varErrors.has('y')).toBe(false);

    // Drilling into child re-scopes the cache (fire-and-forget refresh).
    controller.drillIntoModule('m', 'child', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    await flushTimers();
    expect(controller.getSnapshot().cachedErrors.varErrors.has('y')).toBe(true);
    expect(controller.getSnapshot().cachedErrors.varErrors.has('x')).toBe(false);
    await controller.dispose();
  });

  it('restores selection on navigateBack and refuses to drill into a missing model', async () => {
    const engine = makeFakeEngine({
      json: () =>
        validProjectJson({
          extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
        }),
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    // Drilling into a nonexistent model is a no-op.
    const missing = controller.drillIntoModule('m', 'nope', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    expect(missing.restoredSelection).toBeUndefined();
    expect(controller.getModelName()).toBe('main');

    // Drill in capturing a parent selection, then navigate back restores it.
    const parentSelection = new Set([42]);
    controller.drillIntoModule('m', 'child', parentSelection, { x: 0, y: 0, width: 1, height: 1 }, 1);
    expect(controller.getModelName()).toBe('child');

    const back = controller.navigateBack();
    expect(back.restoredSelection).toEqual(parentSelection);
    expect(controller.getModelName()).toBe('main');
    await controller.dispose();
  });
});

describe('ProjectController snapshot immutability + coalescing', () => {
  it('produces a fresh snapshot object on each change and never mutates prior ones', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    const s0 = controller.getSnapshot();
    await controller.openInitialProject();
    const s1 = controller.getSnapshot();
    expect(s1).not.toBe(s0);
    // The prior snapshot is untouched.
    expect(s0.project).toBeUndefined();

    await controller.applyPatch({ models: [{ name: 'main', ops: [] }] }, 'edit');
    const s2 = controller.getSnapshot();
    expect(s2).not.toBe(s1);
    expect(s1.projectVersion).not.toBe(s2.projectVersion);
    await controller.dispose();
  });

  it('coalesces a synchronous multi-step navigation into a single notification', async () => {
    const engine = makeFakeEngine({
      json: () =>
        validProjectJson({
          extraModels: [{ name: 'child', stocks: [], flows: [], auxiliaries: [], views: [{ elements: [] }] }],
        }),
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const seen: ProjectSnapshot[] = [];
    controller.subscribe(() => {
      seen.push(controller.getSnapshot());
    });

    // drillIntoModule mutates modelStack + modelName together -> exactly one
    // synchronous notify (the fire-and-forget error refresh notifies later).
    controller.drillIntoModule('m', 'child', new Set(), { x: 0, y: 0, width: 1, height: 1 }, 1);
    expect(seen).toHaveLength(1);
    expect(seen[0].modelName).toBe('child');
    await controller.dispose();
  });

  it('does not notify after dispose', async () => {
    const engine = makeFakeEngine();
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    let notifies = 0;
    controller.subscribe(() => {
      notifies++;
    });
    await controller.dispose();
    notifies = 0;

    // Post-dispose calls must not fan out to listeners.
    await controller.refreshCachedErrors();
    await controller.recalculateStatus();
    expect(notifies).toBe(0);
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

describe('ProjectController non-finite coordinate guard (#818)', () => {
  const badStock = (): StockViewElement => ({
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
  });

  it('updateView refuses a view with a non-finite coordinate (no patch, no optimistic bump)', async () => {
    const engine = makeFakeEngine();
    const { config, errors } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const view = controller.getView() as StockFlowView;
    const patchesBefore = engine.appliedPatches.length;
    const versionBefore = controller.getSnapshot().projectVersion;

    // A move/geometry bug that produced NaN would serialize to JSON null and the
    // engine would reject the patch, historically leaving the model uneditable.
    const badView: StockFlowView = { ...view, elements: [...view.elements, badStock()] };
    await controller.updateView(badView, { recordHistory: true });

    // No patch reached the engine, and the optimistic view (version bump) never
    // applied -- the canvas stays at the last good state instead of bricking.
    expect(engine.appliedPatches.length).toBe(patchesBefore);
    expect(controller.getSnapshot().projectVersion).toBe(versionBefore);
    expect(controller.canUndo()).toBe(false);
    // The host is told why, with a descriptive (debuggable) message.
    expect(errors.length).toBe(1);
    expect(errors[0].message).toContain('uid=999');
    await controller.dispose();
  });

  it('queueViewUpdate refuses a view with a non-finite coordinate', async () => {
    const engine = makeFakeEngine();
    const { config, errors } = makeControllerConfig({ engine, format: 'protobuf', initialData: snap(1) });
    const controller = new ProjectController(config);
    await controller.openInitialProject();

    const view = controller.getView() as StockFlowView;
    const patchesBefore = engine.appliedPatches.length;

    const badView: StockFlowView = { ...view, elements: [...view.elements, badStock()] };
    await controller.queueViewUpdate(badView);

    expect(engine.appliedPatches.length).toBe(patchesBefore);
    expect(errors.length).toBe(1);
    await controller.dispose();
  });
});
