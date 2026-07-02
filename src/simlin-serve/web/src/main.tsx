// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';
// @simlin/diagram carries its own reset/theme CSS via its package root, but
// not katex's stylesheet: the diagram's Node build can only stub its *own*
// CSS files (build-css.sh), so third-party CSS must come from the browser
// host's entry — the same arrangement as src/app's index.tsx.
import 'katex/dist/katex.min.css';
import './styles.css';

const element = document.getElementById('root');
if (element) {
  const root = createRoot(element);
  root.render(<App />);
}
