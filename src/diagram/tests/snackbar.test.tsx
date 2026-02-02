// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, act } from '@testing-library/react';
import Snackbar, { SnackbarContent } from '../components/Snackbar';
import { Toast } from '../ErrorToast';

// Controlled wrapper to test open/close transitions
class ControlledSnackbar extends React.Component<{ autoHideDuration?: number }, { open: boolean; closeCount: number }> {
  state = { open: false, closeCount: 0 };

  setOpen = (open: boolean) => {
    this.setState({ open });
  };

  handleClose = () => {
    this.setState((prev) => ({ open: false, closeCount: prev.closeCount + 1 }));
  };

  render() {
    return (
      <Snackbar open={this.state.open} autoHideDuration={this.props.autoHideDuration}>
        <Toast message="Test message" onClose={this.handleClose} variant="info" />
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
        <Toast message="Test message" onClose={jest.fn()} variant="info" />
      </Snackbar>,
    );

    const content = document.querySelector('[id="client-snackbar"]');
    expect(content).not.toBeNull();
    expect(content!.textContent).toContain('Test message');
  });

  test('does not render children when closed', () => {
    render(
      <Snackbar open={false}>
        <Toast message="Test message" onClose={jest.fn()} variant="info" />
      </Snackbar>,
    );

    const content = document.querySelector('[id="client-snackbar"]');
    expect(content).toBeNull();
  });

  test('auto-hides when duration is provided', () => {
    const onClose = jest.fn();
    render(
      <Snackbar open={true} autoHideDuration={3000}>
        <Toast message="Test message" onClose={onClose} variant="info" />
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

    act(() => {
      ref.current!.setOpen(true);
    });

    act(() => {
      jest.advanceTimersByTime(2000);
    });

    act(() => {
      ref.current!.setOpen(false);
    });

    act(() => {
      jest.advanceTimersByTime(5000);
    });

    expect(ref.current!.state.closeCount).toBe(0);
    expect(ref.current!.state.open).toBe(false);
  });

  test('handles rapid open/close cycles without race conditions', () => {
    const ref = React.createRef<ControlledSnackbar>();
    render(<ControlledSnackbar ref={ref} autoHideDuration={3000} />);

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

    act(() => {
      jest.advanceTimersByTime(3000);
    });

    expect(ref.current!.state.closeCount).toBe(1);
  });

  test('does not reset timer on unrelated re-renders', () => {
    class ReRenderWrapper extends React.Component<
      Record<string, never>,
      { counter: number; open: boolean; closeCount: number }
    > {
      state = { counter: 0, open: true, closeCount: 0 };

      forceRerender = () => {
        this.setState((prev) => ({ counter: prev.counter + 1 }));
      };

      handleClose = () => {
        this.setState((prev) => ({ open: false, closeCount: prev.closeCount + 1 }));
      };

      render() {
        return (
          <Snackbar open={this.state.open} autoHideDuration={3000}>
            <Toast message={`Count: ${this.state.counter}`} onClose={this.handleClose} variant="info" />
          </Snackbar>
        );
      }
    }

    const ref = React.createRef<ReRenderWrapper>();
    render(<ReRenderWrapper ref={ref} />);

    act(() => {
      jest.advanceTimersByTime(1000);
    });

    act(() => {
      ref.current!.forceRerender();
    });

    act(() => {
      jest.advanceTimersByTime(1000);
    });

    act(() => {
      ref.current!.forceRerender();
    });

    act(() => {
      jest.advanceTimersByTime(1000);
    });

    expect(ref.current!.state.closeCount).toBe(1);
  });

  test('restarts timer when duration changes while open', () => {
    class DurationWrapper extends React.Component<
      Record<string, never>,
      { duration: number; open: boolean; closeCount: number }
    > {
      state = { duration: 5000, open: true, closeCount: 0 };

      setDuration = (duration: number) => {
        this.setState({ duration });
      };

      handleClose = () => {
        this.setState((prev) => ({ open: false, closeCount: prev.closeCount + 1 }));
      };

      render() {
        return (
          <Snackbar open={this.state.open} autoHideDuration={this.state.duration}>
            <Toast message="Test message" onClose={this.handleClose} variant="info" />
          </Snackbar>
        );
      }
    }

    const ref = React.createRef<DurationWrapper>();
    render(<DurationWrapper ref={ref} />);

    act(() => {
      jest.advanceTimersByTime(1000);
    });

    act(() => {
      ref.current!.setDuration(1000);
    });

    act(() => {
      jest.advanceTimersByTime(900);
    });

    expect(ref.current!.state.closeCount).toBe(0);

    act(() => {
      jest.advanceTimersByTime(200);
    });

    expect(ref.current!.state.closeCount).toBe(1);
  });

  test('does not auto-hide when duration is omitted', () => {
    const onClose = jest.fn();
    render(
      <Snackbar open={true}>
        <Toast message="Test message" onClose={onClose} variant="info" />
      </Snackbar>,
    );

    act(() => {
      jest.advanceTimersByTime(10000);
    });

    expect(onClose).not.toHaveBeenCalled();
  });

  test('renders without errors when no onClose callback is provided', () => {
    render(
      <Snackbar open={true} autoHideDuration={3000}>
        <Toast message="Test message" onClose={() => {}} variant="info" />
      </Snackbar>,
    );

    const initialContent = document.querySelector('[id="client-snackbar"]');
    expect(initialContent).not.toBeNull();

    act(() => {
      jest.advanceTimersByTime(5000);
    });

    const content = document.querySelector('[id="client-snackbar"]');
    expect(content).toBeNull();
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
    const props = {
      message: 'Test',
      onClose: () => {},
      variant: 'error',
      'data-testid': 'content',
    } as React.ComponentProps<typeof SnackbarContent> & { onClose: () => void; variant: string };
    render(<SnackbarContent {...props} />);
    const content = document.querySelector('[data-testid="content"]');
    expect(content).not.toBeNull();
    // Verify onClose and variant are not passed to DOM
    expect(content!.getAttribute('onClose')).toBeNull();
    expect(content!.getAttribute('variant')).toBeNull();
  });
});
