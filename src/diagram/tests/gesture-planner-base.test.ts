// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// sameGeometry, the E5 test of whether a republished view invalidates a live
// gesture. Rows are derived from the element kinds of a scene holding one of
// each and, per kind, the read fields a gesture depends on: each must abort
// when it moves by more than GEOMETRY_EPSILON, and none of the derived or
// cosmetic fields may.
//
// The comparison is whole-view by design: a real change to any element aborts,
// including one the live gesture does not read. The benign republishes -- a
// pending edit landing (one ULP of drift, re-derived fields), sim results
// attaching, error annotations updating -- are rows of their own.
//
// What this does not establish: that an engine round trip drifts by no more
// than GEOMETRY_EPSILON (a measured fact of the design plan), or the Canvas
// acting on the answer (canvas-gestures-lifecycle.test.tsx).

import { describe, it, expect } from '@rstest/core';

import { isNamedViewElement, type StockFlowView, type ViewElement } from '@simlin/core/datamodel';

import { planGesture, sameGeometry } from '../gesture-planner';
import { aux, cloud, flow, link, linkedAuxes, planInput, scene, stock } from './support/gesture-fixtures';

function everyKind(): StockFlowView {
  return scene([
    stock(1, 'S', 100, 100),
    cloud(2, 3, 300, 100),
    flow(3, 'F', { x: 200, y: 100 }, [
      [122.5, 100, 1],
      [300, 100, 2],
    ]),
    aux(10, 'a', 100, 300),
    { type: 'module', uid: 20, name: 'm', x: 600, y: 600 } as never,
    { type: 'alias', uid: 15, aliasOfUid: 10, x: 100, y: 450 } as never,
    link(13, 10, 3, 20),
    { type: 'group', uid: 30, name: 'g', x: 500, y: 100, width: 100, height: 80 } as never,
  ]).view;
}

function edit(view: StockFlowView, uid: number, change: (el: ViewElement) => ViewElement): StockFlowView {
  return { ...view, elements: view.elements.map((el) => (el.uid === uid ? change(el) : el)) };
}

const BIG = 0.5;
const TINY = 1e-9;

// Every read field by element kind; the kinds are the scene's own.
const READ_FIELDS: Record<string, ReadonlyArray<[string, (el: never, by: number) => ViewElement]>> = {
  stock: [
    ['x', (el: ViewElement, by) => ({ ...el, x: el.x + by }) as ViewElement],
    ['labelSide', (el: ViewElement) => ({ ...el, labelSide: 'top' }) as ViewElement],
  ],
  cloud: [
    ['y', (el: ViewElement, by) => ({ ...el, y: el.y + by }) as ViewElement],
    ['flowUid', (el: ViewElement) => ({ ...el, flowUid: 99 }) as ViewElement],
  ],
  flow: [
    ['valve x', (el: ViewElement, by) => ({ ...el, x: el.x + by }) as ViewElement],
    [
      'point y',
      (el: never, by) =>
        ({
          ...(el as { points: { y: number }[] }),
          points: (el as { points: { y: number }[] }).points.map((p, i) => (i === 1 ? { ...p, y: p.y + by } : p)),
        }) as never,
    ],
    [
      'attachment',
      (el: never) =>
        ({
          ...(el as object),
          points: (el as { points: object[] }).points.map((p, i) => (i === 1 ? { ...p, attachedToUid: 1 } : p)),
        }) as never,
    ],
    [
      'point count',
      (el: never) =>
        ({ ...(el as object), points: [...(el as { points: object[] }).points, { x: 400, y: 100 }] }) as never,
    ],
    ['labelSide', (el: ViewElement) => ({ ...el, labelSide: 'left' }) as ViewElement],
  ],
  aux: [
    ['y', (el: ViewElement, by) => ({ ...el, y: el.y + by }) as ViewElement],
    ['labelSide', (el: ViewElement) => ({ ...el, labelSide: 'left' }) as ViewElement],
  ],
  module: [
    ['x', (el: ViewElement, by) => ({ ...el, x: el.x + by }) as ViewElement],
    ['labelSide', (el: ViewElement) => ({ ...el, labelSide: 'top' }) as ViewElement],
  ],
  alias: [
    ['x', (el: ViewElement, by) => ({ ...el, x: el.x + by }) as ViewElement],
    ['aliasOfUid', (el: ViewElement) => ({ ...el, aliasOfUid: 1 }) as ViewElement],
    ['labelSide', (el: ViewElement) => ({ ...el, labelSide: 'top' }) as ViewElement],
  ],
  link: [
    ['arc', (el: ViewElement, by) => ({ ...el, arc: (el as { arc: number }).arc + by }) as ViewElement],
    ['arc to straight', (el: ViewElement) => ({ ...el, arc: undefined }) as ViewElement],
    ['toUid', (el: ViewElement) => ({ ...el, toUid: 1 }) as ViewElement],
    ['fromUid', (el: ViewElement) => ({ ...el, fromUid: 1 }) as ViewElement],
  ],
  group: [
    ['width', (el: ViewElement, by) => ({ ...el, width: (el as { width: number }).width + by }) as ViewElement],
    ['y', (el: ViewElement, by) => ({ ...el, y: el.y + by }) as ViewElement],
  ],
};

