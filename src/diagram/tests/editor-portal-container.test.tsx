// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor's `portalContainer` prop (Hosting Requirements, "Portals"): with
// no container the model-properties drawer portals to document.body and is
// fixed against the viewport; with a host box it renders INSIDE that box and
// is absolute against it, so the box's tokens/attributes reach it and nothing
// in it depends on the viewport. The component-level enumeration of every
// portaled surface is tests/portal-container.test.tsx; this file pins the
// Editor's wiring of the prop and reads the mode's `position` from the
// package's compiled stylesheets (jsdom resolves the cascade for declared
// values, as tests/editor-without-reset-css.test.ts relies on).

import { describe, it, expect, beforeAll, beforeEach, afterEach, rs } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

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

const diagramDir = path.join(__dirname, '..');

function collectModuleCss(): string {
  const chunks: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      if (entry.name.startsWith('lib') || entry.name === 'node_modules' || entry.name === 'tests') {
        continue;
      }
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.name.endsWith('.module.css') || entry.name === 'theme.css') {
        chunks.push(fs.readFileSync(full, 'utf-8'));
      }
    }
  };
  walk(diagramDir);
  return chunks.join('\n').replace(/:global\(([^)]*)\)/g, '$1');
}

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

describe('Editor portalContainer', () => {
  beforeAll(() => {
    const style = document.createElement('style');
    style.setAttribute('data-test', 'diagram-css-for-portal-modes');
    style.textContent = collectModuleCss();
    document.head.appendChild(style);
  });

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

  // A host box the way a notebook cell provides one: positioned, and NOT the
  // Editor's own root (the drawer must land in the host's box, not inside the
  // .editor tree).
  function makeHostBox(): HTMLElement {
    const box = document.createElement('div');
    box.setAttribute('data-testid', 'host-box');
    box.style.position = 'relative';
    document.body.appendChild(box);
    return box;
  }

  function mountAndOpenDrawer(props: EditorProps, into?: HTMLElement): { panel: HTMLElement; backdrop: HTMLElement } {
    act(() => {
      render(React.createElement(Editor, props), into ? { container: into } : undefined);
    });
    act(() => {
      fireEvent.click(screen.getByLabelText(/^menu$/i));
    });
    const panel = document.querySelector('[role="dialog"]') as HTMLElement;
    const backdrop = panel.previousElementSibling as HTMLElement;
    return { panel, backdrop };
  }

  it('by default the drawer portals to document.body and is fixed against the viewport', () => {
    const box = makeHostBox();
    const { panel, backdrop } = mountAndOpenDrawer(makeProps(), box);
    expect(panel.parentElement).toBe(document.body);
    expect(box.contains(panel)).toBe(false);
    expect(window.getComputedStyle(panel).position).toBe('fixed');
    expect(window.getComputedStyle(backdrop).position).toBe('fixed');
    box.remove();
  });

  it('with portalContainer the drawer renders inside that box and is absolute against it', () => {
    const box = makeHostBox();
    const { panel, backdrop } = mountAndOpenDrawer(makeProps({ portalContainer: box }), box);
    expect(box.contains(panel)).toBe(true);
    // Appended to the box itself (a sibling of the Editor root), not inside
    // the Editor's own tree, so it stacks above every piece of Editor chrome.
    expect(panel.parentElement).toBe(box);
    expect(panel.closest('[data-simlin-editor-root]')).toBeNull();
    expect(window.getComputedStyle(panel).position).toBe('absolute');
    expect(window.getComputedStyle(backdrop).position).toBe('absolute');
    // The sheet still works: its controls render and Close closes it.
    expect(screen.getByRole('button', { name: /download model/i })).not.toBeNull();
    act(() => {
      fireEvent.click(screen.getByRole('button', { name: /^close$/i }));
    });
    expect(panel.className).toContain('panelHidden');
    box.remove();
  });
});
