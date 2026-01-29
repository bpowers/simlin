// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen, waitFor, act } from '@testing-library/react';
import Autocomplete from '../components/Autocomplete';

// Simple controlled component to test Autocomplete with external value changes
class ControlledAutocomplete extends React.Component<
  { initialValue?: string; options: string[] },
  { value: string | null }
> {
  state = { value: this.props.initialValue || null };

  setValue = (newValue: string | null) => {
    this.setState({ value: newValue });
  };

  render() {
    return (
      <Autocomplete
        value={this.state.value}
        options={this.props.options}
        onChange={(_event, newValue) => this.setValue(newValue)}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />
    );
  }
}

describe('Autocomplete', () => {
  test('renders input element', () => {
    render(
      <Autocomplete
        value={null}
        options={['apple', 'banana', 'cherry']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    expect(screen.getByTestId('autocomplete-input')).not.toBeNull();
  });

  test('filters options based on input value', async () => {
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot', 'banana', 'cherry']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    const input = screen.getByTestId('autocomplete-input');
    fireEvent.change(input, { target: { value: 'ap' } });

    // The listbox should be in the document (portaled to body)
    await waitFor(() => {
      const options = document.querySelectorAll('li');
      expect(options.length).toBe(2);
    });
  });

  test('syncs inputValue when value prop changes externally', async () => {
    const ref = React.createRef<ControlledAutocomplete>();
    render(<ControlledAutocomplete ref={ref} options={['apple', 'banana', 'cherry']} />);

    const input = screen.getByTestId('autocomplete-input') as HTMLInputElement;
    expect(input.value).toBe('');

    // Programmatically change the value from outside
    act(() => {
      ref.current!.setValue('banana');
    });

    // The input value should sync with the new value prop
    await waitFor(() => {
      expect(input.value).toBe('banana');
    });
  });

  test('inputValue syncs when value prop changes from one value to another', async () => {
    const ref = React.createRef<ControlledAutocomplete>();
    render(<ControlledAutocomplete ref={ref} initialValue="apple" options={['apple', 'banana', 'cherry']} />);

    const input = screen.getByTestId('autocomplete-input') as HTMLInputElement;

    await waitFor(() => {
      expect(input.value).toBe('apple');
    });

    // Change to a different value
    act(() => {
      ref.current!.setValue('cherry');
    });

    await waitFor(() => {
      expect(input.value).toBe('cherry');
    });
  });

  test('inputValue clears when value prop is set to null', async () => {
    const ref = React.createRef<ControlledAutocomplete>();
    render(<ControlledAutocomplete ref={ref} initialValue="apple" options={['apple', 'banana', 'cherry']} />);

    const input = screen.getByTestId('autocomplete-input') as HTMLInputElement;

    await waitFor(() => {
      expect(input.value).toBe('apple');
    });

    // Clear the value
    act(() => {
      ref.current!.setValue(null);
    });

    await waitFor(() => {
      expect(input.value).toBe('');
    });
  });

  test('clearOnEscape clears selection on Escape key', async () => {
    const onChange = jest.fn();
    render(
      <Autocomplete
        value="apple"
        options={['apple', 'banana', 'cherry']}
        onChange={onChange}
        clearOnEscape
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    const input = screen.getByTestId('autocomplete-input');
    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => {
      expect(onChange).toHaveBeenCalledWith(null, null);
    });
  });
});
