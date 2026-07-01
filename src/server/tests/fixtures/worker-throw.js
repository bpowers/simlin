// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Test fixture standing in for render-worker.js: dies with an uncaught
// exception, which surfaces on the main thread as the Worker 'error' event.
'use strict';
throw new Error('worker exploded');
