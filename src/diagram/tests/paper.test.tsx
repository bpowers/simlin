// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import Paper from '../components/Paper';

describe('Paper', () => {
  test('renders children', () => {
    render(<Paper>Paper content</Paper>);
    expect(screen.getByText('Paper content')).not.toBeNull();
  });

  test('applies elevation1 class by default', () => {
    const { container } = render(<Paper>Content</Paper>);
    const paper = container.firstChild as HTMLElement;
    expect(paper.className).toContain('elevation1');
  });

  test('applies elevation0 class when elevation is 0', () => {
    const { container } = render(<Paper elevation={0}>Content</Paper>);
    const paper = container.firstChild as HTMLElement;
    expect(paper.className).toContain('elevation0');
  });

  test('applies elevation1 class for low elevations', () => {
    const { container } = render(<Paper elevation={4}>Content</Paper>);
    const paper = container.firstChild as HTMLElement;
    expect(paper.className).toContain('elevation1');
  });

  test('applies elevation2 class for high elevations', () => {
    const { container } = render(<Paper elevation={8}>Content</Paper>);
    const paper = container.firstChild as HTMLElement;
    expect(paper.className).toContain('elevation2');
  });

  test('applies custom className', () => {
    const { container } = render(<Paper className="custom">Content</Paper>);
    const paper = container.firstChild as HTMLElement;
    expect(paper.className).toContain('custom');
  });

  test('applies custom style', () => {
    const { container } = render(<Paper style={{ padding: 20 }}>Content</Paper>);
    const paper = container.firstChild as HTMLElement;
    expect(paper.style.padding).toBe('20px');
  });
});
