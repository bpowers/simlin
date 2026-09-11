// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The details-panel remount key. The variable/module details panels seed their
// Slate editors once per mount, so a React remount (key change) is what
// refreshes them. The Editor keys a panel on the SELECTED variable's committed,
// user-editable content (plus the read-only flag, pinned in
// editor-readonly-gating.test.ts), so:
//
//  - a landed edit to that variable's equation/units/docs/table remounts it;
//  - a pan, a save acknowledgment or a sim run (render-key-only changes), an
//    edit to ANOTHER variable, and a change to this variable's errors do not --
//    each of those can land while the user types, and a remount would discard
//    the draft.
//
// The rows are the variable fields that can change between snapshots: the
// editable content (equation, units, documentation, gf) and the non-content
// annotations (errors, unitErrors, connectorErrors, data), plus another
// variable's content and the render key. A stub VariableDetails records each
// mount; snapshots are published through the controller subscription, as the
// real controller does.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, render } from '@testing-library/react';

import { projectFromJson, type JsonProject, type Project, type Variable } from '@simlin/core/datamodel';
import { mapSet } from '@simlin/core/common';
import { ProjectController, type ProjectSnapshot } from '../project-controller';

const projectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: [
        { name: 'x', equation: '1' },
        { name: 'y', equation: '2' },
      ],
      views: [
        {
          elements: [
            { type: 'aux', uid: 1, name: 'x', x: 0, y: 0 },
            { type: 'aux', uid: 2, name: 'y', x: 50, y: 0 },
          ],
        },
      ],
    },
  ],
});

let variableDetailsMounts = 0;
rs.mock('../VariableDetails', () => ({
  __esModule: true,
  VariableDetails: () => {
    React.useEffect(() => {
      variableDetailsMounts += 1;
    }, []);
    return null;
  },
}));

interface CapturedCanvasProps {
  onSetSelection: (sel: ReadonlySet<number>) => void;
  onShowVariableDetails: () => void;
}
let capturedCanvasProps: CapturedCanvasProps | undefined;
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CapturedCanvasProps) => {
    capturedCanvasProps = p;
    return null;
  },
  inCreationUid: -2,
}));

import { Editor, detailsPanelKey, type EditorProps } from '../Editor';

function baseProject(): Project {
  return projectFromJson(JSON.parse(projectJson) as JsonProject);
}

function withVariable(project: Project, ident: string, patch: Partial<Variable>): Project {
  const model = project.models.get('main')!;
  const variables = new Map(model.variables);
  variables.set(ident, { ...variables.get(ident)!, ...patch } as Variable);
  return { ...project, models: mapSet(project.models, 'main', { ...model, variables }) };
}

function makeSnapshot(project: Project, projectVersion: number, restoreSeq = 0, token = 0): ProjectSnapshot {
  return {
    project,
    projectVersion,
    serverVersion: 1,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelName: 'main',
    modelStack: [],
    canUndo: false,
    canRedo: false,
    undoRedoQueued: false,
    token,
    restoreSeq,
    navResetSeq: 0,
  } as unknown as ProjectSnapshot;
}

function makeProps(): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: projectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
  } as EditorProps;
}

const gf = {
  kind: 'continuous',
  xScale: { min: 0, max: 1 },
  yScale: { min: 0, max: 1 },
  xPoints: undefined,
  yPoints: [0, 1],
};

// Each row publishes one change to the selected variable x (or to y, or only to
// the render key) and states whether the open panel must remount.
const ROWS: ReadonlyArray<{ name: string; change: (p: Project) => Project; remounts: boolean }> = [
  {
    name: "x's equation",
    change: (p) => withVariable(p, 'x', { equation: { type: 'scalar', equation: '5' } }),
    remounts: true,
  },
  { name: "x's units", change: (p) => withVariable(p, 'x', { units: 'widgets' }), remounts: true },
  { name: "x's documentation", change: (p) => withVariable(p, 'x', { documentation: 'docs' }), remounts: true },
  { name: "x's lookup table", change: (p) => withVariable(p, 'x', { gf } as Partial<Variable>), remounts: true },
  {
    name: "x's equation errors",
    change: (p) => withVariable(p, 'x', { errors: [{ start: 0, end: 1, code: 1 }] } as Partial<Variable>),
    remounts: false,
  },
  {
    name: "x's unit errors",
    change: (p) =>
      withVariable(p, 'x', { unitErrors: [{ start: 0, end: 1, code: 1, kind: 'definition' }] } as Partial<Variable>),
    remounts: false,
  },
  {
    name: "x's connector drift",
    change: (p) =>
      withVariable(p, 'x', {
        connectorErrors: [{ kind: 'missingConnector', ident: 'y', name: 'y' }],
      } as Partial<Variable>),
    remounts: false,
  },
  {
    name: "x's sim series",
    change: (p) =>
      withVariable(p, 'x', { data: [{ name: 'x', time: new Float64Array([0]), values: new Float64Array([1]) }] }),
    remounts: false,
  },
  {
    name: "another variable's equation",
    change: (p) => withVariable(p, 'y', { equation: { type: 'scalar', equation: '9' } }),
    remounts: false,
  },
  { name: 'only the render key (a pan, a save ack)', change: (p) => p, remounts: false },
];

