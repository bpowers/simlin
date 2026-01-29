// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, act } from '@testing-library/react';
import Snackbar, { SnackbarContent } from '../components/Snackbar';

// Controlled wrapper to test open/close transitions
class ControlledSnackbar extends React.Component<
  { autoHideDuration?: number },
  { open: boolean; closeCount: number }
> {
  state = { open: false, closeCount: 0 };

  setOpen = (open: boolean) => {
    this.setState({ open });
  };

  handleClose = () => {
    this.setState((prev) => ({ open: false, closeCount: prev.closeCount + 1 }));
  };

  render() {
    return (
      <Snackbar
        open={this.state.open}
        autoHideDuration={this.props.autoHideDuration}
        onClose={this.handleClose}
      >
        <SnackbarContent message="Test message" />
      </Snackbar>
    );
  }
}

describe('Snackbar', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  test('renders children when open', () => {
    render(
      <Snackbar open={true}>
        <SnackbarContent message="Test message" data-testid="snackbar-content" />
      </Snackbar>,
    );

    const content = document.querySelector('[data-testid="snackbar-content"]');
    expect(content).not.toBeNull();
    expect(content!.textContent).toContain('Test message');
  });

  test('renders content even when closed (for transition purposes)', () => {
    render(
      <Snackbar open={false}>
        <SnackbarContent message="Test message" data-testid="snackbar-content" />
      </Snackbar>,
    );

    // Content is still rendered (visibility controlled by CSS)
    const content = document.querySelector('[data-testid="snackbar-content"]');
    expect(content).not.toBeNull();
  });

  test('starts auto-hide timer when mounted with open=true', () => {
    const onClose = jest.fn();
    render(
      <Snackbar open={true} autoHideDuration={3000} onClose={onClose}>
        <SnackbarContent message="Test message" />
      </Snackbar>,
    );

    expect(onClose).not.toHaveBeenCalled();

    act(() => {
      jest.advanceTimersByTime(3000);
    });

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  test('starts auto-hide timer on transition from closed to open', () => {
    const ref = React.createRef<ControlledSnackbar>();
    render(<ControlledSnackbar ref={ref} autoHideDuration={3000} />);

    expect(ref.current!.state.closeCount).toBe(0);

    // Open the snackbar
    act(() => {
      ref.current!.setOpen(true);
    });

    act(() => {
      jest.advanceTimersByTime(3000);
    });

    expect(ref.current!.state.closeCount).toBe(1);
    expect(ref.current!.state.open).toBe(false);
  });

  test('clears timer when closed before timeout', () => {
    const ref = React.createRef<ControlledSnackbar>();
    render(<ControlledSnackbar ref={ref} autoHideDuration={5000} />);

    // Open the snackbar
    act(() => {
      ref.current!.setOpen(true);
    });

    // Advance part way through
    act(() => {
      jest.advanceTimersByTime(2000);
    });

    // Close before timeout
    act(() => {
      ref.current!.setOpen(false);
    });

    // Advance past original timeout
    act(() => {
      jest.advanceTimersByTime(5000);
    });

    // Should only have closed once (from manual close)
    expect(ref.current!.state.closeCount).toBe(0);
    expect(ref.current!.state.open).toBe(false);
  });

  test('handles rapid open/close cycles without race conditions', () => {
    const ref = React.createRef<ControlledSnackbar>();
    render(<ControlledSnackbar ref={ref} autoHideDuration={3000} />);

    // Rapidly toggle open/close
    act(() => {
      ref.current!.setOpen(true);
    });

    act(() => {
      jest.advanceTimersByTime(500);
    });

    act(() => {
      ref.current!.setOpen(false);
    });

    act(() => {
      ref.current!.setOpen(true);
    });

    act(() => {
      jest.advanceTimersByTime(500);
    });

    act(() => {
      ref.current!.setOpen(false);
    });

    act(() => {
      ref.current!.setOpen(true);
    });

    // Now advance through the full timeout
    act(() => {
      jest.advanceTimersByTime(3000);
    });

    // Should have triggered exactly one auto-close from the final open
    expect(ref.current!.state.closeCount).toBe(1);
  });

  test('does not start timer if no autoHideDuration', () => {
    const onClose = jest.fn();
    render(
      <Snackbar open={true} onClose={onClose}>
        <SnackbarContent message="Test message" />
      </Snackbar>,
    );

    act(() => {
      jest.advanceTimersByTime(10000);
    });

    expect(onClose).not.toHaveBeenCalled();
  });

  test('does not start timer if no onClose callback', () => {
    // This should not throw and should render fine
    render(
      <Snackbar open={true} autoHideDuration={3000}>
        <SnackbarContent message="Test message" />
      </Snackbar>,
    );

    act(() => {
      jest.advanceTimersByTime(5000);
    });

    // Just verify no error is thrown
    expect(true).toBe(true);
  });
});

describe('SnackbarContent', () => {
  test('renders message content', () => {
    render(<SnackbarContent message="Hello World" data-testid="content" />);
    const content = document.querySelector('[data-testid="content"]');
    expect(content!.textContent).toContain('Hello World');
  });

  test('renders action content', () => {
    render(
      <SnackbarContent
        message="Test"
        action={<button data-testid="action-button">Close</button>}
        data-testid="content"
      />,
    );
    const button = document.querySelector('[data-testid="action-button"]');
    expect(button).not.toBeNull();
    expect(button!.textContent).toBe('Close');
  });

  test('applies custom className', () => {
    render(<SnackbarContent message="Test" className="custom-class" data-testid="content" />);
    const content = document.querySelector('[data-testid="content"]');
    expect(content!.className).toContain('custom-class');
  });

  test('passes through aria-describedby', () => {
    render(<SnackbarContent message="Test" aria-describedby="my-description" data-testid="content" />);
    const content = document.querySelector('[data-testid="content"]');
    expect(content!.getAttribute('aria-describedby')).toBe('my-description');
  });

  test('filters out non-DOM props like onClose and variant', () => {
    // This should not throw a React warning about unknown DOM props
    const props: any = {
      message: 'Test',
      onClose: () => {},
      variant: 'error',
      'data-testid': 'content',
    };
    render(<SnackbarContent {...props} />);
    const content = document.querySelector('[data-testid="content"]');
    expect(content).not.toBeNull();
    // Verify onClose and variant are not passed to DOM
    expect(content!.getAttribute('onClose')).toBeNull();
    expect(content!.getAttribute('variant')).toBeNull();
  });
});
