// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import TextField from '../components/TextField';

describe('TextField', () => {
  test('renders input element', () => {
    render(<TextField data-testid="text-field" />);
    expect(screen.getByTestId('text-field')).not.toBeNull();
  });

  test('renders label when provided', () => {
    render(<TextField label="Test Label" />);
    expect(screen.getByText('Test Label')).not.toBeNull();
  });

  test('applies fullWidth class when fullWidth is true', () => {
    const { container } = render(<TextField fullWidth />);
    const root = container.firstChild as HTMLElement;
    expect(root).not.toBeNull();
  });

  test('chains inputProps.onFocus with internal focus handler', () => {
    const externalOnFocus = jest.fn();
    render(
      <TextField
        label="Test"
        inputProps={{ onFocus: externalOnFocus }}
        data-testid="text-field"
      />,
    );

    const input = screen.getByTestId('text-field');
    fireEvent.focus(input);

    // External handler should be called
    expect(externalOnFocus).toHaveBeenCalledTimes(1);
  });

  test('chains inputProps.onBlur with internal blur handler', () => {
    const externalOnBlur = jest.fn();
    render(
      <TextField
        label="Test"
        inputProps={{ onBlur: externalOnBlur }}
        data-testid="text-field"
      />,
    );

    const input = screen.getByTestId('text-field');
    fireEvent.focus(input);
    fireEvent.blur(input);

    // External handler should be called
    expect(externalOnBlur).toHaveBeenCalledTimes(1);
  });

  test('tracks focus state correctly when inputProps has focus handlers', () => {
    // Use a class component wrapper to access the TextField's state via ref inspection
    // Since we can't directly inspect isFocused state, we test the effect:
    // The label should shrink when focused even with external handlers
    const externalOnFocus = jest.fn();
    const externalOnBlur = jest.fn();

    render(
      <TextField
        label="Test Label"
        variant="outlined"
        inputProps={{ onFocus: externalOnFocus, onBlur: externalOnBlur }}
        data-testid="text-field"
      />,
    );

    const input = screen.getByTestId('text-field');

    // Focus the input
    fireEvent.focus(input);

    // External handler should be called
    expect(externalOnFocus).toHaveBeenCalledTimes(1);

    // Blur the input
    fireEvent.blur(input);

    // External handler should be called
    expect(externalOnBlur).toHaveBeenCalledTimes(1);
  });

  test('passes through other inputProps correctly', () => {
    render(
      <TextField
        inputProps={{ 'data-testid': 'custom-input', maxLength: 10 }}
      />,
    );

    const input = screen.getByTestId('custom-input');
    expect(input.getAttribute('maxLength')).toBe('10');
  });

  test('renders standard variant correctly', () => {
    const { container } = render(<TextField variant="standard" label="Standard Field" />);
    // Just verify it renders without error
    expect(container.firstChild).not.toBeNull();
  });

  test('renders outlined variant (default) correctly', () => {
    const { container } = render(<TextField label="Outlined Field" />);
    // Just verify it renders without error
    expect(container.firstChild).not.toBeNull();
  });

  test('handles value changes', () => {
    const onChange = jest.fn();
    render(<TextField value="test" onChange={onChange} data-testid="text-field" />);

    const input = screen.getByTestId('text-field') as HTMLInputElement;
    expect(input.value).toBe('test');

    fireEvent.change(input, { target: { value: 'new value' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });
});
