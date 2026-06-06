/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression test for the details-panel remount key.
//
// The variable/module details panels seed their Slate editors from props in
// their constructors, so React remounts (key changes) are the mechanism that
// refreshes them after the underlying variable changes. The key uses
// `projectGeneration` (not `projectVersion`): projectGeneration increments
// exactly when project *content* changes (real edits, undo/redo) and stays
// put for view-only pan/zoom frames and save-version bookkeeping, so an open
// panel is not remounted -- discarding in-progress edits -- on a pan or an
// autosave.
//
// The *semantics* of projectGeneration (when it does/doesn't increment) now
// live in and are tested against the ProjectController -- see
// project-controller.test.ts ("view-only updates never consume undo slots",
// the applyPatch pipeline generation bump, and the undo generation bump). What
// remains Editor-specific is that the rendered panel key is derived from the
// controller snapshot's projectGeneration, which this test pins.

import * as React from 'react';

import { projectFromJson, type JsonProject } from '@simlin/core/datamodel';

import { Editor } from '../Editor';
import { VariableDetails } from '../VariableDetails';

type EditorInstance = InstanceType<typeof Editor>;

// A project with a single aux 'x' and a view element selecting it, so
// getDetails() resolves to a VariableDetails panel we can read the key off.
const projectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: [{ name: 'x', equation: '1' }],
      views: [{ elements: [{ type: 'aux', uid: 1, name: 'x', x: 0, y: 0 }] }],
    },
  ],
});

function makeEditor(projectGeneration: number): EditorInstance {
  const editor = Object.create(Editor.prototype) as EditorInstance;
  const project = projectFromJson(JSON.parse(projectJson) as JsonProject);

  editor.state = {
    controllerSnapshot: {
      project,
      projectGeneration,
      modelName: 'main',
    },
    selection: new Set<number>([1]),
    showDetails: 'variable',
    flowStillBeingCreated: false,
    variableDetailsActiveTab: 0,
  } as unknown as EditorInstance['state'];

  editor.props = { inputFormat: 'json', name: 'test' } as unknown as EditorInstance['props'];

  return editor;
}

function detailsKey(editor: EditorInstance): string {
  const wrapper = editor.getDetails();
  if (!wrapper || !React.isValidElement(wrapper)) {
    throw new Error('expected getDetails() to render a panel');
  }
  // getDetails() wraps the panel in a div; the panel is its single child.
  const child = (wrapper.props as { children: React.ReactElement }).children;
  expect(child.type).toBe(VariableDetails);
  return String(child.key);
}

describe('Editor details-panel remount key', () => {
  it('derives the VariableDetails key from controllerSnapshot.projectGeneration', () => {
    expect(detailsKey(makeEditor(0))).toBe('vd-0-x');
    expect(detailsKey(makeEditor(7))).toBe('vd-7-x');
  });
});
