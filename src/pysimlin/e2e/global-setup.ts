// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { ENV, launchJupyterLab, type LaunchedServer } from './jupyter-server';

// Playwright runs global setup once in the runner process; values written
// to process.env here are inherited by every worker, which is how the spec
// learns the server's URL, token, and root directory.  The handle itself
// cannot cross the process boundary, so it is parked on globalThis for the
// teardown that runs in this same process.
declare global {
  var __simlinJupyter: LaunchedServer | undefined;
}

export default async function globalSetup(): Promise<void> {
  const server = await launchJupyterLab();
  globalThis.__simlinJupyter = server;
  process.env[ENV.url] = server.url;
  process.env[ENV.token] = server.token;
  process.env[ENV.rootDir] = server.rootDir;
}
