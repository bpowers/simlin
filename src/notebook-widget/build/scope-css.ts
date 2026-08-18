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
 *   :root, :host, html, body   ->  .simlin-notebook-widget      (the root IS the page)
 *   :root[data-theme=dark]     ->  .simlin-notebook-widget[data-theme=dark]
 *   *, .katex .foo, [attr]     ->  :where(.simlin-notebook-widget) *, ...
 *
 * Descendant prefixes use `:where(...)`, which contributes ZERO specificity,
 * so the scoped rules keep exactly the specificity their authors gave them
 * and the cascade against the Editor's own CSS-Module rules is unchanged. The
 * page-root rewrite deliberately keeps its class specificity: `:root` and
 * `body` were (0,1,0)/(0,0,1) and now are (0,1,0), and nothing else in the
 * bundle targets the wrapper by type or pseudo-class.
 *
 * `@font-face`, `@keyframes` and friends have no selectors and are left
 * alone. Conditional at-rules (`@media`, `@supports`, `@layer`,
 * `@container`) are descended into. Any OTHER at-rule containing rules fails
 * the build: an unknown grouping construct is exactly where a page-wide rule
 * could slip through unscoped, so the plugin fails closed rather than
 * guessing. CSS Modules (`*.module.css`) are skipped: their classes are hashed.
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
  const where = `:where(${root})`;
  const trimmed = selector.trim();
  if (trimmed === '') {
    return trimmed;
  }
  // Already scoped (e.g. running twice) -- leave it.
  if (
    trimmed === root ||
    trimmed.startsWith(`${root}[`) ||
    trimmed.startsWith(`${root}.`) ||
    trimmed.startsWith(`${root}:`) ||
    trimmed.startsWith(`${root} `) ||
    trimmed.startsWith(`${where} `)
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
  return `${where} ${trimmed}`;
}

/** At-rules whose bodies hold rules with selectors we descend into. */
const CONDITIONAL_AT_RULES = new Set(['media', 'supports', 'layer', 'container']);
/** At-rules that hold no selectors and are addressed by name. */
const NAME_ADDRESSED_AT_RULES = new Set([
  'font-face',
  'keyframes',
  'counter-style',
  'property',
  'page',
  'font-feature-values',
]);

/**
 * `true` when the rule is inside a name-addressed at-rule (leave it alone);
 * throws when it is inside an at-rule this plugin does not know (fail closed).
 */
function insideNameAddressedAtRule(rule: Rule): boolean {
  let parent = rule.parent as AtRule | undefined;
  while (parent && parent.type === 'atrule') {
    const name = parent.name.replace(/^-\w+-/, '');
    if (NAME_ADDRESSED_AT_RULES.has(name)) {
      return true;
    }
    if (!CONDITIONAL_AT_RULES.has(name)) {
      throw rule.error(
        `simlin-scope-css: rule inside unknown at-rule @${parent.name}; ` +
          'add it to CONDITIONAL_AT_RULES (descend) or NAME_ADDRESSED_AT_RULES (skip) in build/scope-css.ts',
      );
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
