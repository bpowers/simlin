// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Test fixture standing in for render-worker.js: reports a render failure the
// way the real worker does when the engine rejects a model.
'use strict';
const { parentPort } = require('worker_threads');

parentPort.postMessage({ ok: false, error: 'boom: intentional render failure' });
