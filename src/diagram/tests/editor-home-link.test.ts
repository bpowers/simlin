// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor's hamburger drawer offers an "Exit" link to "/" -- the app's and
// simlin-serve's project list. A host that embeds the Editor in a page it owns
// (a notebook cell) has no such route, and the link would pushState on the
// host page, so `showHomeLink={false}` hides it. Neither setting needs a
// router: nothing here wraps the Editor in wouter's <Router>, and the real
// ModelPropertiesDrawer renders in both configurations.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';

import { ProjectController, type ProjectSnapshot } from '../project-controller';

// The canvas is not under test; keep jsdom off SVG geometry.
rs.mock('../drawing/Canvas', () => ({
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
    serverVersion: 1,
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

describe('Editor showHomeLink', () => {
  beforeEach(() => {
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function mountAndOpenDrawer(props: EditorProps): void {
    act(() => {
      render(React.createElement(Editor, props));
    });
    // The hamburger in the search bar opens the model-properties drawer.
    act(() => {
      fireEvent.click(screen.getByLabelText(/^menu$/i));
    });
  }

  it('shows the Exit link by default (app and simlin-serve hosts), with no router in the tree', () => {
    mountAndOpenDrawer(makeProps());
    const exit = screen.getByRole('link', { name: /exit/i });
    expect(exit.getAttribute('href')).toBe('/');
  });

  it('hides the Exit link when showHomeLink is false, and the drawer otherwise works', () => {
    mountAndOpenDrawer(makeProps({ showHomeLink: false }));
    expect(screen.queryByRole('link', { name: /exit/i })).toBeNull();
    expect(screen.getByRole('button', { name: /download model/i })).not.toBeNull();
    expect(screen.getByRole('button', { name: /close/i })).not.toBeNull();
  });
});
