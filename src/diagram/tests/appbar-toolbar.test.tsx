// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect } from '@rstest/core';

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import AppBar from '../components/AppBar';
import Toolbar from '../components/Toolbar';

describe('AppBar', () => {
  test('renders children', () => {
    render(<AppBar>App Bar Content</AppBar>);
    expect(screen.getByText('App Bar Content')).not.toBeNull();
  });

  test('renders as a header element', () => {
    render(<AppBar>Content</AppBar>);
    const header = screen.getByText('Content').closest('header');
    expect(header).not.toBeNull();
  });
});

describe('Toolbar', () => {
  test('renders children', () => {
    render(<Toolbar>Toolbar Content</Toolbar>);
    expect(screen.getByText('Toolbar Content')).not.toBeNull();
  });
});
