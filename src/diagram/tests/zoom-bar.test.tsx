// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';

import { ZoomBar } from '../ZoomBar';

describe('ZoomBar', () => {
  test('shows the current zoom snapped to the nearest step as a percent', () => {
    render(<ZoomBar zoom={1} onChangeZoom={jest.fn()} />);
    expect(screen.getByText('100%')).not.toBeNull();
  });

  test('zoom in steps to the next larger zoom level', () => {
    const onChangeZoom = jest.fn();
    render(<ZoomBar zoom={1} onChangeZoom={onChangeZoom} />);
    fireEvent.click(screen.getByRole('button', { name: /zoom in/i }));
    expect(onChangeZoom).toHaveBeenCalledWith(1.1);
  });

  test('zoom out steps to the next smaller zoom level', () => {
    const onChangeZoom = jest.fn();
    render(<ZoomBar zoom={1} onChangeZoom={onChangeZoom} />);
    fireEvent.click(screen.getByRole('button', { name: /zoom out/i }));
    expect(onChangeZoom).toHaveBeenCalledWith(0.9);
  });

  test('disables zoom in at the maximum zoom level', () => {
    const onChangeZoom = jest.fn();
    render(<ZoomBar zoom={3} onChangeZoom={onChangeZoom} />);
    const zoomIn = screen.getByRole('button', { name: /zoom in/i }) as HTMLButtonElement;
    expect(zoomIn.disabled).toBe(true);
    fireEvent.click(zoomIn);
    expect(onChangeZoom).not.toHaveBeenCalled();
  });

  test('disables zoom out at the minimum zoom level', () => {
    const onChangeZoom = jest.fn();
    render(<ZoomBar zoom={0.2} onChangeZoom={onChangeZoom} />);
    const zoomOut = screen.getByRole('button', { name: /zoom out/i }) as HTMLButtonElement;
    expect(zoomOut.disabled).toBe(true);
    fireEvent.click(zoomOut);
    expect(onChangeZoom).not.toHaveBeenCalled();
  });
});
