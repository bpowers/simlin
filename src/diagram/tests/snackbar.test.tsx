// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { render, act } from '@testing-library/react';
import Snackbar, { SnackbarContent } from '../components/Snackbar';
import { Toast } from '../ErrorToast';

describe('Snackbar', () => {
  beforeEach(() => {
    rs.useFakeTimers();
  });

  afterEach(() => {
    rs.useRealTimers();
  });

  test('renders children when open', () => {
    render(
      <Snackbar open={true}>
        <Toast message="Test message" onClose={rs.fn()} variant="info" />
      </Snackbar>,
    );

    const content = document.querySelector('[id="client-snackbar"]');
    expect(content).not.toBeNull();
    expect(content!.textContent).toContain('Test message');
  });

  test('does not render children when closed', () => {
    render(
      <Snackbar open={false}>
        <Toast message="Test message" onClose={rs.fn()} variant="info" />
      </Snackbar>,
    );

    const content = document.querySelector('[id="client-snackbar"]');
    expect(content).toBeNull();
  });

  // The auto-hide TIMER lives in Toast, and error-toast.test.tsx covers its
  // mechanics (boundary, unmount cleanup, which re-renders restart it) by
  // supplying SnackbarDurationContext by hand. That bypasses Snackbar, so these
  // two rows stay: they are the only coverage of Snackbar publishing
  // `autoHideDuration` into that context, in both the present and absent arms.
  test('auto-hides when duration is provided', () => {
    const onClose = rs.fn();
    render(
      <Snackbar open={true} autoHideDuration={3000}>
        <Toast message="Test message" onClose={onClose} variant="info" />
      </Snackbar>,
    );

    expect(onClose).not.toHaveBeenCalled();

    act(() => {
      rs.advanceTimersByTime(3000);
    });

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  test('does not auto-hide when duration is omitted', () => {
    const onClose = rs.fn();
    render(
      <Snackbar open={true}>
        <Toast message="Test message" onClose={onClose} variant="info" />
      </Snackbar>,
    );

    act(() => {
      rs.advanceTimersByTime(10000);
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
      rs.advanceTimersByTime(5000);
    });

    const content = document.querySelector('[id="client-snackbar"]');
    expect(content).toBeNull();
  });

  test('onClose reports the toast id, not the message, when an id is provided', () => {
    const onClose = rs.fn();
    render(
      <Snackbar open={true} autoHideDuration={3000}>
        <Toast message="duplicate" id={42} onClose={onClose} variant="warning" />
      </Snackbar>,
    );

    act(() => {
      rs.advanceTimersByTime(3000);
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
