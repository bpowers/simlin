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

// Resolve a `--token: <px-only calc/var expr>;` chain from theme.css to a
// number of pixels. The shared-model chrome tokens are all linear px sums of
// other tokens, so a substitute-then-evaluate pass is sufficient (and keeps the
// test honest: if a future edit introduces a non-px term it throws instead of
// silently passing). This is the only place the test "does layout"; jsdom can't.
function resolvePxToken(css: string, token: string, seen: Set<string> = new Set()): number {
  if (seen.has(token)) {
    throw new Error(`cycle resolving ${token}`);
  }
  seen.add(token);
  const m = new RegExp(`${token}\\s*:\\s*([^;]+);`).exec(css);
  if (!m) {
    throw new Error(`token ${token} not found`);
  }
  let expr = m[1].replace(/calc\(/g, '(');
  for (let v = /var\((--[a-z0-9-]+)\)/.exec(expr); v !== null; v = /var\((--[a-z0-9-]+)\)/.exec(expr)) {
    const resolved = resolvePxToken(css, v[1], new Set(seen));
    expr = expr.replace(v[0], `${resolved}px`);
  }
  // The shared chrome tokens are purely additive px sums (only + and -, no *
  // or /), so parentheses don't affect associativity and can be flattened.
  // Anything else (a multiplicative term, a non-px unit) means the resolver's
  // assumption no longer holds, so reject it rather than silently mis-summing.
  if (!/^[\s0-9.+\-()px]+$/.test(expr) || /[*/]/.test(expr)) {
    throw new Error(`unexpected term while resolving ${token}: ${expr}`);
  }
  let total = 0;
  for (const term of expr.replace(/[()]/g, '').match(/[+-]?\s*[0-9.]+px/g) ?? []) {
    total += parseFloat(term.replace(/\s+/g, '').replace('px', ''));
  }
  return total;
}

describe('shared-model banner inset contract', () => {
  // When the shared-model banner is shown it overlays the top of the same
  // top-right slot the detail panels occupy. The Editor applies
  // .varDetailsWithBanner so the panel reserves extra top room and its content
  // clears BOTH the search bar and the banner (issue #797). These assert the
  // CSS half of that contract; the Editor wiring picks the class.
  const editorCss = readCss('Editor.module.css');
  const themeCss = readCss('theme.css');

  it('.varDetailsWithBanner rebinds --panel-top-inset to the with-banner token', () => {
    const [block] = blocksFor(editorCss, '.varDetailsWithBanner');
    expect(block).toBeDefined();
    expect(block.replace(/\s+/g, ' ')).toContain('--panel-top-inset: var(--panel-top-inset-with-banner)');
  });

  it('the banner top derives from the shared chrome token (no literal offset)', () => {
    const [block] = blocksFor(editorCss, '.sharedModelBanner');
    expect(block).toBeDefined();
    expect(block).toContain('top: var(--shared-model-banner-top)');
  });

  it('the with-banner inset clears the banner: it exceeds the base inset', () => {
    const base = resolvePxToken(themeCss, '--panel-top-inset');
    const withBanner = resolvePxToken(themeCss, '--panel-top-inset-with-banner');
    const bannerTop = resolvePxToken(themeCss, '--shared-model-banner-top');
    const bannerHeight = resolvePxToken(themeCss, '--shared-model-banner-height');

    // Panels and the banner share top:8px. Panel content begins at
    // 8 + inset; it must clear the banner's bottom (bannerTop + bannerHeight).
    expect(8 + withBanner).toBeGreaterThanOrEqual(bannerTop + bannerHeight);
    // And the banner-aware band must be strictly larger than the base band,
    // otherwise the modifier would be a no-op and the overlap would persist.
    expect(withBanner).toBeGreaterThan(base);
  });

  it('the search bar height is the --searchbar-height token (one source of truth)', () => {
    // --searchbar-height feeds the banner top and both panel insets; if the bar
    // itself hardcoded a different height the derived offsets would silently lie.
    const [bar] = blocksFor(editorCss, '.searchBar');
    expect(bar).toContain('height: var(--searchbar-height)');
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
