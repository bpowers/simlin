// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor mounted WITHOUT reset.css. Hosts that import the package root get
// reset.css and theme.css automatically, but a host embedding the Editor in a
// page it owns (a notebook cell) deep-imports and loads only theme.css --
// putting a page-wide `*{box-sizing:border-box}` / body typography reset into
// someone else's document is not acceptable. The Editor's own stylesheets must
// therefore not lean on the reset: every box that mixes an explicit width with
// padding/border declares border-box itself, the editor root and each portaled
// surface pin their typography, and the raw elements the tree renders (h2, img,
// select) carry the margins/display/font the reset would have supplied.
//
// The test compiles the package's CSS modules into the jsdom document. rstest
// keeps class names equal to their local names ([local]), so the raw
// stylesheet text applies to the rendered markup; jsdom's getComputedStyle
// resolves the cascade for declared values (not layout, not var()), which is
// exactly what these assertions read. reset.css is deliberately NOT injected.

import { describe, it, expect, beforeAll, beforeEach, afterEach, rs } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import * as React from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';

import { ProjectController, type ProjectSnapshot } from '../project-controller';

// The canvas is not under test (jsdom has no SVG geometry); a marker stands in.
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
  // CSS-module scoping syntax that a plain CSS parser rejects.
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

function makeProps(): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: '{}',
    initialProjectVersion: 1,
    name: 'test-project',
    embedded: false,
    readOnlyMode: false,
    onSave: async () => 1,
  } as EditorProps;
}

// jsdom does not resolve var() in computed values, so typography is checked in
// two steps: the surface references the theme token, and the token (read from
// theme.css) names Roboto.
const FONT_TOKEN = /var\(--font-family-base\)/;

function fontTokenNamesRoboto(): boolean {
  const theme = fs.readFileSync(path.join(diagramDir, 'theme.css'), 'utf-8');
  return /--font-family-base:\s*['"]Roboto['"]/.test(theme);
}

describe('Editor mounted without reset.css', () => {
  beforeAll(() => {
    const style = document.createElement('style');
    style.setAttribute('data-test', 'diagram-css-without-reset');
    style.textContent = collectModuleCss();
    document.head.appendChild(style);
  });

  let consoleError: ReturnType<typeof rs.spyOn>;

  beforeEach(() => {
    consoleError = rs.spyOn(console, 'error').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockReturnValue(makeSnapshot());
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function mount(): HTMLElement {
    let container!: HTMLElement;
    act(() => {
      container = render(React.createElement(Editor, makeProps())).container;
    });
    const root = container.firstElementChild as HTMLElement | null;
    expect(root).not.toBeNull();
    return root!;
  }

  it('the stylesheet actually reached the document (guards the test itself)', () => {
    const root = mount();
    // A declared value that only the module CSS supplies: if the injection
    // silently failed every assertion below would pass vacuously.
    expect(window.getComputedStyle(root).overflow).toBe('hidden');
  });

  it('mounts and renders its chrome without throwing or logging errors', () => {
    const root = mount();
    expect(root.querySelector('.searchBar')).not.toBeNull();
    expect(consoleError).not.toHaveBeenCalled();
  });

  it('the root pins its own box model and typography', () => {
    const cs = window.getComputedStyle(mount());
    expect(cs.position).toBe('relative');
    expect(cs.boxSizing).toBe('border-box');
    expect(cs.fontFamily).toMatch(FONT_TOKEN);
    expect(fontTokenNamesRoboto()).toBe(true);
    expect(cs.lineHeight).toBe('1.5');
    expect(cs.fontSize).toBe('1rem');
  });

  it('the search bar (explicit width + padding) is border-box and absolute', () => {
    const root = mount();
    const cs = window.getComputedStyle(root.querySelector('.searchBar')!);
    expect(cs.boxSizing).toBe('border-box');
    expect(cs.position).toBe('absolute');
  });

  it('the toast viewport is anchored inside the root with no list chrome', () => {
    const root = mount();
    const viewport = root.querySelector('.toastViewport');
    expect(viewport).not.toBeNull();
    const cs = window.getComputedStyle(viewport!);
    expect(cs.position).toBe('absolute');
    expect(cs.margin).toBe('0px');
    expect(cs.padding).toBe('0px');
    expect(cs.listStyle || cs.listStyleType).toMatch(/none/);
  });

  it('nothing under the root computes position: fixed', () => {
    const root = mount();
    const fixed = Array.from(root.querySelectorAll('*')).filter(
      (el) => window.getComputedStyle(el).position === 'fixed',
    );
    expect(fixed).toEqual([]);
  });

  it('the portaled drawer sheet pins its typography and its heading has no UA margin', () => {
    mount();
    act(() => {
      fireEvent.click(screen.getByLabelText(/^menu$/i));
    });
    const panel = document.querySelector('.panel');
    expect(panel).not.toBeNull();
    const cs = window.getComputedStyle(panel!);
    expect(cs.fontFamily).toMatch(FONT_TOKEN);
    expect(cs.lineHeight).toBe('1.5');
    const heading = panel!.querySelector('h2');
    expect(heading).not.toBeNull();
    expect(window.getComputedStyle(heading!).margin).toBe('0px');
  });
});
