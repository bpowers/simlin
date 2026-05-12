/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// getDrawer() decides whether the model-properties drawer offers a "Delete
// project" action: it forwards the host's onDeleteProject callback only when
// the editor is editable (not read-only) and a callback was actually
// supplied. We poke getDrawer() directly via Object.create(Editor.prototype)
// to avoid spinning up WASM / React in the constructor; only the state and
// props getDrawer() reads are seeded.

import * as React from 'react';

import { Editor } from '../Editor';
import { ModelPropertiesDrawer } from '../ModelPropertiesDrawer';

type EditorInstance = InstanceType<typeof Editor>;

function makeEditor(propsOverride: Record<string, unknown> = {}): EditorInstance {
  const editor = Object.create(Editor.prototype) as EditorInstance;

  editor.state = {
    drawerOpen: false,
    modelName: 'main',
    activeProject: {
      name: 'test-project',
      models: new Map([['main', { name: 'main' }]]),
      simSpecs: {
        start: 0,
        stop: 100,
        dt: { isReciprocal: false, value: 1 },
        timeUnits: 'years',
      },
    },
  } as unknown as EditorInstance['state'];

  editor.props = {
    embedded: false,
    readOnlyMode: false,
    ...propsOverride,
  } as unknown as EditorInstance['props'];

  return editor;
}

function drawerElement(editor: EditorInstance): React.ReactElement<React.ComponentProps<typeof ModelPropertiesDrawer>> {
  const el = editor.getDrawer();
  if (!el || !React.isValidElement(el)) {
    throw new Error('expected getDrawer() to return a ModelPropertiesDrawer element');
  }
  expect(el.type).toBe(ModelPropertiesDrawer);
  return el as React.ReactElement<React.ComponentProps<typeof ModelPropertiesDrawer>>;
}

describe('Editor.getDrawer() delete wiring', () => {
  test('forwards onDeleteProject as the drawer onDelete when editable', () => {
    const onDeleteProject = jest.fn(async () => {});
    const editor = makeEditor({ onDeleteProject });
    expect(drawerElement(editor).props.onDelete).toBe(onDeleteProject);
  });

  test('omits onDelete when the editor is read-only', () => {
    const onDeleteProject = jest.fn(async () => {});
    const editor = makeEditor({ onDeleteProject, readOnlyMode: true });
    expect(drawerElement(editor).props.onDelete).toBeUndefined();
  });

  test('omits onDelete when no onDeleteProject callback is supplied', () => {
    const editor = makeEditor();
    expect(drawerElement(editor).props.onDelete).toBeUndefined();
  });

  test('renders no drawer at all when embedded', () => {
    const editor = makeEditor({ onDeleteProject: jest.fn(async () => {}), embedded: true });
    expect(editor.getDrawer()).toBeUndefined();
  });
});
