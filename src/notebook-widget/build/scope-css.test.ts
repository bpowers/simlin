// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import * as fs from 'node:fs';
import * as path from 'node:path';
import { createRequire } from 'node:module';

import postcss from 'postcss';
import type { AtRule, Rule } from 'postcss';

import { scopeCssPlugin, scopeSelector } from './scope-css';

const ROOT = 'simlin-notebook-widget';
const here = import.meta.dirname;
const require = createRequire(import.meta.url);

describe('scopeSelector', () => {
  // Every arm of the rewrite, including the ones the two real stylesheets do
  // not exercise today, so a future stylesheet cannot slip a page-wide rule
  // through an untested branch.
  it.each([
    [':root', `.${ROOT}`],
    [':host', `.${ROOT}`],
    ['html', `.${ROOT}`],
    ['body', `.${ROOT}`],
    [':root[data-theme="dark"]', `.${ROOT}[data-theme="dark"]`],
    [':host([data-theme="dark"])', `.${ROOT}[data-theme="dark"]`],
    [':host(.x) .y', `.${ROOT}.x .y`],
    ['html:focus-within', `.${ROOT}:focus-within`],
    ['body.dark .foo', `.${ROOT}.dark .foo`],
    ['body > .foo', `.${ROOT} > .foo`],
    ['*', `:where(.${ROOT}) *`],
    ['*::before', `:where(.${ROOT}) *::before`],
    ['.katex .foo', `:where(.${ROOT}) .katex .foo`],
    ['[data-x]', `:where(.${ROOT}) [data-x]`],
    ['bodyguard', `:where(.${ROOT}) bodyguard`],
    ['htmlfoo .x', `:where(.${ROOT}) htmlfoo .x`],
    [`.${ROOT}`, `.${ROOT}`],
    [`.${ROOT} .already`, `.${ROOT} .already`],
    [`:where(.${ROOT}) .already`, `:where(.${ROOT}) .already`],
    [`.${ROOT}[data-theme="dark"]`, `.${ROOT}[data-theme="dark"]`],
    ['  .padded  ', `:where(.${ROOT}) .padded`],
  ])('%s -> %s', (input, expected) => {
    expect(scopeSelector(input, ROOT)).toBe(expected);
  });
});

async function scoped(css: string): Promise<postcss.Root> {
  const result = await postcss([scopeCssPlugin(ROOT)]).process(css, { from: undefined });
  return result.root;
}

function topLevelSelectors(root: postcss.Root): string[] {
  const out: string[] = [];
  root.walkRules((rule: Rule) => {
    let parent = rule.parent as AtRule | undefined;
    let nameAddressed = false;
    while (parent && parent.type === 'atrule') {
      if (['font-face', 'keyframes'].includes(parent.name)) {
        nameAddressed = true;
      }
      parent = parent.parent as AtRule | undefined;
    }
    if (!nameAddressed) {
      out.push(...rule.selectors);
    }
  });
  return out;
}

