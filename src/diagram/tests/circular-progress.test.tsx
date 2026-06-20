// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render } from '@testing-library/react';

import CircularProgress from '../components/CircularProgress';

describe('CircularProgress', () => {
  test('renders an indeterminate progressbar with a default accessible label', () => {
    const { getByRole } = render(<CircularProgress />);
    const el = getByRole('progressbar');
    expect(el.getAttribute('aria-label')).toBe('Loading');
    // Indeterminate: no aria-valuenow.
    expect(el.getAttribute('aria-valuenow')).toBeNull();
  });

  test('applies a custom size, thickness, and label', () => {
    const { getByRole } = render(<CircularProgress size={24} thickness={2} label="Loading model" />);
    const el = getByRole('progressbar') as HTMLElement;
    expect(el.getAttribute('aria-label')).toBe('Loading model');
    expect(el.style.width).toBe('24px');
    expect(el.style.height).toBe('24px');
    expect(el.style.borderWidth).toBe('2px');
  });
});
