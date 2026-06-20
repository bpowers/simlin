// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Design-token consolidation contracts for the app shell (issue #799). Asserts
// stylesheet text directly: the dense-toolbar spacer and the caption variant
// must derive from the shared theme.css tokens rather than restating literals,
// so the spacer can't drift from the dense Toolbar height and caption text
// scales with the rem type scale.

import * as fs from 'fs';
import * as path from 'path';

function readCss(name: string): string {
  return fs.readFileSync(path.join(__dirname, '..', name), 'utf-8');
}

describe('app shell design-token coupling', () => {
  it('the toolbar spacer height comes from --toolbar-dense-height', () => {
    const css = readCss('Home.module.css');
    const m = /\.toolbarSpacer\s*\{([^}]*)\}/.exec(css);
    expect(m).not.toBeNull();
    expect(m![1]).toContain('height: var(--toolbar-dense-height)');
  });

  it('the caption typography variant sizes from --font-size-caption', () => {
    const css = readCss('typography.module.css');
    const m = /\.caption\s*\{([^}]*)\}/.exec(css);
    expect(m).not.toBeNull();
    expect(m![1]).toContain('font-size: var(--font-size-caption)');
  });
});
