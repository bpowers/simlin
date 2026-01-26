// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { isReady, configureWasm, resetWasm } from '@system-dynamics/engine2';
import { initializeServerDependencies, ServerInitError } from '../server-init';

describe('Server WASM initialization', () => {
  afterEach(() => {
    resetWasm();
  });

  it('should initialize WASM successfully when file exists', async () => {
    await expect(initializeServerDependencies()).resolves.not.toThrow();
    expect(isReady()).toBe(true);
  });

  it('should not reinitialize if already initialized', async () => {
    await initializeServerDependencies();
    expect(isReady()).toBe(true);

    // Second call should be idempotent (no error, no re-init)
    await expect(initializeServerDependencies()).resolves.not.toThrow();
    expect(isReady()).toBe(true);
  });

  it('should throw ServerInitError with clear message on WASM failure', async () => {
    // Configure WASM with an invalid buffer to simulate corruption
    configureWasm({ source: new Uint8Array([0, 0, 0, 0]) });

    await expect(initializeServerDependencies()).rejects.toThrow(ServerInitError);
    await expect(initializeServerDependencies()).rejects.toThrow(/WASM|initialization/i);
  });
});
