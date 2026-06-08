// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, act } from '@testing-library/react';
import Snackbar, { SnackbarContent } from '../components/Snackbar';
import { Toast } from '../ErrorToast';

// Controlled wrapper to test open/close transitions. Exposes the same imperative
// surface the old class component did -- `setOpen` plus a live `state` object --
// so tests can drive and inspect it through a ref.
interface ControlledSnackbarHandle {
  setOpen: (open: boolean) => void;
  state: { open: boolean; closeCount: number };
}

const ControlledSnackbar = React.forwardRef<ControlledSnackbarHandle, { autoHideDuration?: number }>(
  function ControlledSnackbar({ autoHideDuration }, ref) {
    const [open, setOpen] = React.useState(false);
    const [closeCount, setCloseCount] = React.useState(0);

    const handleClose = () => {
      setOpen(false);
      setCloseCount((prev) => prev + 1);
    };

    React.useImperativeHandle(ref, () => ({ setOpen, state: { open, closeCount } }), [open, closeCount]);

    return (
      <Snackbar open={open} autoHideDuration={autoHideDuration}>
        <Toast message="Test message" onClose={handleClose} variant="info" />
      </Snackbar>
    );
  },
);

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
    const ref = React.createRef<ControlledSnackbarHandle>();
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
    const ref = React.createRef<ControlledSnackbarHandle>();
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
    const ref = React.createRef<ControlledSnackbarHandle>();
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
    interface ReRenderWrapperHandle {
      forceRerender: () => void;
      state: { counter: number; open: boolean; closeCount: number };
    }

    const ReRenderWrapper = React.forwardRef<ReRenderWrapperHandle, Record<string, never>>(
      function ReRenderWrapper(_props, ref) {
        const [counter, setCounter] = React.useState(0);
        const [open, setOpen] = React.useState(true);
        const [closeCount, setCloseCount] = React.useState(0);

        const forceRerender = () => {
          setCounter((prev) => prev + 1);
        };

        const handleClose = () => {
          setOpen(false);
          setCloseCount((prev) => prev + 1);
        };

        React.useImperativeHandle(ref, () => ({ forceRerender, state: { counter, open, closeCount } }), [
          counter,
          open,
          closeCount,
        ]);

        return (
          <Snackbar open={open} autoHideDuration={3000}>
            <Toast message={`Count: ${counter}`} onClose={handleClose} variant="info" />
          </Snackbar>
        );
      },
    );

    const ref = React.createRef<ReRenderWrapperHandle>();
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
    interface DurationWrapperHandle {
      setDuration: (duration: number) => void;
      state: { duration: number; open: boolean; closeCount: number };
    }

    const DurationWrapper = React.forwardRef<DurationWrapperHandle, Record<string, never>>(
      function DurationWrapper(_props, ref) {
        const [duration, setDuration] = React.useState(5000);
        const [open, setOpen] = React.useState(true);
        const [closeCount, setCloseCount] = React.useState(0);

        const handleClose = () => {
          setOpen(false);
          setCloseCount((prev) => prev + 1);
        };

        React.useImperativeHandle(ref, () => ({ setDuration, state: { duration, open, closeCount } }), [
          duration,
          open,
          closeCount,
        ]);

        return (
          <Snackbar open={open} autoHideDuration={duration}>
            <Toast message="Test message" onClose={handleClose} variant="info" />
          </Snackbar>
        );
      },
    );

    const ref = React.createRef<DurationWrapperHandle>();
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

  test('does not reset timer when message changes', () => {
    interface MessageWrapperHandle {
      setMessage: (message: string) => void;
      state: { message: string; open: boolean; closeCount: number };
    }

    const MessageWrapper = React.forwardRef<MessageWrapperHandle, Record<string, never>>(
      function MessageWrapper(_props, ref) {
        const [message, setMessage] = React.useState('First');
        const [open, setOpen] = React.useState(true);
        const [closeCount, setCloseCount] = React.useState(0);

        const handleClose = () => {
          setOpen(false);
          setCloseCount((prev) => prev + 1);
        };

        React.useImperativeHandle(ref, () => ({ setMessage, state: { message, open, closeCount } }), [
          message,
          open,
          closeCount,
        ]);

        return (
          <Snackbar open={open} autoHideDuration={3000}>
            <Toast message={message} onClose={handleClose} variant="info" />
          </Snackbar>
        );
      },
    );

    const ref = React.createRef<MessageWrapperHandle>();
    render(<MessageWrapper ref={ref} />);

    act(() => {
      jest.advanceTimersByTime(1000);
    });

    act(() => {
      ref.current!.setMessage('Second');
    });

    act(() => {
      jest.advanceTimersByTime(2000);
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

  test('renders and auto-hides with a noop onClose callback', () => {
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

  test('onClose reports the toast id, not the message, when an id is provided', () => {
    const onClose = jest.fn();
    render(
      <Snackbar open={true} autoHideDuration={3000}>
        <Toast message="duplicate" id={42} onClose={onClose} variant="warning" />
      </Snackbar>,
    );

    act(() => {
      jest.advanceTimersByTime(3000);
    });

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledWith(42);
  });

  test('closing one of two identical-message toasts leaves the other (dedup by id)', () => {
    // Mirrors Editor.getSnackbar / handleCloseSnackbar: two errors with the
    // SAME message text must be removed independently. Removal keys on a
    // per-toast id, not the message, so the first toast's auto-hide timer
    // dismisses only itself.
    interface Item {
      id: number;
      message: string;
    }

    interface DupHostHandle {
      state: { items: Item[] };
    }

    const DupHost = React.forwardRef<DupHostHandle, Record<string, never>>(function DupHost(_props, ref) {
      const [items, setItems] = React.useState<Item[]>([
        { id: 1, message: 'same error' },
        { id: 2, message: 'same error' },
      ]);

      const handleClose = (id: string | number) => {
        setItems((prev) => prev.filter((it) => it.id !== id));
      };

      React.useImperativeHandle(ref, () => ({ state: { items } }), [items]);

      return (
        <Snackbar open={items.length > 0} autoHideDuration={3000}>
          <div>
            {items.map((it) => (
              <Toast key={it.id} id={it.id} message={it.message} onClose={handleClose} variant="warning" />
            ))}
          </div>
        </Snackbar>
      );
    });

    const ref = React.createRef<DupHostHandle>();
    const { container } = render(<DupHost ref={ref} />);

    // Two toasts initially.
    expect(document.querySelectorAll('[id="client-snackbar"]').length).toBe(2);

    // Click the FIRST toast's close button only. Under the old
    // filter-by-message logic this removed both identical-message toasts;
    // keyed by id it removes only id 1.
    const closeButtons = container.querySelectorAll('button[aria-label="close"]');
    expect(closeButtons.length).toBe(2);
    act(() => {
      (closeButtons[0] as HTMLButtonElement).click();
    });

    // Exactly one error remains, and it is id 2 -- NOT both dismissed.
    expect(ref.current!.state.items).toEqual([{ id: 2, message: 'same error' }]);
    expect(document.querySelectorAll('[id="client-snackbar"]').length).toBe(1);
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
