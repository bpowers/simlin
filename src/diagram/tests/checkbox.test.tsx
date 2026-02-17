// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import Checkbox from '../components/Checkbox';

describe('Checkbox', () => {
  test('renders a checkbox role', () => {
    render(<Checkbox />);
    expect(screen.getByRole('checkbox')).not.toBeNull();
  });

  test('calls onChange when clicked', () => {
    const onChange = jest.fn();
    render(<Checkbox onChange={onChange} />);
    fireEvent.click(screen.getByRole('checkbox'));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  test('toggles checked state', () => {
    const onChange = jest.fn();
    render(<Checkbox checked={true} onChange={onChange} />);
    fireEvent.click(screen.getByRole('checkbox'));
    expect(onChange).toHaveBeenCalledWith(false);
  });

  test('applies primary class by default', () => {
    render(<Checkbox />);
    const checkbox = screen.getByRole('checkbox');
    expect(checkbox.className).toContain('primary');
  });

  test('applies secondary class', () => {
    render(<Checkbox color="secondary" />);
    const checkbox = screen.getByRole('checkbox');
    expect(checkbox.className).toContain('secondary');
  });

  test('respects disabled prop', () => {
    const onChange = jest.fn();
    render(<Checkbox disabled onChange={onChange} />);
    const checkbox = screen.getByRole('checkbox');
    expect(checkbox).toHaveProperty('disabled', true);
  });

  test('applies custom className', () => {
    render(<Checkbox className="custom-check" />);
    const checkbox = screen.getByRole('checkbox');
    expect(checkbox.className).toContain('custom-check');
  });

  test('renders with checked state', () => {
    render(<Checkbox checked={true} />);
    const checkbox = screen.getByRole('checkbox');
    expect(checkbox.getAttribute('data-state')).toBe('checked');
  });

  test('renders with unchecked state', () => {
    render(<Checkbox checked={false} />);
    const checkbox = screen.getByRole('checkbox');
    expect(checkbox.getAttribute('data-state')).toBe('unchecked');
  });
});
