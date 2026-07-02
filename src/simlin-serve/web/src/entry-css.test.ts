// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Entry stylesheet contracts. jsdom can neither bundle nor lay out CSS, so
// (matching @simlin/diagram's theme-tokens tests) these assert the source
// text directly. They exist because a missing global stylesheet is invisible
// to unit tests but very visible to users: when theme tokens went missing
// the whole diagram rendered with black SVG fills.

import * as fs from 'fs';
import * as path from 'path';

function read(...segments: string[]): string {
  return fs.readFileSync(path.join(__dirname, ...segments), 'utf-8');
}

describe('SPA entry loads the stylesheets the diagram needs', () => {
  // @simlin/diagram carries its own reset.css/theme.css via its package
  // root, but it cannot carry katex's stylesheet: the package's Node build
  // stubs only its *own* CSS files (see src/diagram/build-css.sh), so a
  // third-party CSS import in the package would crash Node consumers.
  // Browser hosts therefore import katex's CSS at their entry, exactly as
  // src/app's index.tsx does.
  it('main.tsx imports the katex stylesheet', () => {
    const entry = read('main.tsx');
    expect(entry).toMatch(/^import 'katex\/dist\/katex\.min\.css';$/m);
  });

  // The diagram's canvas, labels, and equation editors all specify Roboto /
  // Roboto Mono. simlin-serve is self-contained (no network deps once
  // launched), so the faces must be bundled rather than fetched from Google
  // Fonts; without them every label silently falls back to Helvetica.
  it('styles.css declares the self-hosted Roboto faces', () => {
    const css = read('styles.css');
    for (const weight of [300, 400, 500]) {
      expect(css).toMatch(new RegExp(`font-family:\\s*'?Roboto'?;[\\s\\S]{0,200}?font-weight:\\s*${weight};`));
    }
    expect(css).toMatch(/font-family:\s*'Roboto Mono';/);
    // Faces must resolve to bundled files (relative url() lets Vite hash
    // them and keep the SPA relocatable under any base path).
    expect(css).toMatch(/src:\s*url\('\.\/fonts\/[^']+\.woff2'\) format\('woff2'\)/);
  });
});
