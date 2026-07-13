// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Shared shape for the global fetch stub that setup-fetch.ts installs.
//
// Why a setup file rather than an assignment at the top of app.test.tsx: a
// module's imports are fully evaluated before its own top-level statements run.
// App.tsx constructs UserInfoSingleton at module scope and its constructor
// fetches /api/user immediately, so by the time a `globalThis.fetch = ...`
// statement in the test file executed, App would already have called the real
// fetch. (This used to work: TypeScript's CommonJS emit kept `require()` calls
// in source order, so the assignment genuinely preceded `require('../App')`.
// Rspack hoists ES imports above the module body, as the ES spec requires.)

export type FetchImpl = (input: unknown, init?: { method?: string }) => Promise<Response>;

/** A minimal Response: the app only ever reads `status` and `json()`. */
export function userResponse(status: number, body: unknown): Response {
  return {
    status,
    async json() {
      return body;
    },
  } as unknown as Response;
}

/**
 * A plain-text response, e.g. express's res.sendStatus(500) body of
 * "Internal Server Error": json() rejects with a SyntaxError exactly as
 * real fetch does on a non-JSON body (#927).
 */
export function textResponse(status: number, body: string): Response {
  return {
    status,
    async json() {
      throw new SyntaxError(`Unexpected token 'I', "${body}" is not valid JSON`);
    },
  } as unknown as Response;
}
