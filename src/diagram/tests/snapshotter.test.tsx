// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';

import { Snapshotter } from '../Snapshotter';

describe('Snapshotter', () => {
  test('clicking the snapshot button calls onSnapshot with "show"', () => {
    const onSnapshot = jest.fn();
    render(<Snapshotter onSnapshot={onSnapshot} />);
    fireEvent.click(screen.getByRole('button', { name: /snapshot/i }));
    expect(onSnapshot).toHaveBeenCalledWith('show');
  });
});
