// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

describe('Panel width constants sync', () => {
  it('TypeScript constants match CSS variables', () => {
    const themeCss = fs.readFileSync(path.join(__dirname, '../theme.css'), 'utf-8');

    const smMatch = themeCss.match(/--panel-width-sm:\s*(\d+)px/);
    const lgMatch = themeCss.match(/--panel-width-lg:\s*(\d+)px/);

    expect(smMatch).not.toBeNull();
    expect(lgMatch).not.toBeNull();

    const cssWidthSm = parseInt(smMatch![1], 10);
    const cssWidthLg = parseInt(lgMatch![1], 10);

    const editorTs = fs.readFileSync(path.join(__dirname, '../Editor.tsx'), 'utf-8');

    const tsSmMatch = editorTs.match(/SearchbarWidthSm\s*=\s*(\d+)/);
    const tsLgMatch = editorTs.match(/SearchbarWidthLg\s*=\s*(\d+)/);

    expect(tsSmMatch).not.toBeNull();
    expect(tsLgMatch).not.toBeNull();

    const tsWidthSm = parseInt(tsSmMatch![1], 10);
    const tsWidthLg = parseInt(tsLgMatch![1], 10);

    expect(tsWidthSm).toBe(cssWidthSm);
    expect(tsWidthLg).toBe(cssWidthLg);
  });
});
