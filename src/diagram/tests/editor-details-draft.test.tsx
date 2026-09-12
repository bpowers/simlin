// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Details-panel drafts across the controller's edit queue, with the REAL
// VariableDetails (Canvas mocked, controller snapshot stubbed):
//
//  - a press outside the panel (a canvas press, which does not blur the panel
//    because the Canvas prevents the default focus change) flushes the draft
//    first: the panel's model edit is enqueued before the press is handled;
//  - the same draft submitted again while its edit is still pending (the blur
//    that follows) is not enqueued twice;
//  - a landed change to the selected variable's errors does not remount the
//    panel, so the draft text survives; a landed change to its content does.
//
// What this does not establish: that the flushed edit lands against the real
// engine before a following gesture's edit (editor-engine-races.test.ts).

import { describe, it, expect, beforeAll, beforeEach, afterEach, rs } from '@rstest/core';

beforeAll(() => {
  // jsdom lacks isContentEditable and Range geometry, which slate-react and the
  // equation preview read.
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
  if (!('getBoundingClientRect' in Range.prototype)) {
    const zero = () =>
      ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} }) as DOMRect;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Range.prototype as any).getBoundingClientRect = zero;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Range.prototype as any).getClientRects = () =>
      ({ length: 0, item: () => null, [Symbol.iterator]: function* () {} }) as unknown as DOMRectList;
  }
});

import * as React from 'react';
import { act, render, fireEvent, type RenderResult } from '@testing-library/react';
import { Editor as SlateEditor, Transforms } from 'slate';
import { ELEMENT_TO_NODE } from 'slate-dom';

import * as react from 'react' with { rstest: 'importActual' };

import { projectFromJson, type JsonProject, type Project, type Variable } from '@simlin/core/datamodel';
import { mapSet } from '@simlin/core/common';
import type { JsonProjectPatch } from '@simlin/engine';

import { ProjectController, type ProjectSnapshot } from '../project-controller';
import type { CanvasProps } from '../drawing/Canvas';

let canvasProps: CanvasProps | undefined;
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: CanvasProps) => {
    canvasProps = p;
    return react.createElement('div', { 'data-testid': 'canvas' });
  },
  inCreationUid: -2,
}));

import { Editor, type EditorProps } from '../Editor';

const projectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: [{ name: 'x', equation: '1', units: 'people' }],
      views: [{ elements: [{ type: 'aux', uid: 1, name: 'x', x: 0, y: 0 }] }],
    },
  ],
});

function baseProject(): Project {
  return projectFromJson(JSON.parse(projectJson) as JsonProject);
}

function withX(project: Project, patch: Partial<Variable>): Project {
  const model = project.models.get('main')!;
  const variables = new Map(model.variables);
  variables.set('x', { ...variables.get('x')!, ...patch } as Variable);
  return { ...project, models: mapSet(project.models, 'main', { ...model, variables }) };
}

