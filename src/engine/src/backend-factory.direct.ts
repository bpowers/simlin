// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Direct (main-thread) backend factory: returns a DirectBackend that calls
 * into libsimlin on the calling thread. Selected for Node, for the test
 * suites of every TypeScript package, and for bundles that cannot spawn a
 * Web Worker from a second file -- the notebook widget, whose single-file
 * module is loaded from a blob: URL (see src/notebook-widget). Browser SPAs
 * select backend-factory.browser.ts (Web Worker) instead.
 */

import { EngineBackend } from './backend';
import { DirectBackend } from './direct-backend';

let sharedBackend: EngineBackend | null = null;

export function getBackend(): EngineBackend {
  if (!sharedBackend) {
    sharedBackend = new DirectBackend();
  }
  return sharedBackend;
}

export function resetBackend(): void {
  sharedBackend = null;
}
