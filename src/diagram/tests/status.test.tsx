// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, rs } from '@rstest/core';

import * as React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';

import { Status } from '../Status';

function fillOf(container: HTMLElement): string | null {
  return container.querySelector('circle')!.getAttribute('fill');
}

describe('Status', () => {
  test('renders a green circle when status is ok', () => {
    const { container } = render(<Status status="ok" onClick={rs.fn()} />);
    expect(fillOf(container)).toBe('#2e7d32');
  });

  test('renders a red circle when status is error', () => {
    const { container } = render(<Status status="error" onClick={rs.fn()} />);
    expect(fillOf(container)).toBe('#c62828');
  });

  test('renders a grey circle when status is disabled', () => {
    const { container } = render(<Status status="disabled" onClick={rs.fn()} />);
    expect(fillOf(container)).toBe('#bdbdbd');
  });

  test('is a real button whose accessible name carries the status', () => {
    // The dot toggles the errors panel, so it must be reachable by keyboard
    // and announce more than a color: the bare <svg onClick> it replaced did
    // neither.
    const { rerender } = render(<Status status="ok" onClick={rs.fn()} />);
    expect(screen.getByRole('button', { name: /no errors/i })).not.toBeNull();

    rerender(<Status status="error" onClick={rs.fn()} />);
    expect(screen.getByRole('button', { name: /errors found/i })).not.toBeNull();

    rerender(<Status status="disabled" onClick={rs.fn()} />);
    expect(screen.getByRole('button', { name: /simulation unavailable/i })).not.toBeNull();
  });

  test('the circle is decoration: hidden from assistive tech', () => {
    const { container } = render(<Status status="ok" onClick={rs.fn()} />);
    expect(container.querySelector('svg')!.getAttribute('aria-hidden')).toBe('true');
  });

  test('clicking the button invokes onClick', () => {
    const onClick = rs.fn();
    render(<Status status="ok" onClick={onClick} />);
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('clicking the circle inside the button still invokes onClick (hit area)', () => {
    const onClick = rs.fn();
    const { container } = render(<Status status="ok" onClick={onClick} />);
    fireEvent.click(container.querySelector('circle')!);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
