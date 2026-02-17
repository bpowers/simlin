// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import InputAdornment from '../components/InputAdornment';

describe('InputAdornment', () => {
  test('renders children', () => {
    render(<InputAdornment position="start">$</InputAdornment>);
    expect(screen.getByText('$')).not.toBeNull();
  });

  test('applies start position class', () => {
    const { container } = render(<InputAdornment position="start">$</InputAdornment>);
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('positionStart');
  });

  test('applies end position class', () => {
    const { container } = render(<InputAdornment position="end">kg</InputAdornment>);
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('positionEnd');
  });

  test('applies custom className', () => {
    const { container } = render(
      <InputAdornment position="start" className="custom">
        $
      </InputAdornment>,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('custom');
  });
});
