// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function validateRuntimeConfig(nodeEnv: string | undefined, authentication: unknown): void {
  if (nodeEnv !== 'production') {
    return;
  }

  if (!isRecord(authentication) || !isRecord(authentication.seshcookie)) {
    throw new Error('production config must define authentication.seshcookie');
  }

  const key = authentication.seshcookie.key;
  if (typeof key !== 'string' || key.trim() === '' || key === 'IN ENV') {
    throw new Error('production authentication.seshcookie.key must be set from the environment');
  }
}
