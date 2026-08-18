// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { afterEach, rs } from '@rstest/core';
import { cleanup } from '@testing-library/react';

// @testing-library/react self-registers this only when `afterEach` is a
// global; rstest keeps test globals off, so wire it up here or every render()
// leaks its container into the next test.
afterEach(cleanup);

// @testing-library/dom's waitFor advances fake timers only through a global
// `jest` object (see src/simlin-serve/web/src/test-utils for the full story).
(globalThis as { jest?: { advanceTimersByTime: (ms: number) => void } }).jest = {
  advanceTimersByTime: (ms: number) => {
    rs.advanceTimersByTime(ms);
  },
};
