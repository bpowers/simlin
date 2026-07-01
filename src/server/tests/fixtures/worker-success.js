// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Test fixture standing in for render-worker.js: echoes the projectContents
// bytes back as the "png" so tests can verify the payload round-trips through
// workerData and the result message without needing the WASM engine.
'use strict';
const { parentPort, workerData } = require('worker_threads');

parentPort.postMessage({ ok: true, png: workerData.projectContents });
