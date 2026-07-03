// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';

import { ModelPropertiesDrawer } from '../ModelPropertiesDrawer';
import type { SimSpecField } from '../sim-spec-draft';

type DrawerProps = React.ComponentProps<typeof ModelPropertiesDrawer>;

function baseProps(overrides: Partial<DrawerProps> = {}): DrawerProps {
  const noop = () => {};
  return {
    modelName: 'climate',
    open: true,
    onDrawerToggle: noop,
    startTime: 0,
    stopTime: 100,
    dt: 1,
    timeUnits: 'years',
    onSimSpecCommit: noop,
    onDownloadXmile: noop,
    ...overrides,
  };
}

function renderDrawer(overrides: Partial<DrawerProps> = {}) {
  return render(<ModelPropertiesDrawer {...baseProps(overrides)} />);
}

// Focus via the real DOM method so jsdom sets activeElement, which is what
// makes a later `.blur()` (used by Enter/Escape) actually dispatch a blur.
function focus(input: HTMLElement): void {
  act(() => {
    (input as HTMLInputElement).focus();
  });
}

function getField(label: RegExp): HTMLInputElement {
  return screen.getByLabelText(label) as HTMLInputElement;
}

describe('ModelPropertiesDrawer', () => {
  describe('existing affordances', () => {
    test('always offers the model download', () => {
      renderDrawer();
      expect(screen.getByRole('button', { name: /download model/i })).not.toBeNull();
    });

    test('does not show a delete action when onDelete is not provided', () => {
      renderDrawer();
      expect(screen.queryByRole('button', { name: /delete project/i })).toBeNull();
    });

    test('shows a delete action when onDelete is provided', () => {
      renderDrawer({ onDelete: jest.fn() });
      expect(screen.getByRole('button', { name: /delete project/i })).not.toBeNull();
    });

    test('confirming the delete dialog invokes onDelete', async () => {
      const onDelete = jest.fn().mockResolvedValue(undefined);
      renderDrawer({ onDelete });
      fireEvent.click(screen.getByRole('button', { name: /delete project/i }));
      await waitFor(() => expect(screen.getByText(/delete this project\?/i)).not.toBeNull());
      await act(async () => {
        fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
      });
      expect(onDelete).toHaveBeenCalledTimes(1);
    });
  });

  describe('sim-specs draft commit (issue #55)', () => {
    test('displays the model values', () => {
      renderDrawer();
      expect(getField(/start time/i).value).toBe('0');
      expect(getField(/stop time/i).value).toBe('100');
      expect(getField(/^dt$/i).value).toBe('1');
      expect(getField(/time units/i).value).toBe('years');
    });

    test('typing does NOT commit per keystroke', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '1' } });
      fireEvent.change(field, { target: { value: '19' } });
      fireEvent.change(field, { target: { value: '190' } });
      fireEvent.change(field, { target: { value: '1900' } });
      expect(onSimSpecCommit).not.toHaveBeenCalled();
    });

    test('blur commits exactly once with the final numeric value', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '1900' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
      expect(onSimSpecCommit).toHaveBeenCalledWith('startTime' satisfies SimSpecField, 1900);
    });

    test('Enter commits once', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/stop time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '250' } });
      fireEvent.keyDown(field, { key: 'Enter' });
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
      expect(onSimSpecCommit).toHaveBeenCalledWith('stopTime' satisfies SimSpecField, 250);
    });

    test('dt commits as a positive number', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/^dt$/i);
      focus(field);
      fireEvent.change(field, { target: { value: '0.25' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
      expect(onSimSpecCommit).toHaveBeenCalledWith('dt' satisfies SimSpecField, 0.25);
    });

    test('a non-positive dt does not commit and reverts on blur', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/^dt$/i);
      focus(field);
      fireEvent.change(field, { target: { value: '0' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).not.toHaveBeenCalled();
      expect(field.value).toBe('1');
    });

    test('time units commits the free string', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/time units/i);
      focus(field);
      fireEvent.change(field, { target: { value: 'months' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
      expect(onSimSpecCommit).toHaveBeenCalledWith('timeUnits' satisfies SimSpecField, 'months');
    });

    test('time units may be cleared to empty', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/time units/i);
      focus(field);
      fireEvent.change(field, { target: { value: '' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
      expect(onSimSpecCommit).toHaveBeenCalledWith('timeUnits' satisfies SimSpecField, '');
    });

    test('empty numeric input does not commit and reverts on blur', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).not.toHaveBeenCalled();
      expect(field.value).toBe('0');
    });

    test('an unchanged value on blur does not commit', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '0' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).not.toHaveBeenCalled();
    });

    test('Escape reverts the draft without committing', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '4321' } });
      expect(field.value).toBe('4321');
      fireEvent.keyDown(field, { key: 'Escape' });
      expect(onSimSpecCommit).not.toHaveBeenCalled();
      expect(field.value).toBe('0');
    });

    test('Escape in a field does not dismiss the drawer', () => {
      // Escape means "revert this field", not "close the drawer": the field
      // handler stops propagation so the Drawer's own Escape-close listener
      // never sees the key.
      const onDrawerToggle = jest.fn();
      renderDrawer({ onDrawerToggle });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '4321' } });
      fireEvent.keyDown(field, { key: 'Escape' });
      expect(onDrawerToggle).not.toHaveBeenCalled();
    });

    test('a props refresh while unfocused updates the display', () => {
      const { rerender } = renderDrawer();
      expect(getField(/start time/i).value).toBe('0');
      rerender(<ModelPropertiesDrawer {...baseProps({ startTime: 50 })} />);
      expect(getField(/start time/i).value).toBe('50');
    });

    test('a props refresh while focused does NOT clobber the draft', () => {
      const onSimSpecCommit = jest.fn();
      const { rerender } = renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '1234' } });
      // An engine refresh (e.g. autosave completing) republishes props.
      rerender(<ModelPropertiesDrawer {...baseProps({ startTime: 999, onSimSpecCommit })} />);
      expect(getField(/start time/i).value).toBe('1234');
      expect(onSimSpecCommit).not.toHaveBeenCalled();
    });

    test('a valid commit keeps the typed value visible (no flash of the old value)', () => {
      const onSimSpecCommit = jest.fn();
      const { rerender } = renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '1900' } });
      fireEvent.blur(field);
      // Before the engine round-trip republishes props the field still shows
      // the committed value, not the pre-edit 0.
      expect(getField(/start time/i).value).toBe('1900');
      // Once the model catches up, the draft releases to the (equal) prop.
      rerender(<ModelPropertiesDrawer {...baseProps({ startTime: 1900, onSimSpecCommit })} />);
      expect(getField(/start time/i).value).toBe('1900');
    });

    test('committing one field then editing another commits each once', () => {
      const onSimSpecCommit = jest.fn();
      renderDrawer({ onSimSpecCommit });
      const start = getField(/start time/i);
      focus(start);
      fireEvent.change(start, { target: { value: '10' } });
      fireEvent.blur(start);
      const stop = getField(/stop time/i);
      focus(stop);
      fireEvent.change(stop, { target: { value: '20' } });
      fireEvent.blur(stop);
      expect(onSimSpecCommit).toHaveBeenCalledTimes(2);
      expect(onSimSpecCommit).toHaveBeenNthCalledWith(1, 'startTime', 10);
      expect(onSimSpecCommit).toHaveBeenNthCalledWith(2, 'stopTime', 20);
    });

    test('unmounting a field with a pending draft (drawer close) commits on blur', () => {
      // The Drawer never unmounts its children on close -- it CSS-hides the
      // panel and its [open] effect restores focus to the previously-active
      // element, which is what blurs the focused field and drives the commit.
      // We assert that ordering explicitly: a blur commits, and whatever
      // teardown follows (here, unmount) fires nothing further.
      const onSimSpecCommit = jest.fn();
      const { unmount } = renderDrawer({ onSimSpecCommit });
      const field = getField(/start time/i);
      focus(field);
      fireEvent.change(field, { target: { value: '77' } });
      fireEvent.blur(field);
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
      unmount();
      expect(onSimSpecCommit).toHaveBeenCalledTimes(1);
    });
  });
});
