// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

if (process.env.NODE_ENV === 'production') {
  // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-var-requires
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

// eslint-disable-next-line @typescript-eslint/no-misused-promises
setImmediate(main);
