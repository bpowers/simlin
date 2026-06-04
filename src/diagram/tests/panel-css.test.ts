// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Layout contracts for the right-hand panels (search bar, details cards)
// and the equation preview. jsdom can't do real layout, so these assert the
// stylesheet text directly:
//
//  - The search bar and the details cards are width: var(--panel-width-*)
//    boxes anchored at right: 8px; their left edges align only if every box
//    that carries horizontal padding also computes as border-box. The global
//    reset (reset.css) is not reliably present in every host bundle, so each
//    panel rule must declare box-sizing: border-box itself.
//
//  - KaTeX renders equations into atomic inline-block .base spans with
//    white-space: nowrap; overflow-wrap on an ancestor cannot break inside
//    them, so a long equation needs both a .base override and a horizontal
//    scroll fallback on .eqnPreview to stay within the card.

import * as fs from 'fs';
import * as path from 'path';

function readCss(name: string): string {
  return fs.readFileSync(path.join(__dirname, '..', name), 'utf-8');
}

/** The text of every top-level declaration block for `selector` (ignores
 *  blocks inside media queries only insofar as they also match). */
function blocksFor(css: string, selector: string): string[] {
  const blocks: string[] = [];
  const re = new RegExp(`${selector.replace('.', '\\.')}[^{]*\\{([^}]*)\\}`, 'g');
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    blocks.push(m[1]);
  }
  return blocks;
}

describe('panel box-sizing contracts', () => {
  it('the search bar declares border-box (it has horizontal padding)', () => {
    const css = readCss('Editor.module.css');
    const [main] = blocksFor(css, '.searchBar');
    expect(main).toContain('box-sizing: border-box');
  });

  const cards: Array<[string, string]> = [
    ['VariableDetails.module.css', '.card'],
    ['ModuleDetails.module.css', '.card'],
    ['ErrorDetails.module.css', '.card'],
  ];

  it.each(cards)('%s %s declares border-box to match the search bar', (file, selector) => {
    const css = readCss(file);
    const [main] = blocksFor(css, selector);
    expect(main).toBeDefined();
    expect(main).toContain('box-sizing: border-box');
  });
});

describe('equation preview overflow contracts', () => {
  const css = readCss('VariableDetails.module.css');

  it('.eqnPreview can scroll horizontally as a fallback for unbreakable content', () => {
    const [main] = blocksFor(css, '.eqnPreview');
    expect(main).toContain('overflow-x: auto');
  });

  it('.eqnPreview flex children may shrink below content size', () => {
    // A flex item defaults to min-width: auto and refuses to shrink below
    // its content, pushing wide KaTeX output past the card edge.
    const blocks = blocksFor(css, '.eqnPreview > \\*');
    expect(blocks.length).toBeGreaterThan(0);
    expect(blocks[0]).toContain('min-width: 0');
  });

  it("overrides KaTeX's nowrap .base spans so long equations can wrap", () => {
    expect(css).toMatch(/\.eqnPreview\s+:global\(\.katex\s+\.base\)\s*\{[^}]*white-space:\s*normal/);
  });
});
