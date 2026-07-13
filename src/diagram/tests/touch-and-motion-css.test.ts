// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mobile/touch and reduced-motion stylesheet contracts. jsdom can't do layout
// or evaluate media queries, so -- like tests/panel-css.test.ts -- these
// assert the stylesheet text; the geometry math lives in comments beside each
// rule in the CSS.
//
//  - Touch targets: interactive controls whose visual box is smaller than the
//    44px WCAG/Material target grow their HIT area under
//    @media (pointer: coarse). Two patterns, chosen per control:
//      * overlay -- a centered transparent ::after with 44px minimums; needs
//        position: relative on the control and is only safe where neighbors
//        don't sit flush along the growth axis;
//      * real growth -- min-height on the control itself, used where flush
//        stacking (menu items) or overflow: hidden (breadcrumb links) rules
//        the overlay out.
//
//  - Reduced motion: one global policy in theme.css (the stylesheet every
//    host is contractually required to load) neutralizes decorative CSS
//    motion, instead of per-component opt-ins that were only ever added in
//    two places.

import { describe, it, expect } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

function readCss(...segments: string[]): string {
  return fs.readFileSync(path.join(__dirname, '..', ...segments), 'utf-8');
}

/** The contents of the first `@media (<condition>) { ... }` block, extracted
 *  with balanced-brace scanning (the block nests rule blocks, so a regex to
 *  the first `}` would truncate it). Throws when the block is missing so a
 *  deleted policy fails loudly rather than vacuously passing. */
function mediaBlock(css: string, condition: string): string {
  const marker = `@media (${condition})`;
  const start = css.indexOf(marker);
  if (start < 0) {
    throw new Error(`no "${marker}" block found`);
  }
  const open = css.indexOf('{', start);
  let depth = 1;
  let i = open + 1;
  for (; i < css.length && depth > 0; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}') depth--;
  }
  if (depth !== 0) {
    throw new Error(`unbalanced braces in "${marker}" block`);
  }
  return css.slice(open + 1, i - 1);
}

/** The text of the first declaration block for `selector` within `css`. */
function blockFor(css: string, selector: string): string {
  const re = new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\{([^}]*)\\}`);
  const m = re.exec(css);
  if (!m) {
    throw new Error(`no block for ${selector}`);
  }
  return m[1];
}

describe('drawer width clamps to the viewport', () => {
  it('ModelPropertiesDrawer .content is min(375px, 100vw - margin), not a fixed 375px', () => {
    // A fixed 375px sheet overflows a 320px phone and clips its own close
    // button; the clamp leaves a tappable strip of backdrop at any width.
    const css = readCss('ModelPropertiesDrawer.module.css');
    const content = blockFor(css, '.content');
    expect(content.replace(/\s+/g, ' ')).toContain('width: min(375px, calc(100vw - 48px))');
  });
});

describe('coarse-pointer touch targets: overlay pattern', () => {
  // (file, control selector, overlay minimums that must appear in the coarse
  // block). Width minimums are omitted where flush horizontal neighbors (a
  // Cancel/Save button pair) would make overlapping overlays misroute taps.
  const overlays: Array<[string, string, string[]]> = [
    ['components/IconButton.module.css', '.iconButton', ['min-width: 44px', 'min-height: 44px']],
    ['components/SpeedDial.module.css', '.actionButton', ['min-width: 44px', 'min-height: 44px']],
    ['components/Button.module.css', '.button', ['min-height: 44px']],
    ['Status.module.css', '.status', ['min-width: 44px', 'min-height: 44px']],
  ];

  it.each(overlays)('%s %s grows its hit area via a centered ::after overlay', (file, selector, mins) => {
    const css = readCss(file);
    const coarse = mediaBlock(css, 'pointer: coarse');
    const overlay = blockFor(coarse, `${selector}::after`);
    expect(overlay).toContain("content: ''");
    expect(overlay).toContain('position: absolute');
    for (const min of mins) {
      expect(overlay).toContain(min);
    }
  });

  it.each(overlays)('%s %s anchors the overlay (position: relative on the control)', (file, selector) => {
    // Without a positioned ancestor the absolute overlay sizes against some
    // outer box instead of centering on the control -- silently wrong in a
    // way jsdom can't observe.
    const css = readCss(file);
    expect(blockFor(css, selector)).toContain('position: relative');
  });
});

describe('coarse-pointer touch targets: real-growth pattern', () => {
  it('Menu .menuItem grows to min-height 44px (flush stacking forbids overlays)', () => {
    const coarse = mediaBlock(readCss('components/Menu.module.css'), 'pointer: coarse');
    const item = blockFor(coarse, '.menuItem');
    expect(item).toContain('min-height: 44px');
    expect(item).toContain('box-sizing: border-box');
  });

  it('Editor .breadcrumbLink grows to min-height 44px (overflow: hidden clips overlays)', () => {
    const coarse = mediaBlock(readCss('Editor.module.css'), 'pointer: coarse');
    const link = blockFor(coarse, '.breadcrumbLink');
    expect(link).toContain('min-height: 44px');
    expect(link).toContain('box-sizing: border-box');
  });

  it('Autocomplete .option grows to min-height 44px (flush stacking forbids overlays)', () => {
    const coarse = mediaBlock(readCss('components/Autocomplete.module.css'), 'pointer: coarse');
    const option = blockFor(coarse, '.option');
    expect(option).toContain('min-height: 44px');
    expect(option).toContain('box-sizing: border-box');
  });
});

describe('reduced-motion policy', () => {
  it('theme.css carries the single global prefers-reduced-motion rule', () => {
    const reduce = mediaBlock(readCss('theme.css'), 'prefers-reduced-motion: reduce');
    // The universal selector is what makes this a policy rather than another
    // per-component opt-in.
    expect(reduce).toMatch(/\*\s*,\s*\*::before\s*,\s*\*::after\s*\{/);
    // 0.01ms (not `none`) keeps transitionend/animationend firing;
    // iteration-count: 1 ends infinite loops.
    expect(reduce).toContain('animation-duration: 0.01ms !important');
    expect(reduce).toContain('transition-duration: 0.01ms !important');
    expect(reduce).toContain('animation-iteration-count: 1 !important');
  });

  it('CircularProgress keeps its stronger animation: none override', () => {
    // The global policy leaves a 0.01ms one-shot spin; the spinner opts all
    // the way out so the ring holds still instead of jumping one frame.
    const reduce = mediaBlock(readCss('components/CircularProgress.module.css'), 'prefers-reduced-motion: reduce');
    expect(blockFor(reduce, '.spinner')).toContain('animation: none');
  });
});
