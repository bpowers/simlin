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

import type { LinkViewElement, StockFlowView } from '@simlin/core/datamodel';
import { defined } from '@simlin/core/common';

import { ProjectController } from '../project-controller';
import { makeFakeEngine, makeControllerConfig } from './fake-engine';

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
