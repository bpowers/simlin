// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Connector drift on the rendered project: the controller fetches each
// target's equation dependencies from the engine (`getIncomingLinks`) in a
// maintenance item and computes `connectorErrors` at render time against the
// RENDERED view, so a connector drawn by a pending edit counts immediately.
// Engine failures degrade to no annotations.

import { describe, it, expect } from '@rstest/core';

import type { LinkViewElement, StockFlowView, Variable } from '@simlin/core/datamodel';
import { ErrorCode } from '@simlin/core/datamodel';
import type { ErrorDetail } from '@simlin/engine';
import { SimlinErrorKind } from '@simlin/engine';

import { ProjectController } from '../project-controller';
import { makeFakeEngine, makeControllerConfig, makeGate, type FakeEngineOptions } from './fake-engine';

// A main model referencing child model 'child' via module 'm'; the child has
// auxes ca (constant) and cb (= ca) WITHOUT a connector, so cb misses one.
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

// Two auxes (a, b = a) and, optionally, a connector a -> b. uids a=1, b=2, link=3.
function projectJson(
  withConnector: boolean,
  auxiliaries = [
    { name: 'a', equation: '1' },
    { name: 'b', equation: 'a' },
  ],
): string {
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
    models: [{ name: 'main', stocks: [], flows: [], auxiliaries, views: [{ elements }] }],
  });
}

async function open(json: string, options: FakeEngineOptions): Promise<ProjectController> {
  const engine = makeFakeEngine({ json: () => json, ...options });
  const { config } = makeControllerConfig({ engine, format: 'json' });
  const controller = new ProjectController(config);
  await controller.openInitialProject();
  await controller.whenIdle();
  return controller;
}

function variable(controller: ProjectController, ident: string, modelName = 'main'): Variable | undefined {
  return controller.getSnapshot().project?.models.get(modelName)?.variables.get(ident);
}

const rect = { x: 0, y: 0, width: 1, height: 1 };

describe('ProjectController connector drift', () => {
  it('flags a missing connector when an equation dependency has none drawn', async () => {
    const controller = await open(projectJson(false), { incomingLinks: { b: ['a'], a: [] } });
    expect(variable(controller, 'b')?.connectorErrors).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
    await controller.dispose();
  });

  it('flags nothing when the connector matches the dependency', async () => {
    const controller = await open(projectJson(true), { incomingLinks: { b: ['a'], a: [] } });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    await controller.dispose();
  });

  it('flags a stale connector the equation does not use', async () => {
    const controller = await open(projectJson(true), { incomingLinks: { b: [], a: [] } });
    expect(variable(controller, 'b')?.connectorErrors).toEqual([{ kind: 'staleConnector', ident: 'a', name: 'a' }]);
    await controller.dispose();
  });

  it('degrades to no annotations when getModel throws', async () => {
    const controller = await open(projectJson(false), { incomingLinks: { b: ['a'] }, getModelThrows: true });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    expect(controller.getSnapshot().project).toBeDefined();
    await controller.dispose();
  });

  it('drops only the variable whose getIncomingLinks throws', async () => {
    const controller = await open(projectJson(false), {
      incomingLinks: (name: string) => {
        if (name === 'b') {
          throw new Error('transient');
        }
        return [];
      },
    });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    await controller.dispose();
  });

  it('a connector drawn by a pending edit satisfies the dependency at once (the rendered view, not committed)', async () => {
    const gate = makeGate();
    const controller = await open(projectJson(false), {
      incomingLinks: { b: ['a'], a: [] },
      applyPatchGate: () => gate.wait(),
    });
    expect(variable(controller, 'b')?.connectorErrors).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
    const view = controller.getView() as StockFlowView;
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
    void controller.enqueueViewEdit({
      label: 'link',
      nextView: { ...view, elements: [...view.elements, connector], nextUid: view.nextUid + 1 },
    });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    gate.open();
    await controller.whenIdle();
    await controller.dispose();
  });
});

