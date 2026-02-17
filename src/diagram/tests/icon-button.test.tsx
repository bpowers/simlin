// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import IconButton from '../components/IconButton';

describe('IconButton', () => {
  test('renders children', () => {
    render(
      <IconButton aria-label="test">
        <span data-testid="icon">★</span>
      </IconButton>,
    );
    expect(screen.getByTestId('icon')).not.toBeNull();
  });

  test('calls onClick when clicked', () => {
    const onClick = jest.fn();
    render(
      <IconButton aria-label="test" onClick={onClick}>
        ★
      </IconButton>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('does not call onClick when disabled', () => {
    const onClick = jest.fn();
    render(
      <IconButton aria-label="test" onClick={onClick} disabled>
        ★
      </IconButton>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).not.toHaveBeenCalled();
  });

  test('applies color inherit class', () => {
    render(
      <IconButton aria-label="test" color="inherit">
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('colorInherit');
  });

  test('applies size classes', () => {
    const { rerender } = render(
      <IconButton aria-label="test" size="small">
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('sizeSmall');

    rerender(
      <IconButton aria-label="test" size="large">
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('sizeLarge');
  });

  test('applies edge start class', () => {
    render(
      <IconButton aria-label="test" edge="start">
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('edgeStart');
  });

  test('applies edge end class', () => {
    render(
      <IconButton aria-label="test" edge="end">
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('edgeEnd');
  });

  test('applies disabled class', () => {
    render(
      <IconButton aria-label="test" disabled>
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('disabled');
  });

  test('applies custom className', () => {
    render(
      <IconButton aria-label="test" className="custom">
        ★
      </IconButton>,
    );
    expect(screen.getByRole('button').className).toContain('custom');
  });

  test('passes through aria-label', () => {
    render(<IconButton aria-label="close menu">★</IconButton>);
    expect(screen.getByRole('button').getAttribute('aria-label')).toBe('close menu');
  });

  test('renders as type="button"', () => {
    render(<IconButton aria-label="test">★</IconButton>);
    expect(screen.getByRole('button').getAttribute('type')).toBe('button');
  });
});
