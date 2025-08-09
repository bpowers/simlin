// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { createRoot } from 'react-dom/client';
import 'katex/dist/katex.min.css';

import { App } from './App';

const element = document.getElementById('root');
if (element) {
  const root = createRoot(element);
  root.render(<App />);
}
