// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Stand-in for the bundler-instantiated `core/libsimlin-browser.wasm` module
// namespace that wasm.browser.ts imports (see rstest.config.mts). Under a
// bundler that import evaluates to the module's exports object; here it is a
// recognisable fake so tests can tell "adopted the bundled artifact" from
// "instantiated the caller's source".
export const memory = new WebAssembly.Memory({ initial: 1 });
export const simlin_init = (): void => {
  initCalls.push('bundled');
};
export const bundled_marker = (): number => 42;
export const initCalls: string[] = [];