describe('scopeCssPlugin on the stylesheets the widget bundles', () => {
  it('leaves no page-wide selector in @simlin/diagram theme.css', async () => {
    const themeCss = fs.readFileSync(path.join(here, '..', '..', 'diagram', 'theme.css'), 'utf8');
    // Sanity: the input really is page-wide (otherwise this test proves nothing).
    expect(themeCss).toMatch(/^:root,\s*:host\s*\{/m);
    expect(themeCss).toMatch(/^\s*\*,\s*$/m);
    const root = await scoped(themeCss);
    const selectors = topLevelSelectors(root);
    expect(selectors.length).toBeGreaterThan(3);
    for (const s of selectors) {
      expect(s.startsWith(`.${ROOT}`) || s.startsWith(`:where(.${ROOT}) `)).toBe(true);
    }
    const out = root.toString();
    expect(out).not.toMatch(/(^|[,\s{}]):root/);
    expect(out).not.toMatch(/(^|[,\s{}]):host/);
    // The reduced-motion universal rule is now the widget's own subtree.
    expect(out).toMatch(
      new RegExp(
        `:where\\(\\.${ROOT}\\) \\*,\\s*:where\\(\\.${ROOT}\\) \\*::before,\\s*:where\\(\\.${ROOT}\\) \\*::after`,
      ),
    );
    // The dark-theme token block keys off the wrapper's data-theme.
    expect(out).toContain(`.${ROOT}[data-theme="dark"]`);
  });

  it('leaves no page-wide selector in katex.min.css and keeps its @font-face intact', async () => {
    const katexCss = fs.readFileSync(require.resolve('katex/dist/katex.min.css'), 'utf8');
    expect(katexCss).toContain('body{counter-reset:');
    const root = await scoped(katexCss);
    for (const s of topLevelSelectors(root)) {
      expect(s.startsWith(`.${ROOT}`) || s.startsWith(`:where(.${ROOT}) `)).toBe(true);
    }
    const out = root.toString();
    expect(out).toContain(`.${ROOT}{counter-reset:`);
    let fontFaces = 0;
    root.walkAtRules('font-face', () => {
      fontFaces += 1;
    });
    expect(fontFaces).toBeGreaterThan(10);
    expect(out).toContain('font-family:KaTeX_Main');
  });

  it('skips CSS Modules (their classes are hashed already)', async () => {
    const result = await postcss([scopeCssPlugin(ROOT)]).process(':root { --a: 1 } .host { b: 2 }', {
      from: '/x/widget.module.css',
    });
    expect(result.root.toString()).toBe(':root { --a: 1 } .host { b: 2 }');
  });

  it('descendant prefixes add zero specificity (:where), page-root rewrites keep class specificity', async () => {
    // Specificity is what decides the cascade against the Editor's own
    // CSS-Module rules; a plain `.root .katex .foo` prefix would have raised
    // every scoped rule by one class and let theme/katex rules beat module
    // rules they used to lose to.
    const out = (await scoped('.katex .foo { a: 1 } * { b: 2 } :root { --c: 3 } body.dark .x { d: 4 }')).toString();
    expect(out).toContain(`:where(.${ROOT}) .katex .foo`);
    expect(out).toContain(`:where(.${ROOT}) *`);
    expect(out).toContain(`.${ROOT} { --c: 3 }`);
    expect(out).toContain(`.${ROOT}.dark .x`);
    // No descendant prefix without :where.
    expect(out).not.toMatch(new RegExp(`(^|[,{}\\s])\\.${ROOT} [^{]`));
  });

  it('fails closed on a rule inside an at-rule it does not know', async () => {
    await expect(scoped('@scope (.a) { .b { c: 1 } }')).rejects.toThrow(/unknown at-rule @scope/);
    await expect(scoped('@-moz-document url(x) { .b { c: 1 } }')).rejects.toThrow(/unknown at-rule/);
  });

  it('descends into every conditional at-rule and skips every name-addressed one', async () => {
    const out = (
      await scoped(
        '@media (x) { .a { b: 1 } } @supports (x: y) { .c { d: 1 } } @layer l { .e { f: 1 } } @container (min-width: 1px) { .g { h: 1 } } ' +
          '@font-face { font-family: F } @keyframes k { from { a: 1 } } @counter-style s { system: cyclic } @property --p { syntax: "*" } @page { margin: 0 }',
      )
    ).toString();
    for (const cls of ['.a', '.c', '.e', '.g']) {
      expect(out).toContain(`:where(.${ROOT}) ${cls}`);
    }
    expect(out).toContain('from');
    expect(out).not.toContain(`${ROOT}) from`);
    expect(out).toContain('@font-face { font-family: F }');
  });

  it('dedupes selectors that collapse onto the root', async () => {
    const out = (await scoped(':root, :host { --a: 1 }')).toString();
    expect(out).toBe(`.${ROOT} { --a: 1 }`);
  });

  it('is idempotent', async () => {
    const once = (await scoped(':root { --a: 1 } * { x: y } body .k { z: 1 }')).toString();
    const twice = (await scoped(once)).toString();
    expect(twice).toBe(once);
  });

  it('does not touch rules nested in @keyframes', async () => {
    const out = (await scoped('@keyframes spin { from { a: 1 } to { a: 2 } } @media (x) { .a { b: 1 } }')).toString();
    expect(out).toContain('from');
    expect(out).not.toContain(`.${ROOT} from`);
    expect(out).toContain(`:where(.${ROOT}) .a`);
  });
});
