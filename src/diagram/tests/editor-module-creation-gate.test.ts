/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// The Editor's getEditorControls() renders the SpeedDial toolbar. Module
// CREATION is the one tool gated behind the `moduleCreationEnabled` prop: the
// hosting app keeps it on for dev builds but off for production while the
// feature is still being debugged. The other creation tools (stock, flow,
// variable, link) are always present, and module wiring/details editing and
// drill-in navigation are unaffected -- only the toolbar's "Module" tool, the
// sole entry point to the module-creation flow, appears or disappears.
//
// We assert against OBSERVABLE output: render a real <Editor> and check which
// tool buttons the SpeedDial contains. SpeedDial is mocked so it renders its
// action children unconditionally (the real one only renders them while open),
// letting us query each tool by the aria-label its SpeedDialAction sets.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render, screen } from '@testing-library/react';

import { ProjectController, type ProjectSnapshot } from '../project-controller';

// Render the SpeedDial's action children unconditionally (the real component
// hides them until the dial is open) and surface each SpeedDialAction as a
// button carrying its title as the aria-label, so tools are queryable by name.
jest.mock('../components/SpeedDial', () => {
  const react = jest.requireActual('react') as typeof import('react');
  return {
    __esModule: true,
    default: (p: { children?: React.ReactNode }) => react.createElement('div', null, p.children),
    SpeedDialAction: (p: { title: string }) => react.createElement('button', { type: 'button', 'aria-label': p.title }),
    SpeedDialIcon: () => null,
  };
});

// Canvas mounts a ResizeObserver and reads SVG geometry jsdom lacks; this test
// exercises only the toolbar, so stub Canvas to a null renderer.
jest.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: () => null,
  inCreationUid: -2,
}));

// Import the Editor AFTER jest.mock so it binds to the stubs.
import { Editor, type EditorProps } from '../Editor';

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

describe('Editor module-creation tool gating', () => {
  beforeEach(() => {
    jest.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    jest.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    jest.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  test('shows the Module tool when module creation is explicitly enabled', () => {
    renderEditor(makeProps({ moduleCreationEnabled: true }));
    expect(screen.queryByLabelText('Module')).not.toBeNull();
  });

  test('hides the Module tool when module creation is disabled', () => {
    renderEditor(makeProps({ moduleCreationEnabled: false }));
    expect(screen.queryByLabelText('Module')).toBeNull();
    // The other creation tools remain available -- only module creation is gated.
    expect(screen.queryByLabelText('Stock')).not.toBeNull();
    expect(screen.queryByLabelText('Flow')).not.toBeNull();
    expect(screen.queryByLabelText('Variable')).not.toBeNull();
    expect(screen.queryByLabelText('Link')).not.toBeNull();
  });

  test('shows the Module tool by default when the prop is omitted', () => {
    renderEditor(makeProps());
    expect(screen.queryByLabelText('Module')).not.toBeNull();
  });
});