function makeSnapshot(project: Project, projectVersion: number): ProjectSnapshot {
  return {
    project,
    projectVersion,
    serverVersion: 1,
    status: 'ok',
    cachedErrors: {
      simError: undefined,
      modelErrors: [],
      varErrors: new Map(),
      unitErrors: new Map(),
      varWarnings: new Map(),
    },
    data: new Map(),
    modelName: 'main',
    modelStack: [],
    canUndo: false,
    canRedo: false,
    undoRedoQueued: false,
    token: 0,
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

function unitsEditor(container: HTMLElement): SlateEditor {
  const el = container.querySelector('.unitsEditor');
  expect(el).not.toBeNull();
  return ELEMENT_TO_NODE.get(el as HTMLElement) as unknown as SlateEditor;
}

function unitsText(container: HTMLElement): string {
  return container.querySelector('.unitsEditor')?.textContent ?? '';
}

async function appendUnits(container: HTMLElement, text: string): Promise<void> {
  const editor = unitsEditor(container);
  await act(async () => {
    Transforms.insertText(editor, text, { at: SlateEditor.end(editor, []) });
    editor.onChange();
    await Promise.resolve();
  });
}

describe('Editor details-panel drafts', () => {
  let snapshot: ProjectSnapshot;
  let listener: (() => void) | undefined;
  let modelEdits: Array<{ label: string; buildPatch: (committed: Project) => JsonProjectPatch }>;

  beforeEach(() => {
    canvasProps = undefined;
    listener = undefined;
    modelEdits = [];
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
    rs.spyOn(ProjectController.prototype, 'query').mockResolvedValue(undefined);
    // Edits stay pending: they never settle, as while a slow patch is in flight.
    rs.spyOn(ProjectController.prototype, 'enqueueModelEdit').mockImplementation((edit) => {
      modelEdits.push(edit);
      return new Promise<boolean>(() => {});
    });
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

  function openPanel(): RenderResult {
    let result!: RenderResult;
    act(() => {
      result = render(React.createElement(Editor, makeProps()));
    });
    act(() => {
      canvasProps!.onSetSelection(new Set([1]));
      canvasProps!.onShowVariableDetails();
    });
    expect(result.container.querySelector('.unitsEditor')).not.toBeNull();
    return result;
  }

  it('a press outside the panel flushes the draft as one model edit, and a repeat submission is not enqueued twice', async () => {
    const { container, getByTestId } = openPanel();
    await appendUnits(container, ' per year');
    expect(modelEdits).toHaveLength(0);

    act(() => {
      fireEvent.pointerDown(getByTestId('canvas'));
      fireEvent.pointerUp(getByTestId('canvas'));
    });
    expect(modelEdits).toHaveLength(1);
    const patch = modelEdits[0].buildPatch(baseProject());
    const op = patch.models![0].ops[0] as { type: string; payload: { aux: { units?: string } } };
    expect(op.type).toBe('upsertAux');
    expect(op.payload.aux.units).toBe('people per year');

    // A second press (or the blur that follows) submits the same pending draft.
    act(() => {
      fireEvent.pointerDown(getByTestId('canvas'));
    });
    expect(modelEdits).toHaveLength(1);
  });

  it('a draft that differs from the latest pending submission is enqueued, even one changed back to an earlier pending draft', async () => {
    const { container, getByTestId } = openPanel();
    const press = () =>
      act(() => {
        fireEvent.pointerDown(getByTestId('canvas'));
        fireEvent.pointerUp(getByTestId('canvas'));
      });
    const unitsOf = (i: number) =>
      (modelEdits[i].buildPatch(baseProject()).models![0].ops[0] as { payload: { aux: { units?: string } } }).payload
        .aux.units;
    await appendUnits(container, ' per year');
    press();
    await appendUnits(container, '!');
    press();
    // Changed back to the first draft while both earlier edits are still pending.
    const editor = unitsEditor(container);
    await act(async () => {
      Transforms.delete(editor, { at: SlateEditor.end(editor, []), distance: 1, unit: 'character', reverse: true });
      editor.onChange();
      await Promise.resolve();
    });
    press();
    expect(modelEdits.map((_, i) => unitsOf(i))).toEqual(['people per year', 'people per year!', 'people per year']);
  });

  it('the same variable ident in another model is its own submission target, and its own panel', async () => {
    const { container, getByTestId } = openPanel();
    const press = () =>
      act(() => {
        fireEvent.pointerDown(getByTestId('canvas'));
        fireEvent.pointerUp(getByTestId('canvas'));
      });
    await appendUnits(container, ' per year');
    press();
    expect(modelEdits).toHaveLength(1);
    // Unsubmitted text past the submission holds main's panel key.
    await appendUnits(container, '!');
    // Drilled into a child model holding a variable with the same ident,
    // element uid and content, while main's edit is still pending.
    const project = baseProject();
    const main = project.models.get('main')!;
    const twoModels = { ...project, models: new Map([...project.models, ['child', { ...main, name: 'child' }]]) };
    publish({ ...makeSnapshot(twoModels, 2), modelName: 'child' } as ProjectSnapshot);
    expect(unitsText(container)).toBe('people');
    await appendUnits(container, ' per year');
    press();
    expect(modelEdits).toHaveLength(2);
  });

  it('a press inside the panel does not flush', async () => {
    const { container } = openPanel();
    await appendUnits(container, '!');
    act(() => {
      fireEvent.pointerDown(container.querySelector('.unitsEditor')!);
    });
    expect(modelEdits).toHaveLength(0);
  });

  it("a landed change to the variable's errors keeps the draft; its submitted edit landing re-seeds it", async () => {
    const { container, getByTestId } = openPanel();
    const editorBefore = unitsEditor(container);
    await appendUnits(container, ' per year');
    expect(unitsText(container)).toBe('people per year');

    // An unrelated edit landed and broke x (e.g. a variable x reads was deleted).
    publish(
      makeSnapshot(
        withX(baseProject(), {
          errors: [{ start: 0, end: 1, code: 1 }],
          unitErrors: [{ start: 0, end: 6, code: 1, kind: 'definition' }],
        } as Partial<Variable>),
        2,
      ),
    );
    expect(unitsEditor(container)).toBe(editorBefore);
    expect(unitsText(container)).toBe('people per year');

    // Committed content that merely equals an unsubmitted draft does not release
    // it: the text differs from what the panel last submitted (nothing).
    publish(makeSnapshot(withX(baseProject(), { units: 'people per year' }), 3));
    expect(unitsEditor(container)).toBe(editorBefore);

    // The draft is submitted and its edit lands: the panel re-seeds.
    act(() => {
      fireEvent.pointerDown(getByTestId('canvas'));
      fireEvent.pointerUp(getByTestId('canvas'));
    });
    expect(modelEdits).toHaveLength(1);
    publish(makeSnapshot(withX(baseProject(), { units: 'people per year' }), 4));
    expect(unitsEditor(container)).not.toBe(editorBefore);
    expect(unitsText(container)).toBe('people per year');
  });
});
