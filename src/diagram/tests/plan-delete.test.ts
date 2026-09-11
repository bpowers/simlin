// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// planDelete, one row per element kind a selection can hold (stock, flow, aux,
// module, link, alias, cloud, group), plus the cascades: clouds of removed
// flows, aliases of removed elements, links touching anything removed, and
// endpoints of surviving flows on removed stocks. The view is loaded through
// the production loader (projectFromJson), so element shapes are production's.

import { describe, it, expect } from '@rstest/core';

import { projectFromJson, type StockFlowView, type UID, type ViewElement } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

import { planDelete } from '../plan-delete';
import { checkReferentialIntegrity, formatViewViolations } from './support/view-invariants';

function view(): StockFlowView {
  const json = {
    name: 'plan-delete',
    simSpecs: { startTime: 0, endTime: 1, dt: '1' },
    models: [
      {
        name: 'main',
        stocks: [
          { name: 'A', initialEquation: '1', inflows: [], outflows: ['f'] },
          { name: 'B', initialEquation: '1', inflows: ['f', 'loop'], outflows: ['loop'] },
        ],
        flows: [
          { name: 'f', equation: '1' },
          { name: 'loop', equation: '1' },
          { name: 'k', equation: '1' },
        ],
        auxiliaries: [{ name: 'x', equation: '1' }],
        modules: [{ name: 'm', modelName: 'main' }],
        views: [
          {
            elements: [
              { type: 'stock', uid: 1, name: 'A', x: 100, y: 100 },
              { type: 'stock', uid: 2, name: 'B', x: 300, y: 100 },
              {
                type: 'flow',
                uid: 3,
                name: 'f',
                x: 200,
                y: 100,
                points: [
                  { x: 122.5, y: 100, attachedToUid: 1 },
                  { x: 277.5, y: 100, attachedToUid: 2 },
                ],
              },
              {
                type: 'flow',
                uid: 4,
                name: 'loop',
                x: 300,
                y: 50,
                points: [
                  { x: 300, y: 82.5, attachedToUid: 2 },
                  { x: 300, y: 40, attachedToUid: 2 },
                ],
              },
              {
                type: 'flow',
                uid: 5,
                name: 'k',
                x: 100,
                y: 300,
                points: [
                  { x: 0, y: 300, attachedToUid: 6 },
                  { x: 200, y: 300, attachedToUid: 7 },
                ],
              },
              { type: 'cloud', uid: 6, flowUid: 5, x: 0, y: 300 },
              { type: 'cloud', uid: 7, flowUid: 5, x: 200, y: 300 },
              { type: 'aux', uid: 8, name: 'x', x: 200, y: 20 },
              { type: 'module', uid: 9, name: 'm', x: 500, y: 20 },
              { type: 'link', uid: 10, fromUid: 1, toUid: 8 },
              { type: 'alias', uid: 11, aliasOfUid: 1, x: 100, y: 250 },
              { type: 'link', uid: 12, fromUid: 11, toUid: 5 },
              { type: 'link', uid: 13, fromUid: 8, toUid: 9 },
              { type: 'group', uid: 14, name: 'g', x: 0, y: 0, width: 50, height: 50 },
            ],
          },
        ],
      },
    ],
  } as unknown as JsonProject;
  return projectFromJson(json).models.get('main')!.views[0];
}

function uids(v: StockFlowView): UID[] {
  return v.elements.map((el) => el.uid).sort((a, b) => a - b);
}

function byUid(v: StockFlowView, uid: UID): ViewElement | undefined {
  return v.elements.find((el) => el.uid === uid);
}

function endpoints(v: StockFlowView, flowUid: UID): Array<number | undefined> {
  const flow = byUid(v, flowUid);
  if (flow?.type !== 'flow') {
    throw new Error(`no flow ${flowUid}`);
  }
  return [flow.points[0].attachedToUid, flow.points[flow.points.length - 1].attachedToUid];
}

describe('planDelete', () => {
  it('stock: removes it, its alias, links touching either, and clouds the endpoints on it', () => {
    const next = planDelete(view(), new Set([1]));
    // 1 stock, 10 link from it, 11 its alias, 12 link from the alias.
    expect(uids(next)).toEqual([2, 3, 4, 5, 6, 7, 8, 9, 13, 14, 15]);
    // f's source was on A: a new cloud at the endpoint, owned by f.
    expect(endpoints(next, 3)).toEqual([15, 2]);
    expect(byUid(next, 15)).toMatchObject({ type: 'cloud', flowUid: 3, x: 122.5, y: 100 });
    expect(next.nextUid).toBe(16);
    expect(formatViewViolations(checkReferentialIntegrity(next))).toBe('');
  });

  it('stock a flow both starts and ends on: two clouds, one per endpoint', () => {
    const next = planDelete(view(), new Set([2]));
    expect(endpoints(next, 4)).toEqual([16, 17]);
    expect(endpoints(next, 3)).toEqual([1, 15]);
    expect(formatViewViolations(checkReferentialIntegrity(next))).toBe('');
  });

  it('flow: removes it and its clouds, and the links touching it', () => {
    const next = planDelete(view(), new Set([5]));
    expect(uids(next)).toEqual([1, 2, 3, 4, 8, 9, 10, 11, 13, 14]);
    expect(next.nextUid).toBe(15);
    expect(formatViewViolations(checkReferentialIntegrity(next))).toBe('');
  });

  it('aux and module: removes them and the links touching them', () => {
    expect(uids(planDelete(view(), new Set([8])))).toEqual([1, 2, 3, 4, 5, 6, 7, 9, 11, 12, 14]);
    expect(uids(planDelete(view(), new Set([9])))).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 14]);
  });

  it('link: removes only the link', () => {
    expect(uids(planDelete(view(), new Set([10])))).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14]);
  });

  it('alias: removes the alias and links touching it, never the aliased element', () => {
    expect(uids(planDelete(view(), new Set([11])))).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13, 14]);
  });

  it('cloud whose flow survives: ignored, nothing changes', () => {
    const before = view();
    const next = planDelete(before, new Set([6]));
    expect(uids(next)).toEqual(uids(before));
    expect(next.nextUid).toBe(before.nextUid);
  });

  it('cloud together with its flow: removed with the flow', () => {
    expect(uids(planDelete(view(), new Set([5, 6])))).toEqual([1, 2, 3, 4, 8, 9, 10, 11, 13, 14]);
  });

  it('group: removes only the group', () => {
    expect(uids(planDelete(view(), new Set([14])))).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
  });

  it('a selected uid the view does not contain is ignored', () => {
    const before = view();
    expect(uids(planDelete(before, new Set([999])))).toEqual(uids(before));
  });

  it('empty selection returns an equal view', () => {
    const before = view();
    expect(planDelete(before, new Set())).toEqual(before);
  });
});
