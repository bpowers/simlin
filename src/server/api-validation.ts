// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure request-body validators with no I/O and no heavy dependencies (no
// express, DB, protobuf, or engine WASM), so they can be unit-tested in
// isolation without pulling the server's render/DB surface into the test graph.

// body-parser 2 (paired with Express 5) leaves `req.body` undefined for
// empty-body or non-matching-Content-Type requests, where body-parser 1 left
// `{}`. These validators guard that case so a malformed request gets the
// intended 400 instead of throwing a TypeError that Express 5 surfaces as a
// generic 500 (issue #691). Each returns the client-facing error message, or
// undefined when the body is acceptable.

export function validateCreateProjectBody(body: unknown): string | undefined {
  if (typeof body !== 'object' || body === null || !(body as Record<string, unknown>).projectName) {
    return 'projectName is required';
  }
  return undefined;
}

export function validateSaveProjectBody(body: unknown): string | undefined {
  if (typeof body !== 'object' || body === null) {
    return 'currVersion is required';
  }
  const fields = body as Record<string, unknown>;
  if (fields.currVersion === undefined) {
    return 'currVersion is required';
  }
  // currVersion is the optimistic-concurrency token; the server increments
  // it by 1 on each save, so integers are the invariant to enforce. 0 is a
  // legitimate value -- rows created before versions were seeded at 1 carry
  // proto3's implicit default -- which is why presence is checked by type
  // rather than truthiness (a falsy check bricked saves of those rows).
  if (typeof fields.currVersion !== 'number' || !Number.isInteger(fields.currVersion)) {
    return 'currVersion must be an integer';
  }
  if (typeof fields.projectPB !== 'string' || fields.projectPB === '') {
    return 'projectPB is required';
  }
  return undefined;
}

export function validateUserPatchBody(body: unknown): string | undefined {
  // A user PATCH may only carry two fields. This validator enforces "exactly two
  // keys, one of which is a truthy `username`"; the handler separately requires
  // the second key to be `agreeToTermsAndPrivacyPolicy`.
  if (typeof body !== 'object' || body === null) {
    return 'only username can be patched';
  }
  const fields = body as Record<string, unknown>;
  if (Object.keys(fields).length !== 2 || !fields.username) {
    return 'only username can be patched';
  }
  return undefined;
}
