// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

if (process.env.NODE_ENV === 'production') {
  require('@google-cloud/trace-agent').start();
}

import * as logger from 'winston';

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

setImmediate(main);
