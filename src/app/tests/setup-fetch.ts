// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { rs } from '@rstest/core';

import { userResponse, type FetchImpl } from './fetch-stub';

// Runs before any test module is loaded, which is the only place early enough
// to be observed by App.tsx's module-scope fetch of /api/user -- see
// fetch-stub.ts. Signed out (401) by default; tests re-point it through
// `globalThis.fetch`, which they read back as a typed Mock.
(globalThis as unknown as { fetch: FetchImpl }).fetch = rs.fn<FetchImpl>(async () => userResponse(401, {}));
