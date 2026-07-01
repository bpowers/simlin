// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Test fixture standing in for render-worker.js: never responds. The interval
// keeps the thread alive indefinitely so only worker.terminate() -- the
// timeout path under test -- can end it.
'use strict';
setInterval(() => {}, 1000);
