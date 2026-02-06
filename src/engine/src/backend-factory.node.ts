// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Node.js backend factory: returns a DirectBackend.
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
