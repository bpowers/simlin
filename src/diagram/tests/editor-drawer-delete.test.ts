/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// The Editor's getDrawer() decides whether the model-properties drawer offers a
// "Delete project" action: it forwards the host's onDeleteProject callback only
// when the editor is editable (not read-only) and a callback was actually
// supplied, and it renders no drawer at all when embedded.
//
// The Editor is now a function component, so we assert against OBSERVABLE
// behavior: render a real <Editor> and capture the props it hands to its
// ModelPropertiesDrawer child (the drawer is mocked to a prop-recording stub).
// The controller is stubbed so a seeded snapshot supplies the active project
// without WASM, and the drawer element is rendered unconditionally by
// getDrawer() (independent of the open/closed state), so no router or drawer
// interaction is needed.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render } from '@testing-library/react';

import type { ModelPropertiesDrawer as ModelPropertiesDrawerType } from '../ModelPropertiesDrawer';
import { ProjectController, type ProjectSnapshot } from '../project-controller';

// Capture the props the Editor passes into its ModelPropertiesDrawer.
type DrawerProps = React.ComponentProps<typeof ModelPropertiesDrawerType>;
let capturedDrawerProps: DrawerProps | undefined;

jest.mock('../ModelPropertiesDrawer', () => ({
  __esModule: true,
  ModelPropertiesDrawer: (p: DrawerProps) => {
    capturedDrawerProps = p;
    return null;
  },
}));

// Canvas mounts a ResizeObserver and reads SVG geometry that jsdom lacks; this
// test exercises only the drawer wiring, so stub Canvas to a null renderer.
// (inCreationUid is re-exported so the Editor's import keeps resolving.)
jest.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: () => null,
  inCreationUid: -2,
}));

// Import the Editor AFTER jest.mock so it binds to the stub drawer.
import { Editor, type EditorProps } from '../Editor';

// A minimal snapshot whose project + modelName getDrawer() reads.
function makeSnapshot(): ProjectSnapshot {
  const view = {
    nextUid: 1,
    elements: [],
    viewBox: { x: 0, y: 0, width: 800, height: 600 },
    zoom: 1,
    useLetteredPolarity: false,
  };
  return {
    project: {
      name: 'test-project',
      models: new Map([['main', { name: 'main', variables: new Map(), views: [view], loopMetadata: [], groups: [] }]]),
      simSpecs: {
        start: 0,
        stop: 100,
        dt: { isReciprocal: false, value: 1 },
        timeUnits: 'years',
      },
    },
    modelName: 'main',
    projectVersion: 1,
    projectGeneration: 0,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelStack: [],
    canUndo: false,
    canRedo: false,
    navResetSeq: 0,
  } as unknown as ProjectSnapshot;
}

function makeProps(overrides: Partial<EditorProps> = {}): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: '{}',
    initialProjectVersion: 1,
    name: 'test-project',
    embedded: false,
    readOnlyMode: false,
    onSave: async () => 1,
    ...overrides,
  } as EditorProps;
}

function renderEditor(props: EditorProps): void {
  act(() => {
    render(React.createElement(Editor, props));
  });
}

describe('Editor.getDrawer() delete wiring', () => {
  beforeEach(() => {
    capturedDrawerProps = undefined;
    // Stub the controller so a seeded snapshot supplies the project and the
    // engine never opens (keeps the test off WASM).
    jest.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    jest.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    jest.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  test('forwards onDeleteProject as the drawer onDelete when editable', () => {
    const onDeleteProject = jest.fn(async () => {});
    renderEditor(makeProps({ onDeleteProject }));
    expect(capturedDrawerProps).toBeDefined();
    expect(capturedDrawerProps!.onDelete).toBe(onDeleteProject);
  });

  test('omits onDelete when the editor is read-only', () => {
    const onDeleteProject = jest.fn(async () => {});
    renderEditor(makeProps({ onDeleteProject, readOnlyMode: true }));
    expect(capturedDrawerProps).toBeDefined();
    expect(capturedDrawerProps!.onDelete).toBeUndefined();
  });

  test('omits onDelete when no onDeleteProject callback is supplied', () => {
    renderEditor(makeProps());
    expect(capturedDrawerProps).toBeDefined();
    expect(capturedDrawerProps!.onDelete).toBeUndefined();
  });

  test('renders no drawer at all when embedded', () => {
    renderEditor(makeProps({ onDeleteProject: jest.fn(async () => {}), embedded: true }));
    expect(capturedDrawerProps).toBeUndefined();
  });
});
