// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { afterEach, rs } from '@rstest/core';
import { cleanup } from '@testing-library/react';

// @testing-library/react self-registers this only when `afterEach` happens to be
// a global (see its index.js). Rstest keeps test globals off, so wire it up here
// or every render() leaks its container and later queries match across tests.
afterEach(cleanup);

// @testing-library/dom's waitFor pumps fake timers by reaching for a global
// `jest` (helpers.js gates on `typeof jest !== 'undefined'`, then wait-for.js
// calls `jest.advanceTimersByTime`). Rstest's fake timers are sinon-backed --
// the same implementation jest's modern timers use, and they set the `clock`
// marker waitFor looks for -- but there is no `jest` global, so waitFor would
// silently take its real-timer path and poll with a `setInterval` that never
// fires. Expose the single method it calls. Inert under real timers: the marker
// check downstream still fails, so waitFor polls for real as it should.
(globalThis as { jest?: { advanceTimersByTime: (ms: number) => void } }).jest = {
  advanceTimersByTime: (ms: number) => {
    rs.advanceTimersByTime(ms);
  },
};
