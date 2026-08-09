// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect } from '@rstest/core';

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import Card, { CardContent, CardActions } from '../components/Card';

describe('Card', () => {
  test('renders children', () => {
    render(<Card>Card content</Card>);
    expect(screen.getByText('Card content')).not.toBeNull();
  });

  test('applies custom style', () => {
    const { container } = render(<Card style={{ maxWidth: 300 }}>Content</Card>);
    const card = container.firstChild as HTMLElement;
    expect(card.style.maxWidth).toBe('300px');
  });
});

describe('CardContent', () => {
  test('renders children', () => {
    render(<CardContent>Inner content</CardContent>);
    expect(screen.getByText('Inner content')).not.toBeNull();
  });
});

describe('CardActions', () => {
  test('renders children', () => {
    render(
      <CardActions>
        <button>Action</button>
      </CardActions>,
    );
    expect(screen.getByText('Action')).not.toBeNull();
  });
});
