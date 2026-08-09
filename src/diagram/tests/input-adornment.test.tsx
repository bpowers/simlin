// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect } from '@rstest/core';

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import InputAdornment from '../components/InputAdornment';

describe('InputAdornment', () => {
  test('renders children', () => {
    render(<InputAdornment position="start">$</InputAdornment>);
    expect(screen.getByText('$')).not.toBeNull();
  });
});
