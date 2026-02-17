// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import FormControlLabel from '../components/FormControlLabel';

describe('FormControlLabel', () => {
  test('renders label text', () => {
    render(<FormControlLabel control={<input type="checkbox" />} label="Accept terms" />);
    expect(screen.getByText('Accept terms')).not.toBeNull();
  });

  test('renders control element', () => {
    render(<FormControlLabel control={<input type="checkbox" data-testid="ctrl" />} label="Label" />);
    expect(screen.getByTestId('ctrl')).not.toBeNull();
  });

  test('wraps in a label element', () => {
    const { container } = render(<FormControlLabel control={<input type="checkbox" />} label="Label" />);
    const label = container.querySelector('label');
    expect(label).not.toBeNull();
  });

  test('applies formControlLabel class', () => {
    const { container } = render(<FormControlLabel control={<input type="checkbox" />} label="Label" />);
    const label = container.querySelector('label');
    expect(label!.className).toContain('formControlLabel');
  });

  test('applies custom className', () => {
    const { container } = render(
      <FormControlLabel control={<input type="checkbox" />} label="Label" className="custom" />,
    );
    const label = container.querySelector('label');
    expect(label!.className).toContain('custom');
  });

  test('renders React element as label', () => {
    const complexLabel = (
      <span>
        I agree to the <a href="/terms">terms</a>
      </span>
    );
    render(<FormControlLabel control={<input type="checkbox" />} label={complexLabel} />);
    expect(screen.getByText('terms')).not.toBeNull();
  });
});
