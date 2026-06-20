// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen, waitFor, act, createEvent } from '@testing-library/react';
import Autocomplete from '../components/Autocomplete';

// Simple controlled component to test Autocomplete with external value changes.
// Exposes a `setValue` imperative handle so tests can drive the value prop from
// outside, matching the old class component's public surface.
interface ControlledAutocompleteHandle {
  setValue: (newValue: string | null) => void;
}

const ControlledAutocomplete = React.forwardRef<
  ControlledAutocompleteHandle,
  { initialValue?: string; options: string[] }
>(function ControlledAutocomplete({ initialValue, options }, ref) {
  const [value, setValue] = React.useState<string | null>(initialValue || null);

  React.useImperativeHandle(ref, () => ({ setValue }), []);

  return (
    <Autocomplete
      value={value}
      options={options}
      onChange={(_event, newValue) => setValue(newValue)}
      renderInput={(params) => (
        <div ref={params.InputProps.ref}>
          <input {...params.inputProps} data-testid="autocomplete-input" />
        </div>
      )}
    />
  );
});

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
    const ref = React.createRef<ControlledAutocompleteHandle>();
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
    const ref = React.createRef<ControlledAutocompleteHandle>();
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
    const ref = React.createRef<ControlledAutocompleteHandle>();
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

  test('scrolls the keyboard-highlighted option into view', async () => {
    // jsdom does not implement scrollIntoView, so install a stub to observe the
    // call (the component guards with optional chaining for exactly this reason).
    const scrollFn = jest.fn();
    (HTMLElement.prototype as unknown as { scrollIntoView: unknown }).scrollIntoView = scrollFn;
    try {
      render(
        <Autocomplete
          value={null}
          options={['apple', 'apricot', 'avocado']}
          onChange={() => {}}
          renderInput={(params) => (
            <div ref={params.InputProps.ref}>
              <input {...params.inputProps} data-testid="autocomplete-input" />
            </div>
          )}
        />,
      );

      const input = screen.getByTestId('autocomplete-input');
      fireEvent.change(input, { target: { value: 'a' } });
      await screen.findByText('apple');

      // Arrow navigation must scroll the now-highlighted row into view.
      fireEvent.keyDown(input, { key: 'ArrowDown' });
      await waitFor(() => {
        expect(scrollFn).toHaveBeenCalled();
      });
    } finally {
      delete (HTMLElement.prototype as unknown as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  test('commits the highlighted option on keyboard blur (tab away)', () => {
    const onChange = jest.fn();
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot', 'avocado']}
        onChange={onChange}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    const input = screen.getByTestId('autocomplete-input');
    fireEvent.change(input, { target: { value: 'a' } });
    fireEvent.keyDown(input, { key: 'ArrowDown' }); // highlight 'apple'
    fireEvent.blur(input);

    expect(onChange).toHaveBeenCalledWith(null, 'apple');
  });

  test('does not commit the highlight on a mouse-driven blur', () => {
    const onChange = jest.fn();
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot', 'avocado']}
        onChange={onChange}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    const input = screen.getByTestId('autocomplete-input');
    fireEvent.change(input, { target: { value: 'a' } });
    fireEvent.keyDown(input, { key: 'ArrowDown' }); // highlight 'apple'
    // A pointer press in progress means this blur is a click elsewhere, not a
    // tab-away, so the highlight must not be auto-committed.
    fireEvent.mouseDown(document.body);
    fireEvent.blur(input);

    expect(onChange).not.toHaveBeenCalled();
  });

  test('option press prevents default on pointerdown so touch taps keep focus', async () => {
    // On touch, focus can shift on pointerdown (before the synthesized
    // mousedown); preventing it keeps focus on the input so the list is not
    // unmounted by a blur before the tap's click selects the option.
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    fireEvent.change(screen.getByTestId('autocomplete-input'), { target: { value: 'ap' } });
    const option = await screen.findByText('apple');

    const pointerDown = createEvent.pointerDown(option);
    fireEvent(option, pointerDown);
    expect(pointerDown.defaultPrevented).toBe(true);

    const mouseDown = createEvent.mouseDown(option);
    fireEvent(option, mouseDown);
    expect(mouseDown.defaultPrevented).toBe(true);
  });

  test('selecting an option fills the input even when uncontrolled', async () => {
    // The parent ignores onChange (value stays null), so the input text must
    // come from the component itself -- downshift used to set it on select.
    render(
      <Autocomplete
        value={null}
        options={['apple', 'apricot', 'banana']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="autocomplete-input" />
          </div>
        )}
      />,
    );

    const input = screen.getByTestId('autocomplete-input') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'ap' } });

    const apple = await screen.findByText('apple');
    fireEvent.click(apple);

    await waitFor(() => {
      expect(input.value).toBe('apple');
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
