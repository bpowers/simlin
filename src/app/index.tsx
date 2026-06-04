// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { createRoot } from 'react-dom/client';
// The reset must be imported explicitly: it is a side-effect import inside
// @simlin/diagram's index, which production tree-shaking has been observed to
// drop from the emitted CSS bundle -- silently losing the universal
// box-sizing: border-box and body defaults app-wide.
import '@simlin/diagram/reset.css';
import 'katex/dist/katex.min.css';
import '@simlin/diagram/theme.css';

import { App } from './App';

const element = document.getElementById('root');
if (element) {
  const root = createRoot(element);
  root.render(<App />);
}
