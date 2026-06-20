// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Design-token consolidation contracts (issues #799, #709). These assert the
// stylesheet text directly because jsdom can't resolve cascaded custom
// properties or do layout. The point is to keep the component library on the
// theme.css tokens so a single edit (or the dark-mode pass) reaches every
// surface, instead of literals silently creeping back in.

import * as fs from 'fs';
import * as path from 'path';

function readCss(...segments: string[]): string {
  return fs.readFileSync(path.join(__dirname, '..', ...segments), 'utf-8');
}

// A hex color (#abc / #aabbcc / #aabbccff) or an rgb()/rgba() function. The
// component library expresses every color through a var(--color-*) token, so a
// raw literal here means a token was missed.
const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|\brgba?\(/;

describe('component library uses theme tokens, not color literals', () => {
  const componentsDir = path.join(__dirname, '..', 'components');
  const files = fs
    .readdirSync(componentsDir)
    .filter((f) => f.endsWith('.module.css'))
    .sort();

  it('finds component CSS modules to check', () => {
    expect(files.length).toBeGreaterThan(0);
  });

  it.each(files)('%s contains no hardcoded color literals', (file) => {
    const css = fs.readFileSync(path.join(componentsDir, file), 'utf-8');
    const offenders = css
      .split('\n')
      .map((line, i) => [i + 1, line] as const)
      .filter(([, line]) => COLOR_LITERAL.test(line));
    expect(offenders.map(([n, line]) => `${file}:${n} ${line.trim()}`)).toEqual([]);
  });
});

describe('theme.css token contracts', () => {
  const theme = readCss('theme.css');

  function darkBlock(css: string): string {
    const m = /\[data-theme="dark"\]\s*\{([\s\S]*?)\}/.exec(css);
    if (!m) {
      throw new Error('no [data-theme="dark"] block in theme.css');
    }
    return m[1];
  }

  it('defines the dense-toolbar and caption tokens in :root', () => {
    expect(theme).toMatch(/--toolbar-dense-height:\s*48px/);
    expect(theme).toMatch(/--font-size-caption:\s*0\.75rem/);
  });

  it('renames shadows to the elevation scheme (no sm/md/lg, adds 4/8/16/24)', () => {
    for (const level of [1, 2, 3, 4, 8, 16, 24]) {
      expect(theme).toMatch(new RegExp(`--shadow-${level}:`));
    }
    expect(theme).not.toMatch(/--shadow-(sm|md|lg)\b/);
  });

  it('gives the chrome surface/text/border tokens dark-mode values', () => {
    const dark = darkBlock(theme);
    for (const token of [
      '--color-surface',
      '--color-field-bg',
      '--color-background',
      '--color-text-primary',
      '--color-text-muted',
      '--color-text-light',
      '--color-border',
      '--color-divider',
      '--color-action-hover',
    ]) {
      expect(dark).toContain(`${token}:`);
    }
  });
});

describe('dense-toolbar height is a single source of truth', () => {
  it('Toolbar .dense derives its min-height from the token', () => {
    const css = readCss('components', 'Toolbar.module.css');
    const m = /\.dense\s*\{([^}]*)\}/.exec(css);
    expect(m).not.toBeNull();
    expect(m![1]).toContain('min-height: var(--toolbar-dense-height)');
  });
});

// Walk the source CSS (diagram + app), skipping generated output, so these
// contracts hold for every stylesheet rather than a hand-listed subset.
function sourceCssFiles(): string[] {
  const roots = [path.join(__dirname, '..'), path.join(__dirname, '..', '..', 'app')];
  const skip = /(^|\/)(lib|lib\.browser|lib\.module|build|build-component|dist|node_modules)(\/|$)/;
  const out: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (skip.test(full)) {
        continue;
      }
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.name.endsWith('.css')) {
        out.push(full);
      }
    }
  };
  roots.forEach(walk);
  return out;
}

describe('source stylesheets only reference defined tokens', () => {
  const theme = readCss('theme.css');
  const defined = new Set([...theme.matchAll(/^\s*(--[a-z0-9-]+):/gm)].map((m) => m[1]));
  const files = sourceCssFiles();

  it('scans the source tree', () => {
    expect(files.length).toBeGreaterThan(10);
  });

  it('every var(--token) resolves to a definition in theme.css', () => {
    const undefinedRefs: string[] = [];
    for (const file of files) {
      const css = fs.readFileSync(file, 'utf-8');
      for (const m of css.matchAll(/var\((--[a-z0-9-]+)/g)) {
        // --radix-* are injected at runtime by the Radix primitives (e.g. the
        // accordion content height used for the open/close animation), so they
        // are deliberately not theme tokens.
        if (m[1].startsWith('--radix-')) {
          continue;
        }
        if (!defined.has(m[1])) {
          undefinedRefs.push(`${path.basename(file)}: ${m[1]}`);
        }
      }
    }
    expect([...new Set(undefinedRefs)]).toEqual([]);
  });

  it('no source stylesheet uses the retired --shadow-sm/md/lg names', () => {
    const offenders = files.filter((f) => /var\(--shadow-(sm|md|lg)\)/.test(fs.readFileSync(f, 'utf-8')));
    expect(offenders.map((f) => path.basename(f))).toEqual([]);
  });
});

describe('editor chrome text sizes scale with zoom (rem, not raw px)', () => {
  it.each(['Editor.module.css', 'ModuleDetails.module.css'])('%s sizes font in rem', (file) => {
    const css = readCss(file);
    const rawPx = [...css.matchAll(/font-size:\s*[0-9]+px/g)].map((m) => m[0]);
    expect(rawPx).toEqual([]);
  });
});
