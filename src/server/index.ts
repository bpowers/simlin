// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as logger from './logger';

import { createApp } from './app';

process.on('unhandledRejection', (reason, p) => {
  logger.error(`Unhandled Rejection at: Promise ${p}: ${reason}`);
  console.log(p);
  console.log(reason);
});

async function main(): Promise<void> {
  const app = await createApp();
  app.listen();
}

// A failed boot (e.g. ServerInitError when the engine WASM is missing)
// must take the process down: logging alone would leave a zombie that
// never binds the port, and on GAE such an instance hangs until the
// port-bind timeout instead of recycling immediately with a clear error
// in the logs.
setImmediate(() => {
  main().catch((err: unknown) => {
    const detail = err instanceof Error ? (err.stack ?? err.message) : String(err);
    logger.error(`server startup failed, exiting: ${detail}`);
    process.exit(1);
  });
});
