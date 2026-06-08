// Copyright 2026 The Simlin Authors. All rights reserved.
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

describe('Autocomplete dropdown positioning', () => {
  test('repositions the portaled listbox when an ancestor scrolls while open', async () => {
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref} data-testid="wrapper">
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    // jsdom has no layout: stub the wrapper's rect and move it between calls,
    // simulating the input shifting as its scrollable container scrolls.
    const wrapper = screen.getByTestId('wrapper');
    const rect = {
      top: 100,
      bottom: 120,
      left: 10,
      right: 210,
      width: 200,
      height: 20,
      x: 10,
      y: 100,
      toJSON: () => ({}),
    };
    const rectMock = jest.fn(() => ({ ...rect }) as DOMRect);
    wrapper.getBoundingClientRect = rectMock;

    // Typing opens the dropdown; the position is computed from the rect.
    fireEvent.change(screen.getByTestId('autocomplete-input'), { target: { value: 'ap' } });
    let listbox: HTMLElement | null = null;
    await waitFor(() => {
      listbox = document.querySelector('ul[role="listbox"]');
      expect(listbox).not.toBeNull();
      expect((listbox as HTMLElement).style.top).toBe('120px');
    });

    // The container scrolls: the input is now 60px higher on screen. A
    // capture-phase scroll listener must recompute the portal position.
    rect.top = 40;
    rect.bottom = 60;
    await act(async () => {
      fireEvent.scroll(document.body);
    });

    await waitFor(() => {
      expect((document.querySelector('ul[role="listbox"]') as HTMLElement).style.top).toBe('60px');
    });
  });

  test('repositions the portaled listbox on window resize while open', async () => {
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref} data-testid="wrapper">
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    const wrapper = screen.getByTestId('wrapper');
    const rect = {
      top: 100,
      bottom: 120,
      left: 10,
      right: 210,
      width: 200,
      height: 20,
      x: 10,
      y: 100,
      toJSON: () => ({}),
    };
    wrapper.getBoundingClientRect = jest.fn(() => ({ ...rect }) as DOMRect);

    fireEvent.change(screen.getByTestId('autocomplete-input'), { target: { value: 'ap' } });
    await waitFor(() => {
      expect((document.querySelector('ul[role="listbox"]') as HTMLElement).style.top).toBe('120px');
    });

    rect.left = 50;
    await act(async () => {
      fireEvent.resize(window);
    });

    await waitFor(() => {
      expect((document.querySelector('ul[role="listbox"]') as HTMLElement).style.left).toBe('50px');
    });
  });
});
