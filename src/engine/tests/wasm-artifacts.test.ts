// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Guards the dual-artifact WASM build contract (see build.sh):
 *
 * - `core/libsimlin.wasm` is the full build Node loads (the server's PNG
 *   preview rendering depends on `simlin_project_render_png`).
 * - `core/libsimlin-browser.wasm` is the slim build bundlers ship to the
 *   browser; it must NOT carry the PNG render stack (resvg + text shaping
 *   + an embedded font, ~28% of the full binary).
 *
 * A regression here is silent: the browser bundle still works, it just
 * ships megabytes of dead rasterization code to every visitor.
 */

import * as fs from 'fs';
import * as path from 'path';

const CORE_DIR = path.join(__dirname, '..', 'core');

async function exportNames(wasmPath: string): Promise<Set<string>> {
  const buf = fs.readFileSync(wasmPath);
  const module = await WebAssembly.compile(
    buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength) as ArrayBuffer,
  );
  return new Set(WebAssembly.Module.exports(module).map((e) => e.name));
}

describe('wasm build artifacts', () => {
  it('full (node) artifact exports PNG rendering', async () => {
    const exports = await exportNames(path.join(CORE_DIR, 'libsimlin.wasm'));
    expect(exports.has('simlin_project_render_png')).toBe(true);
    expect(exports.has('simlin_project_render_svg')).toBe(true);
    expect(exports.has('simlin_sim_new')).toBe(true);
  });

  it('browser artifact omits PNG rendering but keeps the rest of the API', async () => {
    const browserWasm = path.join(CORE_DIR, 'libsimlin-browser.wasm');
    expect(fs.existsSync(browserWasm)).toBe(true);

    const exports = await exportNames(browserWasm);
    expect(exports.has('simlin_project_render_png')).toBe(false);
    // SVG rendering is pure string generation (no resvg) and stays available.
    expect(exports.has('simlin_project_render_svg')).toBe(true);
    expect(exports.has('simlin_sim_new')).toBe(true);
    expect(exports.has('simlin_project_open_protobuf')).toBe(true);
    expect(exports.has('simlin_model_compile_to_wasm')).toBe(true);
  });

  it('browser artifact is meaningfully smaller than the full artifact', () => {
    const full = fs.statSync(path.join(CORE_DIR, 'libsimlin.wasm')).size;
    const slim = fs.statSync(path.join(CORE_DIR, 'libsimlin-browser.wasm')).size;
    // The PNG render stack is ~28% of the full binary; require at least a
    // 15% delta so the test stays robust to unrelated size drift while
    // still catching "both artifacts accidentally built identically".
    expect(slim).toBeLessThan(full * 0.85);
  });
});
