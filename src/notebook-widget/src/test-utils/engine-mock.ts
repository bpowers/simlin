// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Stand-in for @simlin/engine's ready() in the shell tests: records what it
// was handed instead of instantiating libsimlin.

export const readyCalls: unknown[] = [];

export async function ready(source?: unknown): Promise<void> {
  readyCalls.push(source);
}

export function resetEngineMock(): void {
  readyCalls.length = 0;
}
