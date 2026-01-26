// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as logger from 'winston';
import { ready, isReady } from '@system-dynamics/engine2';

/**
 * Error thrown when server initialization fails.
 */
export class ServerInitError extends Error {
  constructor(
    message: string,
    public readonly cause?: Error,
  ) {
    super(message);
    this.name = 'ServerInitError';
  }
}

/**
 * Initialize server dependencies, including the WASM simulation engine.
 *
 * This should be called early in server startup to fail fast with clear
 * error messages if required dependencies are missing or misconfigured.
 *
 * @throws {ServerInitError} If WASM initialization fails
 */
export async function initializeServerDependencies(): Promise<void> {
  if (isReady()) {
    logger.info('WASM already initialized');
    return;
  }

  try {
    await ready();
    logger.info('WASM module initialized successfully');
  } catch (e) {
    const err = e as Error;
    const isFileNotFound = err.message.includes('ENOENT') || err.message.includes('not found');
    const hint = isFileNotFound ? ' Ensure the engine2 package is built (yarn build in src/engine2).' : '';
    throw new ServerInitError(`Server startup failed: Unable to initialize WASM module. ${err.message}${hint}`, err);
  }
}
