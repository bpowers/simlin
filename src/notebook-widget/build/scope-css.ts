// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * PostCSS plugin that confines a GLOBAL stylesheet to the widget's root
 * element. The widget injects its CSS into the notebook page's <head>
 * (there is no shadow root and no separate CSS file), so anything written
 * for a whole page -- theme.css's `:root` design tokens, its universal
 * reduced-motion rule, katex's `body { counter-reset }` -- would otherwise
 * apply to JupyterLab itself, and N displayed widgets would inject N copies
 * of it. Every selector is rewritten to live under the root class:
 *
 *   :root, :host, html, body   ->  .simlin-notebook-widget   (the root IS the page)
 *   :root[data-theme=dark]     ->  .simlin-notebook-widget[data-theme=dark]
 *   *, .katex .foo, [attr]     ->  .simlin-notebook-widget *, .simlin-notebook-widget .katex .foo, ...
 *
 * `@font-face` and `@keyframes` have no selector and are left alone (fonts and
 * animations are addressed by name, which is fine). CSS Modules (`*.module.css`)
 * are skipped: their classes are already hashed per module.
 *
 * Kept a plain-string transform on `rule.selectors` (no selector parser):
 * the two stylesheets it runs on are known quantities, and the test pins
 * that no top-level selector survives without the root class in front.
 */

import type { AtRule, Plugin, Rule } from 'postcss';

/** Selectors that denote "the page itself" and become the root element. */
const PAGE_ROOT_SELECTORS = new Set([':root', ':host', 'html', 'body']);

/**
 * Rewrite one selector so it applies only inside `.${rootClass}`.
 * Exported for the unit test; the plugin applies it to every rule.
 */
export function scopeSelector(selector: string, rootClass: string): string {
  const root = `.${rootClass}`;
  const trimmed = selector.trim();
  if (trimmed === '') {
    return trimmed;
  }
  // Already scoped (e.g. running twice) -- leave it.
  if (
    trimmed === root ||
    trimmed.startsWith(`${root} `) ||
    trimmed.startsWith(`${root}[`) ||
    trimmed.startsWith(`${root}.`) ||
    trimmed.startsWith(`${root}:`)
  ) {
    return trimmed;
  }
  // `:root`, `:host`, `html`, `body` -- alone or with a compound suffix like
  // `:root[data-theme="dark"]`, `:host(.x)`, `body.dark`, `html:focus-within`,
  // or followed by descendants (`body .foo`) -- become the root element.
  for (const page of PAGE_ROOT_SELECTORS) {
    if (trimmed === page) {
      return root;
    }
    if (trimmed.startsWith(page)) {
      const rest = trimmed.slice(page.length);
      // `:host(...)` functional form: the argument is a compound on the host.
      if (page === ':host' && rest.startsWith('(')) {
        const close = rest.indexOf(')');
        if (close > 0) {
          return `${root}${rest.slice(1, close)}${rest.slice(close + 1)}`;
        }
      }
      // Reject false prefixes such as `bodyguard` / `htmlfoo`: the next char
      // must start a compound suffix or a combinator.
      const next = rest[0];
      if (next === undefined || /[\s.:#[>+~]/.test(next)) {
        return `${root}${rest}`;
      }
    }
  }
  return `${root} ${trimmed}`;
}

/** At-rules whose bodies hold rules with selectors we must scope. */
const RULE_BEARING_AT_RULES = new Set(['media', 'supports', 'layer', 'container']);
/** At-rules that hold no selectors and are addressed by name. */
const NAME_ADDRESSED_AT_RULES = new Set(['font-face', 'keyframes', 'counter-style', 'property', 'import', 'charset']);

function insideNameAddressedAtRule(rule: Rule): boolean {
  let parent = rule.parent as AtRule | undefined;
  while (parent && parent.type === 'atrule') {
    if (NAME_ADDRESSED_AT_RULES.has(parent.name.replace(/^-\w+-/, ''))) {
      return true;
    }
    if (!RULE_BEARING_AT_RULES.has(parent.name)) {
      return true;
    }
    parent = parent.parent as AtRule | undefined;
  }
  return false;
}

export function scopeCssPlugin(rootClass: string): Plugin {
  return {
    postcssPlugin: 'simlin-scope-css',
    Rule(rule) {
      const file = rule.root().source?.input.file;
      if (file !== undefined && file.endsWith('.module.css')) {
        return;
      }
      if (insideNameAddressedAtRule(rule)) {
        return;
      }
      // `:root, :host` both become the root class: dedupe so the output is
      // not `.x, .x`.
      rule.selectors = [...new Set(rule.selectors.map((s) => scopeSelector(s, rootClass)))];
    },
  };
}
scopeCssPlugin.postcss = true;