describe('ProjectController connector drift on module drill-in', () => {
  it('annotates the newly active child model once its dependencies are fetched', async () => {
    const controller = await open(moduleProjectJson(), { incomingLinks: { cb: ['ca'], ca: [] } });
    expect(variable(controller, 'cb', 'child')?.connectorErrors).toBeUndefined();
    controller.drillIntoModule('m', 'child', new Set(), rect, 1);
    await controller.whenIdle();
    expect(variable(controller, 'cb', 'child')?.connectorErrors).toEqual([
      { kind: 'missingConnector', ident: 'ca', name: 'ca' },
    ]);
    await controller.dispose();
  });

  it('annotates equation-error dots on the child model on drill-in, synchronously', async () => {
    const controller = await open(moduleProjectJson(), {
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
    expect(variable(controller, 'cb', 'child')?.errors).toBeUndefined();
    controller.drillIntoModule('m', 'child', new Set(), rect, 1);
    expect(variable(controller, 'cb', 'child')?.errors).toEqual([{ start: 0, end: 1, code: 1 }]);
    await controller.dispose();
  });

  it("only the active model carries connector annotations: navigating back clears the child's", async () => {
    const controller = await open(moduleProjectJson(), { incomingLinks: { cb: ['ca'], ca: [] } });
    controller.drillIntoModule('m', 'child', new Set(), rect, 1);
    await controller.whenIdle();
    expect(variable(controller, 'cb', 'child')?.connectorErrors).toBeDefined();
    controller.navigateBack();
    await controller.whenIdle();
    expect(controller.getModelName()).toBe('main');
    expect(variable(controller, 'cb', 'child')?.connectorErrors).toBeUndefined();
    await controller.dispose();
  });
});

describe('ProjectController connector drift skips errored-equation targets', () => {
  const eqnError = (variableName: string): ErrorDetail =>
    ({
      modelName: 'main',
      variableName,
      kind: SimlinErrorKind.Variable,
      code: 1,
      startOffset: 0,
      endOffset: 1,
    }) as unknown as ErrorDetail;
  const unitError = (variableName: string): ErrorDetail =>
    ({
      modelName: 'main',
      variableName,
      kind: SimlinErrorKind.Units,
      code: 44,
      startOffset: 0,
      endOffset: 1,
    }) as unknown as ErrorDetail;

  it('does not flag a stale connector on a variable with a fatal equation error', async () => {
    const controller = await open(projectJson(true), { incomingLinks: { a: [], b: [] }, errors: [eqnError('b')] });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    expect(variable(controller, 'b')?.errors).toEqual([{ start: 0, end: 1, code: 1 }]);
    await controller.dispose();
  });

  it('does not flag a missing connector on a variable with a fatal equation error', async () => {
    const controller = await open(projectJson(false), { incomingLinks: { a: [], b: ['a'] }, errors: [eqnError('b')] });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    await controller.dispose();
  });

  it('still checks a healthy sibling variable in the same view', async () => {
    const json = JSON.stringify({
      name: 'test',
      simSpecs: { startTime: 0, endTime: 10, dt: '1' },
      models: [
        {
          name: 'main',
          stocks: [],
          flows: [],
          auxiliaries: [
            { name: 'a', equation: '1' },
            { name: 'b', equation: '@' },
            { name: 'c', equation: 'a' },
          ],
          views: [
            {
              elements: [
                { type: 'aux', uid: 1, name: 'a', x: 0, y: 0 },
                { type: 'aux', uid: 2, name: 'b', x: 100, y: 0 },
                { type: 'aux', uid: 3, name: 'c', x: 200, y: 0 },
                { type: 'link', uid: 4, fromUid: 1, toUid: 2 },
              ],
            },
          ],
        },
      ],
    });
    const controller = await open(json, { incomingLinks: { a: [], b: [], c: ['a'] }, errors: [eqnError('b')] });
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    expect(variable(controller, 'c')?.connectorErrors).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
    await controller.dispose();
  });

  it('suppresses all connector warnings in an all-empty starter model (hasNoEquations)', async () => {
    const emptyErr = (variableName: string): ErrorDetail =>
      ({
        modelName: 'main',
        variableName,
        kind: SimlinErrorKind.Variable,
        code: ErrorCode.EmptyEquation,
        startOffset: 0,
        endOffset: 0,
      }) as unknown as ErrorDetail;
    const controller = await open(projectJson(true, [{ name: 'a' }, { name: 'b' }] as never), {
      incomingLinks: { a: [], b: [] },
      errors: [emptyErr('a'), emptyErr('b')],
    });
    expect(controller.getSnapshot().project?.hasNoEquations).toBe(true);
    expect(variable(controller, 'a')?.connectorErrors).toBeUndefined();
    expect(variable(controller, 'b')?.connectorErrors).toBeUndefined();
    await controller.dispose();
  });

  it('still checks a variable that has only unit errors (its AST is valid)', async () => {
    const controller = await open(projectJson(true), { incomingLinks: { a: [], b: [] }, errors: [unitError('b')] });
    expect(variable(controller, 'b')?.connectorErrors).toEqual([{ kind: 'staleConnector', ident: 'a', name: 'a' }]);
    await controller.dispose();
  });
});
