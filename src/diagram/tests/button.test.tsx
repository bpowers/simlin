// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, rs } from '@rstest/core';

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import Button from '../components/Button';

describe('Button', () => {
  test('renders children text', () => {
    render(<Button>Click me</Button>);
    expect(screen.getByText('Click me')).not.toBeNull();
  });

  test('renders as a button element by default', () => {
    render(<Button>Test</Button>);
    const button = screen.getByRole('button');
    expect(button.tagName).toBe('BUTTON');
  });

  test('renders as a label when component="label"', () => {
    const { container } = render(<Button component="label">Label Button</Button>);
    const label = container.querySelector('label');
    expect(label).not.toBeNull();
    expect(label!.textContent).toBe('Label Button');
  });

  test('calls onClick when clicked', () => {
    const onClick = rs.fn();
    render(<Button onClick={onClick}>Click</Button>);
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('does not call onClick when disabled', () => {
    const onClick = rs.fn();
    render(
      <Button onClick={onClick} disabled>
        Click
      </Button>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).not.toHaveBeenCalled();
  });

  test('renders startIcon', () => {
    render(<Button startIcon={<span data-testid="icon">★</span>}>With Icon</Button>);
    expect(screen.getByTestId('icon')).not.toBeNull();
  });

  test('passes through aria attributes', () => {
    render(
      <Button aria-label="test label" aria-haspopup="true">
        Aria
      </Button>,
    );
    const button = screen.getByRole('button');
    expect(button.getAttribute('aria-label')).toBe('test label');
    expect(button.getAttribute('aria-haspopup')).toBe('true');
  });

  test('sets button type', () => {
    render(<Button type="submit">Submit</Button>);
    expect(screen.getByRole('button').getAttribute('type')).toBe('submit');
  });

  test('defaults to type="button"', () => {
    render(<Button>Default</Button>);
    expect(screen.getByRole('button').getAttribute('type')).toBe('button');
  });
});
