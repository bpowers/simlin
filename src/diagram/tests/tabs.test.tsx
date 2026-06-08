// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import { Tabs, Tab } from '../components/Tabs';

describe('Tabs', () => {
  test('renders each tab label as a tab', () => {
    render(
      <Tabs value={0} onChange={() => {}} aria-label="selector">
        <Tab label="First" />
        <Tab label="Second" />
      </Tabs>,
    );

    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(2);
    expect(screen.getByText('First')).not.toBeNull();
    expect(screen.getByText('Second')).not.toBeNull();
  });

  test('marks the tab at `value` as selected via aria-selected', () => {
    render(
      <Tabs value={1} onChange={() => {}} aria-label="selector">
        <Tab label="First" />
        <Tab label="Second" />
      </Tabs>,
    );

    const tabs = screen.getAllByRole('tab');
    expect(tabs[0].getAttribute('aria-selected')).toBe('false');
    expect(tabs[1].getAttribute('aria-selected')).toBe('true');
  });

  test('calls onChange with the index of the clicked tab', () => {
    const onChange = jest.fn();
    render(
      <Tabs value={0} onChange={onChange} aria-label="selector">
        <Tab label="First" />
        <Tab label="Second" />
      </Tabs>,
    );

    // Radix selects on mouse-down (matching native tab behavior), not on the
    // synthetic `click` event, so drive selection the way a user would.
    fireEvent.mouseDown(screen.getByText('Second'), { button: 0 });

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0][1]).toBe(1);
  });

  test('does not call onChange when selecting the already-selected tab', () => {
    const onChange = jest.fn();
    render(
      <Tabs value={0} onChange={onChange} aria-label="selector">
        <Tab label="First" />
        <Tab label="Second" />
      </Tabs>,
    );

    fireEvent.mouseDown(screen.getByText('First'), { button: 0 });

    expect(onChange).not.toHaveBeenCalled();
  });

  test('forwards aria-label to the tab list', () => {
    render(
      <Tabs value={0} onChange={() => {}} aria-label="my selector">
        <Tab label="First" />
      </Tabs>,
    );

    const list = screen.getByRole('tablist');
    expect(list.getAttribute('aria-label')).toBe('my selector');
  });

  test('applies a custom className to the tab list', () => {
    render(
      <Tabs value={0} onChange={() => {}} className="custom-tabs" aria-label="selector">
        <Tab label="First" />
      </Tabs>,
    );

    const list = screen.getByRole('tablist');
    expect(list.className).toContain('custom-tabs');
  });

  test('ignores null/false children when assigning tab indices', () => {
    const onChange = jest.fn();
    const lookupTab = false;
    render(
      <Tabs value={0} onChange={onChange} aria-label="selector">
        <Tab label="First" />
        {lookupTab}
        <Tab label="Second" />
      </Tabs>,
    );

    // The second real tab should still resolve to index 1 despite the falsy child.
    fireEvent.mouseDown(screen.getByText('Second'), { button: 0 });
    expect(onChange.mock.calls[0][1]).toBe(1);
  });
});
