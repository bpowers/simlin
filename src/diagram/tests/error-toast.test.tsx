// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { render, act } from '@testing-library/react';
import * as RadixToast from '@radix-ui/react-toast';

import { Toast } from '../ErrorToast';
import { SnackbarDurationContext } from '../components/Snackbar';

// Toast reads its auto-hide duration from SnackbarDurationContext and renders a
// RadixToast.Root, so it needs both a RadixToast.Provider and the duration
// context above it. Drive `duration` directly (rather than through Snackbar) so
// each test controls the exact value the timer effect keys on -- the most
// conversion-sensitive behavior in the batch.
function renderToast(props: {
  duration: number | undefined;
  message?: string;
  onClose?: (id: string | number) => void;
  id?: string | number;
}) {
  const { duration, message = 'boom', onClose = rs.fn(), id } = props;
  return render(
    <RadixToast.Provider duration={2147483647}>
      <SnackbarDurationContext.Provider value={duration}>
        <Toast message={message} onClose={onClose} variant="warning" id={id} />
      </SnackbarDurationContext.Provider>
      <RadixToast.Viewport />
    </RadixToast.Provider>,
  );
}

describe('Toast', () => {
  beforeEach(() => {
    rs.useFakeTimers();
  });

  afterEach(() => {
    rs.useRealTimers();
  });

  test('auto-hides: onClose fires after the duration elapses', () => {
    const onClose = rs.fn();
    renderToast({ duration: 3000, onClose });

    act(() => {
      rs.advanceTimersByTime(2999);
    });
    expect(onClose).not.toHaveBeenCalled();

    act(() => {
      rs.advanceTimersByTime(1);
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  test('reports the provided id (not the message) to onClose', () => {
    const onClose = rs.fn();
    renderToast({ duration: 1000, onClose, id: 7, message: 'dup' });
    act(() => {
      rs.advanceTimersByTime(1000);
    });
    expect(onClose).toHaveBeenCalledWith(7);
  });

  test('falls back to the message as the close id when none is provided', () => {
    const onClose = rs.fn();
    renderToast({ duration: 1000, onClose, message: 'fallback-id' });
    act(() => {
      rs.advanceTimersByTime(1000);
    });
    expect(onClose).toHaveBeenCalledWith('fallback-id');
  });

  test('undefined duration sets no timer (never auto-hides)', () => {
    const onClose = rs.fn();
    renderToast({ duration: undefined, onClose });
    act(() => {
      rs.advanceTimersByTime(1_000_000);
    });
    expect(onClose).not.toHaveBeenCalled();
  });

  test('cleans up the pending timer on unmount (no leak, no late fire)', () => {
    const onClose = rs.fn();
    const { unmount } = renderToast({ duration: 3000, onClose });

    act(() => {
      rs.advanceTimersByTime(1000);
    });
    unmount();
    // The auto-hide timer was scheduled but not yet fired; the effect's cleanup
    // must clear it so it never fires against the unmounted tree. Advancing well
    // past the original deadline must therefore produce no onClose. (We assert
    // on onClose rather than rs.getTimerCount(): RadixToast.Provider keeps its
    // own effectively-infinite internal timer alive, so the global timer count
    // is not a clean signal for Toast's timer specifically.)
    act(() => {
      rs.advanceTimersByTime(5000);
    });
    expect(onClose).not.toHaveBeenCalled();
  });

  test('a re-render that only changes `message` does NOT restart the timer', () => {
    // If the timer effect over-depended on `message` (or on the recreated
    // onClose handler), a message change at t=1000 would reset the countdown
    // and onClose would not fire until t=4000. Keyed only on [open, duration]
    // it fires at the original t=3000.
    const onClose = rs.fn();
    function Host({ message }: { message: string }): React.ReactElement {
      return (
        <RadixToast.Provider duration={2147483647}>
          <SnackbarDurationContext.Provider value={3000}>
            <Toast message={message} onClose={onClose} variant="warning" />
          </SnackbarDurationContext.Provider>
          <RadixToast.Viewport />
        </RadixToast.Provider>
      );
    }

    const { rerender } = render(<Host message="first" />);
    act(() => {
      rs.advanceTimersByTime(1000);
    });
    rerender(<Host message="second" />);
    // 2000ms more reaches the ORIGINAL 3000ms deadline; if the timer had
    // restarted, nothing would have fired yet.
    act(() => {
      rs.advanceTimersByTime(2000);
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  test('changing the context duration while open DOES restart the timer', () => {
    const onClose = rs.fn();
    function Host({ duration }: { duration: number }): React.ReactElement {
      return (
        <RadixToast.Provider duration={2147483647}>
          <SnackbarDurationContext.Provider value={duration}>
            <Toast message="boom" onClose={onClose} variant="warning" />
          </SnackbarDurationContext.Provider>
          <RadixToast.Viewport />
        </RadixToast.Provider>
      );
    }

    const { rerender } = render(<Host duration={5000} />);
    act(() => {
      rs.advanceTimersByTime(1000);
    });
    // Shorten the duration to 1000ms. The effect must clear the old 5000ms
    // timer and schedule a fresh 1000ms one from now.
    rerender(<Host duration={1000} />);
    act(() => {
      rs.advanceTimersByTime(999);
    });
    expect(onClose).not.toHaveBeenCalled();
    act(() => {
      rs.advanceTimersByTime(1);
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
