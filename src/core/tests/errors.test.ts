// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import { ErrorCode as EngineErrorCode, errorCodeDescription as engineErrorCodeDescription } from '@simlin/engine';

import { ErrorCode, errorCodeDescription } from '../errors';

// `errors.ts` is a copy of the engine package's table: core imports only types
// from `@simlin/engine`, so that using core loads none of the engine. The
// engine's own tests hold its copy to the codes libsimlin names; this holds
// the two copies to each other.
describe('ErrorCode', () => {
  it("has the engine package's codes, number for number", () => {
    expect(Object.entries(ErrorCode)).toEqual(Object.entries(EngineErrorCode));
  });

  it("describes every code as the engine package's table does", () => {
    const codes = Object.values(ErrorCode).filter((value): value is ErrorCode => typeof value === 'number');
    expect(codes.length).toBeGreaterThan(0);
    // One past the last code too: the fallback for a code neither table knows.
    for (const code of [...codes, codes.length as ErrorCode]) {
      expect(errorCodeDescription(code)).toBe(engineErrorCodeDescription(code as unknown as EngineErrorCode));
    }
  });
});
