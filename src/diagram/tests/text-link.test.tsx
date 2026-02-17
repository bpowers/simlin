// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import TextLink from '../components/TextLink';

describe('TextLink', () => {
  test('renders children', () => {
    render(<TextLink>Click here</TextLink>);
    expect(screen.getByText('Click here')).not.toBeNull();
  });

  test('renders as an anchor element', () => {
    render(<TextLink href="https://example.com">Link</TextLink>);
    const link = screen.getByText('Link');
    expect(link.tagName).toBe('A');
    expect(link.getAttribute('href')).toBe('https://example.com');
  });

  test('calls onClick when clicked', () => {
    const onClick = jest.fn();
    render(<TextLink onClick={onClick}>Click</TextLink>);
    fireEvent.click(screen.getByText('Click'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('applies underline always by default', () => {
    render(<TextLink>Link</TextLink>);
    const link = screen.getByText('Link');
    expect(link.className).toContain('underlineAlways');
  });

  test('applies underline hover class', () => {
    render(<TextLink underline="hover">Link</TextLink>);
    const link = screen.getByText('Link');
    expect(link.className).toContain('underlineHover');
  });

  test('applies underline none class', () => {
    render(<TextLink underline="none">Link</TextLink>);
    const link = screen.getByText('Link');
    expect(link.className).toContain('underlineNone');
  });

  test('applies custom className', () => {
    render(<TextLink className="custom">Link</TextLink>);
    const link = screen.getByText('Link');
    expect(link.className).toContain('custom');
  });
});
