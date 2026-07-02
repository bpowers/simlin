/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Verifies ProjectController.attachConnectorErrors wiring: after a rebuild, the
// active model's variables carry connectorErrors derived from the engine's
// getIncomingLinks and the sketch connectors, and engine failures degrade
// gracefully.

import type { LinkViewElement, StockFlowView, Variable } from '@simlin/core/datamodel';
import { defined } from '@simlin/core/common';
import type { ErrorDetail } from '@simlin/engine';
import { SimlinErrorKind } from '@simlin/engine';

import { ProjectController } from '../project-controller';
import { makeFakeEngine, makeControllerConfig } from './fake-engine';

// Drain microtasks + macrotasks so fire-and-forget navigation refreshes settle.
async function flushTimers(): Promise<void> {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// A project with a main model that references child model 'child' via module
// 'm', and a child model with auxes ca (constant) and cb (= ca) laid out on its
// view WITHOUT a connector -- so cb has a missing-connector issue. Module uid=1;
// child aux uids ca=10, cb=11.
function moduleProjectJson(): string {
  return JSON.stringify({
    name: 'test',
    simSpecs: { startTime: 0, endTime: 10, dt: '1' },
    models: [
      {
        name: 'main',
        stocks: [],
        flows: [],
        auxiliaries: [],
        modules: [{ name: 'm', modelName: 'child' }],
        views: [{ elements: [{ type: 'module', uid: 1, name: 'm', x: 0, y: 0 }] }],
      },
      {
        name: 'child',
        stocks: [],
        flows: [],
        auxiliaries: [
          { name: 'ca', equation: '1' },
          { name: 'cb', equation: 'ca' },
        ],
        views: [
          {
            elements: [
              { type: 'aux', uid: 10, name: 'ca', x: 0, y: 0 },
              { type: 'aux', uid: 11, name: 'cb', x: 100, y: 0 },
            ],
          },
        ],
      },
    ],
  });
}

function childVar(controller: ProjectController, ident: string): Variable | undefined {
  return controller.getSnapshot().project?.models.get('child')?.variables.get(ident);
}

const rect = { x: 0, y: 0, width: 1, height: 1 };

// A project with two auxes (a, b) and a view holding both plus optionally a
// connector a -> b. Auxiliary uids: a=1, b=2; link uid=3.
function projectJson(withConnector: boolean): string {
  const elements: Array<Record<string, unknown>> = [
    { type: 'aux', uid: 1, name: 'a', x: 0, y: 0 },
    { type: 'aux', uid: 2, name: 'b', x: 100, y: 0 },
  ];
  if (withConnector) {
    elements.push({ type: 'link', uid: 3, fromUid: 1, toUid: 2 });
  }
  return JSON.stringify({
    name: 'test',
    simSpecs: { startTime: 0, endTime: 10, dt: '1' },
    models: [
      {
        name: 'main',
        stocks: [],
        flows: [],
        auxiliaries: [
          { name: 'a', equation: '1' },
          { name: 'b', equation: 'a' },
        ],
        views: [{ elements }],
      },
    ],
  });
}

function bConnectorErrors(controller: ProjectController) {
  const project = controller.getSnapshot().project;
  return project?.models.get('main')?.variables.get('b')?.connectorErrors;
}

describe('ProjectController connector-sync', () => {
  it('attaches a missing-connector error when an equation dep has no connector', async () => {
    const engine = makeFakeEngine({
      json: () => projectJson(false),
      incomingLinks: { b: ['a'], a: [] },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    expect(bConnectorErrors(controller)).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
    await controller.dispose();
  });

  it('attaches no connector error when the connector matches the dependency', async () => {
    const engine = makeFakeEngine({
      json: () => projectJson(true),
      incomingLinks: { b: ['a'], a: [] },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    expect(bConnectorErrors(controller)).toBeUndefined();
    await controller.dispose();
  });

  it('attaches a stale-connector error when a drawn connector is unused', async () => {
    const engine = makeFakeEngine({
      json: () => projectJson(true),
      // b's equation no longer references a, but the connector remains.
      incomingLinks: { b: [], a: [] },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    expect(bConnectorErrors(controller)).toEqual([{ kind: 'staleConnector', ident: 'a', name: 'a' }]);
    await controller.dispose();
  });

  it('degrades gracefully (no connector errors) when getModel throws', async () => {
    const engine = makeFakeEngine({
      json: () => projectJson(false),
      incomingLinks: { b: ['a'] },
      getModelThrows: true,
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    expect(bConnectorErrors(controller)).toBeUndefined();
    // The project still opened successfully despite the getModel failure.
    expect(controller.getSnapshot().project).toBeDefined();
    await controller.dispose();
  });

  it('drops only the variable whose getIncomingLinks throws', async () => {
    const engine = makeFakeEngine({
      json: () => projectJson(false),
      incomingLinks: (name: string) => {
        if (name === 'b') {
          throw new Error('transient');
        }
        return [];
      },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();

    // b's deps could not be fetched, so it is not checked -- no error attached.
    expect(bConnectorErrors(controller)).toBeUndefined();
    await controller.dispose();
  });

  it('computes warnings against the rendered live view, not the stale engine snapshot', async () => {
    // The fake engine always serializes the connector-less view, so after an
    // optimistic view update that ADDS the a -> b connector the live view is
    // newer than what the engine returns. attachConnectorErrors must run on the
    // preserved live view, so b's dependency on a is satisfied and NOT flagged.
    // Pre-fix (annotations computed inside updateVariableErrors on the engine
    // snapshot) this asserted the missing warning was present -> this fails.
    const engine = makeFakeEngine({
      json: () => projectJson(false),
      incomingLinks: { b: ['a'], a: [] },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();
    // Sanity: with no connector drawn yet, b is flagged missing.
    expect(bConnectorErrors(controller)).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);

    const view = defined(controller.getView());
    const connector: LinkViewElement = {
      type: 'link',
      uid: view.nextUid,
      fromUid: 1,
      toUid: 2,
      arc: undefined,
      isStraight: true,
      multiPoint: undefined,
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };
    const liveView: StockFlowView = {
      ...view,
      elements: [...view.elements, connector],
      nextUid: view.nextUid + 1,
    };
    await controller.updateView(liveView, { recordHistory: true });

    expect(bConnectorErrors(controller)).toBeUndefined();
    await controller.dispose();
  });
});

describe('ProjectController connector-sync on module drill-in', () => {
  it('annotates the newly-active child model on drill-in (missing connector)', async () => {
    // Drill-in switches modelName without a rebuild, so before the fix the child
    // model's variables never received connectorErrors on first navigation.
    const engine = makeFakeEngine({
      json: () => moduleProjectJson(),
      incomingLinks: { cb: ['ca'], ca: [] },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();
    // Not drilled in yet: the child model carries no annotations.
    expect(childVar(controller, 'cb')?.connectorErrors).toBeUndefined();

    controller.drillIntoModule('m', 'child', new Set(), rect, 1);
    await flushTimers();

    expect(childVar(controller, 'cb')?.connectorErrors).toEqual([
      { kind: 'missingConnector', ident: 'ca', name: 'ca' },
    ]);
    await controller.dispose();
  });

  it('also annotates equation-error dots on the child model on drill-in (deeper gap)', async () => {
    // The same drill-in gap affected equation/unit error dots, not just connector
    // warnings: updateVariableErrors is model-scoped and only ran on rebuild
    // paths, so the child model's error dots were missing on first navigation
    // even though the error PANEL re-scoped.
    const engine = makeFakeEngine({
      json: () => moduleProjectJson(),
      incomingLinks: { cb: [], ca: [] },
      errors: [
        {
          modelName: 'child',
          variableName: 'cb',
          kind: SimlinErrorKind.Variable,
          code: 1,
          startOffset: 0,
          endOffset: 1,
        } as unknown as ErrorDetail,
      ],
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();
    expect(childVar(controller, 'cb')?.errors).toBeUndefined();

    controller.drillIntoModule('m', 'child', new Set(), rect, 1);
    await flushTimers();

    expect(childVar(controller, 'cb')?.errors).toEqual([{ start: 0, end: 1, code: 1 }]);
    await controller.dispose();
  });

  it('does not clobber a superseding navigation (mid-flight guard)', async () => {
    const engine = makeFakeEngine({
      json: () => moduleProjectJson(),
      incomingLinks: { cb: ['ca'], ca: [] },
    });
    const { config } = makeControllerConfig({ engine });
    const controller = new ProjectController(config);

    await controller.openInitialProject();
    controller.drillIntoModule('m', 'child', new Set(), rect, 1);
    await flushTimers();
    expect(childVar(controller, 'cb')?.connectorErrors).toEqual([
      { kind: 'missingConnector', ident: 'ca', name: 'ca' },
    ]);

    // Start a fresh annotation pass (captures modelName='child'); it suspends at
    // its first engine await. navigateBack then runs synchronously, flipping
    // modelName to 'main' and rebuilding (which resets the child's annotations).
    // When the stale pass resumes its guard sees modelName/project moved and must
    // NOT commit its child-scoped result over the post-navigation project.
    const pending = controller.refreshActiveModelAnnotations();
    controller.navigateBack();
    await flushTimers();
    expect(controller.getModelName()).toBe('main');

    await pending;
    await flushTimers();

    expect(childVar(controller, 'cb')?.connectorErrors).toBeUndefined();
    await controller.dispose();
  });
});
