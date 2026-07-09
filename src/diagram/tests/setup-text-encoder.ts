// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { TextEncoder, TextDecoder } from 'node:util';

// jsdom implements neither TextEncoder nor TextDecoder, yet the engine's memory
// module constructs both at import time (src/engine/src/internal/memory.ts), so
// any test that transitively imports the engine needs them beforehand.
//
// This has to be a setup file rather than a statement in the test that needs it:
// a module's imports are evaluated before its own top-level statements run, so a
// polyfill sitting above the engine import would land too late. (It read as
// load-bearing under jest, whose CommonJS emit preserved source order.) Rstest's
// jsdom environment happens to leave Node's globals in place, which is the only
// reason the misplaced version kept working -- don't rely on that.
Object.assign(globalThis, { TextEncoder, TextDecoder });
