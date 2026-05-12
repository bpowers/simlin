// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';

import { ModelPropertiesDrawer } from '../ModelPropertiesDrawer';

function renderDrawer(overrides: Partial<React.ComponentProps<typeof ModelPropertiesDrawer>> = {}) {
  const noop = () => {};
  return render(
    <ModelPropertiesDrawer
      modelName="climate"
      open={true}
      onDrawerToggle={noop}
      startTime={0}
      stopTime={100}
      dt={1}
      timeUnits="years"
      onStartTimeChange={noop}
      onStopTimeChange={noop}
      onDtChange={noop}
      onTimeUnitsChange={noop}
      onDownloadXmile={noop}
      {...overrides}
    />,
  );
}

describe('ModelPropertiesDrawer', () => {
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
