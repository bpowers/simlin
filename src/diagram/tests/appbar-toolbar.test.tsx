// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

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

  test('applies static position by default', () => {
    const { container } = render(<AppBar>Content</AppBar>);
    const header = container.querySelector('header')!;
    expect(header.className).toContain('positionStatic');
  });

  test('applies fixed position class', () => {
    const { container } = render(<AppBar position="fixed">Content</AppBar>);
    const header = container.querySelector('header')!;
    expect(header.className).toContain('positionFixed');
  });

  test('applies sticky position class', () => {
    const { container } = render(<AppBar position="sticky">Content</AppBar>);
    const header = container.querySelector('header')!;
    expect(header.className).toContain('positionSticky');
  });

  test('applies custom className', () => {
    const { container } = render(<AppBar className="custom-bar">Content</AppBar>);
    const header = container.querySelector('header')!;
    expect(header.className).toContain('custom-bar');
  });
});

describe('Toolbar', () => {
  test('renders children', () => {
    render(<Toolbar>Toolbar Content</Toolbar>);
    expect(screen.getByText('Toolbar Content')).not.toBeNull();
  });

  test('applies regular variant by default', () => {
    const { container } = render(<Toolbar>Content</Toolbar>);
    const toolbar = container.firstChild as HTMLElement;
    expect(toolbar.className).toContain('regular');
  });

  test('applies dense variant', () => {
    const { container } = render(<Toolbar variant="dense">Content</Toolbar>);
    const toolbar = container.firstChild as HTMLElement;
    expect(toolbar.className).toContain('dense');
  });

  test('applies custom className', () => {
    const { container } = render(<Toolbar className="custom-toolbar">Content</Toolbar>);
    const toolbar = container.firstChild as HTMLElement;
    expect(toolbar.className).toContain('custom-toolbar');
  });
});
