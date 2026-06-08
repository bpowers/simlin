// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent } from '@testing-library/react';

import { Status } from '../Status';

function fillOf(container: HTMLElement): string | null {
  return container.querySelector('circle')!.getAttribute('fill');
}

describe('Status', () => {
  test('renders a green circle when status is ok', () => {
    const { container } = render(<Status status="ok" onClick={jest.fn()} />);
    expect(fillOf(container)).toBe('#81c784');
  });

  test('renders an orange circle when status is error', () => {
    const { container } = render(<Status status="error" onClick={jest.fn()} />);
    expect(fillOf(container)).toBe('rgb(255, 152, 0)');
  });

  test('renders a grey circle when status is disabled', () => {
    const { container } = render(<Status status="disabled" onClick={jest.fn()} />);
    expect(fillOf(container)).toBe('#DCDCDC');
  });

  test('clicking the circle invokes onClick', () => {
    const onClick = jest.fn();
    const { container } = render(<Status status="ok" onClick={onClick} />);
    fireEvent.click(container.querySelector('circle')!);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
