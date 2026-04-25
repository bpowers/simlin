// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';
import { captureLaunchToken } from './launch-token';
import './styles.css';

// Run before mounting React so the first network request — App's
// componentDidMount fetch of /api/projects — already carries the bearer.
captureLaunchToken();

const element = document.getElementById('root');
if (element) {
  const root = createRoot(element);
  root.render(<App />);
}