describe('Editor details-panel key', () => {
  let snapshot: ProjectSnapshot;
  let listener: (() => void) | undefined;

  beforeEach(() => {
    variableDetailsMounts = 0;
    capturedCanvasProps = undefined;
    listener = undefined;
    snapshot = makeSnapshot(baseProject(), 1);
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockImplementation(() => snapshot);
    rs.spyOn(ProjectController.prototype, 'subscribe').mockImplementation((l: () => void) => {
      listener = l;
      return () => {
        listener = undefined;
      };
    });
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function publish(next: ProjectSnapshot): void {
    snapshot = next;
    act(() => {
      listener?.();
    });
  }

  for (const row of ROWS) {
    it(`${row.remounts ? 'remounts' : 'does not remount'} the open panel when ${row.name} changes`, () => {
      act(() => {
        render(React.createElement(Editor, makeProps()));
      });
      act(() => {
        capturedCanvasProps!.onSetSelection(new Set([1]));
        capturedCanvasProps!.onShowVariableDetails();
      });
      expect(variableDetailsMounts).toBe(1);

      publish(makeSnapshot(row.change(baseProject()), 2));
      expect(variableDetailsMounts).toBe(row.remounts ? 2 : 1);
    });
  }

  // The key names the model for both panel kinds: a uid is unique only within
  // its model, so a module or variable with the same uid and content in another
  // model must get its own panel.
  it('keys a variable panel and a module panel by model name', () => {
    const variable = baseProject().models.get('main')!.variables.get('x')!;
    const module: Variable = {
      type: 'module',
      ident: 'm',
      modelName: 'child',
      documentation: '',
      units: '',
      references: [],
      canBeModuleInput: false,
      isPublic: false,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: undefined,
    };
    for (const v of [variable, module]) {
      expect(detailsPanelKey('main', 1, v, 0, false)).not.toBe(detailsPanelKey('child', 1, v, 0, false));
      expect(detailsPanelKey('main', 1, v, 0, false)).toBe(detailsPanelKey('main', 1, v, 0, false));
    }
  });

  it('remounts the open panel when another element is selected, even one whose variable has the same content', () => {
    // y gets x's content, so only the selected element tells the two panels apart.
    snapshot = makeSnapshot(withVariable(baseProject(), 'y', { equation: { type: 'scalar', equation: '1' } }), 1);
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([1]));
      capturedCanvasProps!.onShowVariableDetails();
    });
    expect(variableDetailsMounts).toBe(1);
    act(() => {
      capturedCanvasProps!.onSetSelection(new Set([2]));
      capturedCanvasProps!.onShowVariableDetails();
    });
    expect(variableDetailsMounts).toBe(2);
  });

  // The two counters that move without x's content changing: an undo/redo
  // landing (restoreSeq) remounts, since the restored content can equal the
  // content the panel was seeded from while the panel holds a draft whose edit
  // was undone; the token (which a failure elsewhere also moves) does not.
  for (const counter of ['restoreSeq', 'token'] as const) {
    it(`${counter === 'restoreSeq' ? 'remounts' : 'does not remount'} the open panel when only the ${counter} moves`, () => {
      act(() => {
        render(React.createElement(Editor, makeProps()));
      });
      act(() => {
        capturedCanvasProps!.onSetSelection(new Set([1]));
        capturedCanvasProps!.onShowVariableDetails();
      });
      expect(variableDetailsMounts).toBe(1);
      publish(counter === 'restoreSeq' ? makeSnapshot(baseProject(), 2, 1, 0) : makeSnapshot(baseProject(), 2, 0, 1));
      expect(variableDetailsMounts).toBe(counter === 'restoreSeq' ? 2 : 1);
    });
  }
});
