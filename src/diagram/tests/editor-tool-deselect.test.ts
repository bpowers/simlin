/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Tool deselection behavior (regressed in 5191a9b6, which dropped the
// selectedTool clearing from handleDialClick): the user expects that
//  (1) re-clicking the currently-selected tool deselects it, and
//  (2) closing the tool palette (the FAB) deselects the active tool.
// On load no tool is selected; both gestures must return to that state.
//
// We assert against OBSERVABLE output: render a real <Editor>, drive the
// SpeedDial's FAB and action buttons, and read each tool's `selected` flag
// (surfaced as data-selected on the mocked SpeedDialAction button).

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';

import { ProjectController, type ProjectSnapshot } from '../project-controller';

// Mock SpeedDial so its FAB and action children are always rendered and
// queryable. The FAB carries aria-label "dial-fab" and fires the dial's
// onClick (handleDialClick); each action surfaces its title as aria-label and
// its `selected` flag as data-selected, and fires its onClick handler.
jest.mock('../components/SpeedDial', () => {
  const react = jest.requireActual('react') as typeof import('react');
  return {
    __esModule: true,
    default: (p: { children?: React.ReactNode; onClick?: (e: unknown) => void }) =>
      react.createElement(
        'div',
        null,
        react.createElement('button', {
          type: 'button',
          'aria-label': 'dial-fab',
          onClick: (e: unknown) => p.onClick?.(e),
        }),
        p.children,
      ),
    SpeedDialAction: (p: { title: string; selected?: boolean; onClick?: (e: unknown) => void }) =>
      react.createElement('button', {
        type: 'button',
        'aria-label': p.title,
        'data-selected': p.selected ? 'true' : 'false',
        onClick: (e: unknown) => p.onClick?.(e),
      }),
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
      simSpecs: { start: 0, stop: 100, dt: { isReciprocal: false, value: 1 }, timeUnits: 'years' },
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

function renderEditor(props: EditorProps = makeProps()): void {
  act(() => {
    render(React.createElement(Editor, props));
  });
}

const toolSelected = (title: string): boolean => screen.getByLabelText(title).getAttribute('data-selected') === 'true';

describe('Editor tool deselection', () => {
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

  test('no tool is selected on load', () => {
    renderEditor();
    expect(toolSelected('Flow')).toBe(false);
    expect(toolSelected('Stock')).toBe(false);
  });

  test('clicking a tool selects it; re-clicking the active tool deselects it', () => {
    renderEditor();
    fireEvent.click(screen.getByLabelText('dial-fab')); // open the palette (real UX: tools only show when open)
    fireEvent.click(screen.getByLabelText('Flow'));
    expect(toolSelected('Flow')).toBe(true);

    fireEvent.click(screen.getByLabelText('Flow'));
    expect(toolSelected('Flow')).toBe(false);
  });

  test('clicking a different tool switches selection (does not toggle off)', () => {
    renderEditor();
    fireEvent.click(screen.getByLabelText('dial-fab')); // open the palette
    fireEvent.click(screen.getByLabelText('Flow'));
    expect(toolSelected('Flow')).toBe(true);

    fireEvent.click(screen.getByLabelText('Stock'));
    expect(toolSelected('Stock')).toBe(true);
    expect(toolSelected('Flow')).toBe(false);
  });

  test('closing the palette (FAB) deselects the active tool', () => {
    renderEditor();
    // Open the dial, select a tool, then close the dial.
    fireEvent.click(screen.getByLabelText('dial-fab')); // open
    fireEvent.click(screen.getByLabelText('Flow')); // select
    expect(toolSelected('Flow')).toBe(true);

    fireEvent.click(screen.getByLabelText('dial-fab')); // close -> deselect
    expect(toolSelected('Flow')).toBe(false);
  });
});
