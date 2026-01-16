// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as logger from 'winston';

import { createApp } from './app.js';

process.on('unhandledRejection', (reason, p) => {
  logger.error(`Unhandled Rejection at: Promise ${p}: ${reason}`);
  console.log(p);
  console.log(reason);
});

async function main(): Promise<void> {
  if (process.env.NODE_ENV === 'production') {
    const traceAgent = await import('@google-cloud/trace-agent');
    traceAgent.start();
  }

  const app = await createApp();
  app.listen();
}

setImmediate(main);
