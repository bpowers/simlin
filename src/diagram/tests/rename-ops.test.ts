// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A rename's next view (relabelVariable) and the renameVariable op the
// controller derives from it (buildEditOps). The committed model is loaded
// through the production loader.

import { describe, it, expect } from '@rstest/core';

import { projectFromJson, type AuxViewElement, type Model } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

import { relabelVariable } from '../rename-ops';
import { buildEditOps } from '../view-model-sync';

function committed(auxName: string): Model {
  const json = {
    name: 'rename',
    simSpecs: { startTime: 0, endTime: 1, dt: '1' },
    models: [
      {
        name: 'main',
        stocks: [],
        flows: [],
        auxiliaries: [
          { name: auxName, equation: '1' },
          { name: 'Other Var', equation: '2' },
        ],
        views: [
          {
            elements: [
              { type: 'aux', uid: 1, name: auxName, x: 10, y: 20 },
              { type: 'aux', uid: 2, name: 'Other Var', x: 50, y: 20 },
              { type: 'link', uid: 3, fromUid: 1, toUid: 2 },
            ],
          },
        ],
      },
    ],
  } as unknown as JsonProject;
  return projectFromJson(json).models.get('main')!;
}

function renameOps(auxName: string, oldName: string, newName: string) {
  const model = committed(auxName);
  const base = model.views[0];
  return buildEditOps(model, base, relabelVariable(base, oldName, newName));
}

describe('relabelVariable + buildEditOps', () => {
  it('sends the typed name RAW as the rename `to` and the canonical ident as `from`', () => {
    // The engine preserves display spellings verbatim and matches canonically
    // (issue #890); canonicalizing `to` downgraded the stored spelling
    // ("New Students" -> `new_students`) on every rename (issue #906).
    expect(renameOps('Total Students', 'Total Students', 'New Students')[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'total_students', to: 'New Students' },
    });
  });

  it('preserves the display spelling on a case-only rename', () => {
    expect(renameOps('students', 'students', 'Students')[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'students', to: 'Students' },
    });
  });

  it('encodes line breaks in the new name (stored backslash-n form)', () => {
    const model = committed('Total Students');
    const next = relabelVariable(model.views[0], 'Total Students', 'testing\nassymptomatic');
    expect((next.elements[0] as AuxViewElement).name).toBe('testing\\nassymptomatic');
    expect(buildEditOps(model, model.views[0], next)[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'total_students', to: 'testing\\nassymptomatic' },
    });
  });

  it("relabels only the matching named element, gives it the new name's ident, and passes the rest through by reference", () => {
    const model = committed('Total Students');
    const base = model.views[0];
    const next = relabelVariable(base, 'Total Students', 'Enrolled Students');
    expect(next.elements[0]).toMatchObject({ name: 'Enrolled Students', ident: 'enrolled_students' });
    expect(next.elements[1]).toBe(base.elements[1]);
    expect(next.elements[2]).toBe(base.elements[2]);
    const ops = buildEditOps(model, base, next);
    expect(ops.map((op) => op.type)).toEqual(['renameVariable', 'upsertView']);
  });

  it('matches the element by its name, so a second rename of a pending rename (or of a pending create) finds it', () => {
    const model = committed('Total Students');
    const base = model.views[0];
    const once = relabelVariable(base, 'Total Students', 'Enrolled')!;
    // The Canvas commits the second rename with the name the element renders.
    const twice = relabelVariable(once, 'Enrolled', 'Graduated');
    expect(twice.elements[0]).toMatchObject({ name: 'Graduated', ident: 'graduated' });
    // Planned on the first rename's view, against the model that rename produced.
    const renamedModel = committed('Enrolled');
    expect(buildEditOps(renamedModel, once, twice)[0]).toEqual({
      type: 'renameVariable',
      payload: { from: 'enrolled', to: 'Graduated' },
    });
    // A staged element whose ident does not follow its name (a create staged
    // under a default name) is still found by name.
    const staged = { ...base, elements: [{ ...base.elements[0], ident: 'new_variable' }, ...base.elements.slice(1)] };
    expect(relabelVariable(staged, 'Total Students', 'Births').elements[0]).toMatchObject({
      name: 'Births',
      ident: 'births',
    });
  });
});
