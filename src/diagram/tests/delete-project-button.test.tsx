// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';

import { DeleteProjectButton } from '../DeleteProjectButton';

describe('DeleteProjectButton', () => {
  test('renders a Delete project button with the confirmation dialog initially closed', () => {
    render(<DeleteProjectButton projectName="climate" onDelete={jest.fn()} />);
    expect(screen.getByRole('button', { name: /delete project/i })).not.toBeNull();
    expect(screen.queryByText(/delete this project\?/i)).toBeNull();
  });

  test('clicking the trigger opens a confirmation dialog naming the project', () => {
    render(<DeleteProjectButton projectName="climate-101" onDelete={jest.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: /delete project/i }));
    expect(screen.getByText(/delete this project\?/i)).not.toBeNull();
    expect(screen.getByText(/climate-101/)).not.toBeNull();
  });

  test('Cancel closes the dialog without calling onDelete', () => {
    const onDelete = jest.fn();
    render(<DeleteProjectButton projectName="climate" onDelete={onDelete} />);
    fireEvent.click(screen.getByRole('button', { name: /delete project/i }));
    fireEvent.click(screen.getByRole('button', { name: /^cancel$/i }));
    expect(screen.queryByText(/delete this project\?/i)).toBeNull();
    expect(onDelete).not.toHaveBeenCalled();
  });

  test('confirming calls onDelete', async () => {
    const onDelete = jest.fn().mockResolvedValue(undefined);
    render(<DeleteProjectButton projectName="climate" onDelete={onDelete} />);
    fireEvent.click(screen.getByRole('button', { name: /delete project/i }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    });
    expect(onDelete).toHaveBeenCalledTimes(1);
  });

  test('a rejected onDelete keeps the dialog open and shows the error', async () => {
    const onDelete = jest.fn().mockRejectedValue(new Error('network is down'));
    render(<DeleteProjectButton projectName="climate" onDelete={onDelete} />);
    fireEvent.click(screen.getByRole('button', { name: /delete project/i }));
    fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));

    await waitFor(() => {
      expect(screen.getByText(/network is down/i)).not.toBeNull();
    });
    // dialog is still open and the action buttons are usable again
    expect(screen.getByText(/delete this project\?/i)).not.toBeNull();
    const confirmButton = screen.getByRole('button', { name: /^delete$/i }) as HTMLButtonElement;
    expect(confirmButton.disabled).toBe(false);
  });

  test('the action buttons are disabled while the delete is in flight', async () => {
    // Never-resolving promise: the component should stay in its "deleting" state.
    const onDelete = jest.fn().mockReturnValue(new Promise<void>(() => {}));
    render(<DeleteProjectButton projectName="climate" onDelete={onDelete} />);
    fireEvent.click(screen.getByRole('button', { name: /delete project/i }));
    act(() => {
      fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    });
    expect((screen.getByRole('button', { name: /^cancel$/i }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: /deleting/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});
