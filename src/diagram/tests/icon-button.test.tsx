// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, rs } from '@rstest/core';

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
    const onClick = rs.fn();
    render(
      <IconButton aria-label="test" onClick={onClick}>
        ★
      </IconButton>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('does not call onClick when disabled', () => {
    const onClick = rs.fn();
    render(
      <IconButton aria-label="test" onClick={onClick} disabled>
        ★
      </IconButton>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).not.toHaveBeenCalled();
  });

  test('passes through aria-label', () => {
    render(<IconButton aria-label="close menu">★</IconButton>);
    expect(screen.getByRole('button').getAttribute('aria-label')).toBe('close menu');
  });

  test('renders as type="button"', () => {
    render(<IconButton aria-label="test">★</IconButton>);
    expect(screen.getByRole('button').getAttribute('type')).toBe('button');
  });

  test('renders an anchor (not a button) when given href', () => {
    // A <button> nested inside an anchor is invalid interactive content, so
    // href mode makes the anchor itself the styled element.
    render(
      <IconButton aria-label="go home" href="/home">
        ★
      </IconButton>,
    );
    const link = screen.getByRole('link', { name: 'go home' });
    expect(link.tagName).toBe('A');
    expect(link.getAttribute('href')).toBe('/home');
    expect(screen.queryByRole('button')).toBeNull();
  });

  test('ignores disabled in href mode, styling included', () => {
    // Links have no disabled state: a greyed, pointer-events:none anchor would
    // still be keyboard-focusable and Enter would navigate, i.e. it would LOOK
    // disabled without being so. The disabled styling must therefore stay off.
    const onClick = rs.fn();
    render(
      <IconButton aria-label="go home" href="/home" disabled onClick={onClick}>
        ★
      </IconButton>,
    );

    const link = screen.getByRole('link', { name: 'go home' });
    expect(link.hasAttribute('disabled')).toBe(false);
    expect(link.getAttribute('aria-disabled')).toBeNull();
    // Natively focusable: nothing removed it from the tab order.
    expect(link.getAttribute('tabindex')).toBeNull();
    expect(link.className).not.toContain('disabled');

    fireEvent.click(link);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