describe('sameGeometry', () => {
  const base = everyKind();

  it('the scene holds every element kind the table lists', () => {
    expect([...new Set(base.elements.map((el) => el.type))].sort()).toEqual(Object.keys(READ_FIELDS).sort());
  });

  for (const el of base.elements) {
    for (const [field, change] of READ_FIELDS[el.type]) {
      it(`${el.type} ${field}: a change aborts, float noise does not`, () => {
        expect(
          sameGeometry(
            base,
            edit(base, el.uid, (e) => change(e as never, BIG)),
          ),
        ).toBe(false);
        if (change.length === 2 && field !== 'labelSide') {
          expect(
            sameGeometry(
              base,
              edit(base, el.uid, (e) => change(e as never, TINY)),
            ),
          ).toBe(true);
        }
      });
    }
  }

  it('derived and cosmetic fields do not abort: isStraight, var, ident, name, nextUid, viewport', () => {
    let next = edit(base, 13, (e) => ({ ...e, isStraight: true }) as ViewElement);
    next = edit(
      next,
      1,
      (e) => ({ ...e, var: { type: 'stock' } as never, name: 'Renamed', ident: 'renamed' }) as ViewElement,
    );
    next = { ...next, nextUid: 999, zoom: 3, viewBox: { x: 50, y: 50, width: 10, height: 10 } };
    expect(sameGeometry(base, next)).toBe(true);
  });

  it('elements added, removed or reordered: added and removed abort, reordering does not', () => {
    expect(sameGeometry(base, { ...base, elements: base.elements.slice(1) })).toBe(false);
    expect(sameGeometry(base, { ...base, elements: [...base.elements, { ...base.elements[0], uid: 77 }] })).toBe(false);
    expect(sameGeometry(base, { ...base, elements: [...base.elements].reverse() })).toBe(true);
  });

  it('a duplicated uid replacing another element aborts', () => {
    const dup = [...base.elements.slice(0, -1), { ...base.elements[0] }];
    expect(sameGeometry(base, { ...base, elements: dup })).toBe(false);
  });
});

describe('sameGeometry: benign republishes keep a live gesture', () => {
  const base = everyKind();
  // One ULP: the drift an engine round trip can leave on a coordinate.
  const ulp = (v: unknown): unknown =>
    typeof v === 'number' ? v + Math.max(Number.MIN_VALUE, Math.abs(v) * Number.EPSILON) : v;
  const annotateVars = (view: StockFlowView, fields: object): StockFlowView => ({
    ...view,
    elements: view.elements.map((el) =>
      isNamedViewElement(el) ? ({ ...el, var: { ...(el.var ?? {}), ...fields } } as ViewElement) : el,
    ),
  });

  it('a pending edit landing: every coordinate one ULP away, isStraight, var and nextUid re-derived', () => {
    const elements = base.elements.map((el) => {
      const next: Record<string, unknown> = { ...el, x: ulp(el.x), y: ulp(el.y) };
      if (el.type === 'flow') {
        next.points = el.points.map((p) => ({ ...p, x: ulp(p.x), y: ulp(p.y) }));
      } else if (el.type === 'link') {
        next.arc = ulp(el.arc);
        next.isStraight = !el.isStraight;
      } else if (el.type === 'group') {
        next.width = ulp(el.width);
        next.height = ulp(el.height);
      }
      if (isNamedViewElement(el)) {
        next.var = el.var === undefined ? undefined : { ...el.var };
      }
      return next as unknown as ViewElement;
    });
    const landed = { ...base, elements, nextUid: Math.max(...base.elements.map((el) => el.uid)) + 1 };
    expect(landed.elements.filter((el, i) => el.x !== base.elements[i].x).length).toBeGreaterThan(0);
    expect(sameGeometry(base, landed)).toBe(true);
  });

  it('sim results attaching to the variables', () => {
    const series = [{ name: 'S', time: new Float64Array([0, 1]), values: new Float64Array([1, 2]) }];
    expect(sameGeometry(base, annotateVars(base, { data: series }))).toBe(true);
  });

  it('error annotations updating', () => {
    const errors = [{ start: 0, end: 1, code: 'unknown_dependency' }];
    expect(sameGeometry(base, annotateVars(base, { errors, unitErrors: errors }))).toBe(true);
  });

  it('M-1: a link the planner created, landing as the datamodel reads it (x/y NaN, isStraight re-derived)', () => {
    const s = linkedAuxes();
    const p = planGesture(planInput(s, { kind: 'createLink', from: 12 }, { x: 300, y: 450 }, { x: 100, y: 300 }));
    expect(p.commit).toBe('edit');
    const pending = { ...s.view, elements: p.elements, nextUid: p.nextUid };
    const landed = {
      ...pending,
      elements: pending.elements.map((el) =>
        el.type === 'link' && el.uid === p.nextUid - 1
          ? ({ ...el, x: NaN, y: NaN, isStraight: true } as ViewElement)
          : el,
      ),
    };
    expect(sameGeometry(pending, landed)).toBe(true);
    // Whatever a producer stores as a link's position, nothing reads it.
    const zeroed = edit(landed, p.nextUid - 1, (e) => ({ ...e, x: 0, y: 0 }) as ViewElement);
    expect(sameGeometry(landed, zeroed)).toBe(true);
  });

  it('a real change to any element aborts, including one a gesture on another element never reads', () => {
    expect(
      sameGeometry(
        base,
        edit(base, 20, (e) => ({ ...e, x: e.x + 5 }) as ViewElement),
      ),
    ).toBe(false);
  });
});
