// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as logger from 'winston';

import { App } from './app';

const app = new App().app;
const port = app.get('port');

const server = app.listen(port);

process.on('unhandledRejection', (reason, p) => {
  logger.error(`Unhandled Rejection at: Promise ${p}: ${reason}`);
  console.log(p);
  console.log(reason);
});

server.on('listening', () => {
  logger.info(`model-service started on http://${app.get('host')}:${port}`);
});
