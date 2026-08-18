// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Embeddability contracts for the Editor's stylesheets. The Editor is mounted
// by hosts that own the whole viewport (src/app's fixed full-page shell,
// simlin-serve's page) AND by hosts that give it one positioned box on a page
// they own (a notebook output cell, several per page). Chrome that positions
// or sizes itself against the VIEWPORT (position: fixed, vw/vh units) escapes
// such a box; everything inside the Editor tree must therefore anchor to the
// Editor root and size against it. jsdom can't do layout, so -- like
// tests/panel-css.test.ts -- these assert the stylesheet text.
//
// The exemptions are deliberate and enumerated: content rendered through a
// portal to document.body (Drawer, Dialog) IS viewport-level and correctly
// uses fixed positioning / viewport units, as does the drawer's sheet inside
// it; AppBar is page chrome the app composes outside the Editor; the
// HostedWebEditor shell is src/app's own full-page host; reset.css is a
// page-level reset a host may or may not load.

import { describe, it, expect } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

const diagramDir = path.join(__dirname, '..');

function readCss(name: string): string {
  return fs.readFileSync(path.join(diagramDir, name), 'utf-8');
}

/** All stylesheet paths (relative to src/diagram) excluding build output. */
function allCssFiles(): string[] {
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      if (entry.name.startsWith('lib') || entry.name === 'node_modules' || entry.name === 'tests') {
        continue;
      }
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.name.endsWith('.css')) {
        out.push(path.relative(diagramDir, full));
      }
    }
  };
  walk(diagramDir);
  return out.sort();
}

function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '');
}

/** The text of the first declaration block for `selector`. */
function blockFor(css: string, selector: string): string {
  const re = new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\{([^}]*)\\}`);
  const m = re.exec(css);
  if (!m) {
    throw new Error(`no block for ${selector}`);
  }
  return m[1];
}

// Stylesheets whose rules render OUTSIDE the Editor's DOM subtree (portals to
// document.body, page-level chrome, the app's host shell, the page reset) and
// may therefore reference the viewport.
const VIEWPORT_LEVEL_STYLESHEETS = new Set([
  'components/Dialog.module.css', // Radix portal to document.body
  'components/Drawer.module.css', // ReactDOM portal to document.body
  'ModelPropertiesDrawer.module.css', // the sheet inside the Drawer portal
  'components/AppBar.module.css', // page chrome, composed by src/app outside the Editor
  'HostedWebEditor.module.css', // src/app's full-page host shell around the Editor
  'reset.css', // page-level reset
]);

describe('nothing inside the Editor tree positions or sizes against the viewport', () => {
  const files = allCssFiles();

  it('finds the stylesheets to check', () => {
    expect(files.length).toBeGreaterThan(20);
    for (const exempt of VIEWPORT_LEVEL_STYLESHEETS) {
      expect(files).toContain(exempt);
    }
  });

  const inTree = files.filter((f) => !VIEWPORT_LEVEL_STYLESHEETS.has(f));

  it.each(inTree)('%s has no position: fixed', (file) => {
    expect(stripComments(readCss(file))).not.toMatch(/position:\s*fixed/);
  });

  it.each(inTree)('%s uses no viewport units (vw/vh/vmin/vmax)', (file) => {
    expect(stripComments(readCss(file))).not.toMatch(/\d(?:vw|vh|vmin|vmax|dvh|svh|lvh|dvw|svw|lvw)\b/);
  });
});

describe('the Editor root is the containing block for its floating chrome', () => {
  it('.editor is position: relative', () => {
    expect(blockFor(readCss('Editor.module.css'), '.editor')).toContain('position: relative');
  });

  it('the toast viewport is absolute (anchored to the editor root), not fixed', () => {
    const block = blockFor(stripComments(readCss('components/Snackbar.module.css')), '.toastViewport');
    expect(block).toContain('position: absolute');
    expect(block).not.toContain('fixed');
  });

  // The right-hand chrome (search bar, banner, the details slot) clamps its
  // width to the container -- calc(100% - 16px) of the positioned root -- so a
  // narrow host box never has it overflow the left edge; a full-page host
  // resolves 100% to the viewport, so nothing moves there. The width lives on
  // the positioned boxes themselves (never on the cards inside .varDetails: a
  // percentage inside a shrink-wrapped absolute box resolves against the
  // content-sized box, not the editor -- see the .varDetails comment).
  const clamped: Array<[string, string]> = [
    ['Editor.module.css', '.searchBar'],
    ['Editor.module.css', '.sharedModelBanner'],
    ['Editor.module.css', '.varDetails'],
  ];

  it.each(clamped)('%s %s clamps its width to calc(100% - 16px) at every breakpoint', (file, selector) => {
    const css = stripComments(readCss(file));
    const re = new RegExp(`${selector.replace('.', '\\.')}\\s*\\{([^}]*)\\}`, 'g');
    const blocks: string[] = [];
    for (let m = re.exec(css); m !== null; m = re.exec(css)) {
      blocks.push(m[1]);
    }
    // base rule + the md and lg media overrides
    expect(blocks.length).toBe(3);
    for (const b of blocks) {
      expect(b.replace(/\s+/g, ' ')).toMatch(/width: min\(var\(--panel-width-(?:sm|md|lg)\), calc\(100% - 16px\)\)/);
    }
  });

  // The height cap sits on the positioned slot (where the percentage resolves
  // against the editor root) and the cards opt out of the flex-item
  // min-height: auto floor so they shrink to it and scroll. This is only the
  // stylesheet's shape; whether it actually caps in a 400px box is asserted
  // by the notebook widget's Playwright journey (src/notebook-widget/e2e),
  // which has a layout engine -- a text assertion here once stayed green
  // while the card overflowed the box.
  it('.varDetails caps its height against the container and lays the card out as a shrinkable flex item', () => {
    const block = blockFor(stripComments(readCss('Editor.module.css')), '.varDetails').replace(/\s+/g, ' ');
    expect(block).toContain('max-height: calc(100% - 18px)');
    expect(block).toContain('display: flex');
    expect(block).toContain('flex-direction: column');
  });

  it.each([
    ['VariableDetails.module.css', '.card'],
    ['ModuleDetails.module.css', '.card'],
    ['ErrorDetails.module.css', '.card'],
  ])(
    '%s %s shrinks to the slot cap (min-height: 0; overflow-y: auto) and declares no width or height cap of its own',
    (file, selector) => {
      const block = blockFor(stripComments(readCss(file)), selector).replace(/\s+/g, ' ');
      expect(block).toContain('min-height: 0');
      expect(block).toContain('overflow-y: auto');
      expect(block).not.toMatch(/(?:^|[^-])width:/);
      expect(block).not.toContain('max-height');
    },
  );
});
