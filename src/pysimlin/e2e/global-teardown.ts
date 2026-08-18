// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'node:fs';
import * as path from 'node:path';

export default async function globalTeardown(): Promise<void> {
  const server = globalThis.__simlinJupyter;
  if (server === undefined) {
    return;
  }
  // The server log is the first thing to read when the journey fails
  // (kernel start, comm, or extension trouble shows up there, not in the
  // browser), so it outlives the temporary tree.
  const outputDir = path.join(__dirname, '.output');
  fs.mkdirSync(outputDir, { recursive: true });
  try {
    fs.copyFileSync(server.logPath, path.join(outputDir, 'jupyter-lab.log'));
  } catch {
    // Nothing to keep.
  }
  await server.stop();
}
