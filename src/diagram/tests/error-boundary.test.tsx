// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';

import { ErrorBoundary } from '../ErrorBoundary';

function Boom({ message }: { message: string }): React.ReactElement {
  throw new Error(message);
}

describe('ErrorBoundary', () => {
  let consoleErrorSpy: jest.SpyInstance;

  beforeEach(() => {
    // React logs the caught error (and our componentDidCatch logs too).
    // Silence both so the expected error doesn't clutter test output.
    consoleErrorSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    consoleErrorSpy.mockRestore();
  });

  it('renders the fallback (not a propagated throw) when a child throws during render', () => {
    expect(() => {
      render(
        <ErrorBoundary>
          <Boom message="kaboom in render" />
        </ErrorBoundary>,
      );
    }).not.toThrow();

    const alert = screen.getByRole('alert');
    expect(alert).not.toBeNull();
    expect(alert.textContent).toContain('Something went wrong');
    expect(alert.textContent).toContain('kaboom in render');
    // componentDidCatch logged the error.
    expect(consoleErrorSpy).toHaveBeenCalled();
  });

  it('renders normal children unchanged when nothing throws', () => {
    render(
      <ErrorBoundary>
        <div data-testid="child">all good</div>
      </ErrorBoundary>,
    );

    const child = screen.getByTestId('child');
    expect(child.textContent).toBe('all good');
    // No fallback alert should be present.
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('falls back to a generic message when the error has no message', () => {
    function BoomEmpty(): React.ReactElement {
      throw new Error('');
    }

    render(
      <ErrorBoundary>
        <BoomEmpty />
      </ErrorBoundary>,
    );

    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('An unexpected error occurred.');
  });

  it('clears the caught error and re-renders children when resetKey changes', () => {
    // A boundary that latched onto a crash for project A must recover when
    // the host navigates to a healthy project B (resetKey change). Without
    // the reset the fallback would stick forever.
    function Maybe({ crash }: { crash: boolean }): React.ReactElement {
      if (crash) {
        throw new Error('project A is broken');
      }
      return <div data-testid="healthy">project B is fine</div>;
    }

    const { rerender } = render(
      <ErrorBoundary resetKey="A">
        <Maybe crash={true} />
      </ErrorBoundary>,
    );

    // Project A tripped the boundary.
    expect(screen.getByRole('alert').textContent).toContain('project A is broken');

    // Navigate to project B: resetKey changes and children stop throwing.
    rerender(
      <ErrorBoundary resetKey="B">
        <Maybe crash={false} />
      </ErrorBoundary>,
    );

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByTestId('healthy').textContent).toBe('project B is fine');
  });

  it('keeps showing the fallback when resetKey is unchanged', () => {
    // A re-render with the same resetKey (e.g. an unrelated parent update)
    // must not clear a real error.
    function Boomer(): React.ReactElement {
      throw new Error('still broken');
    }

    const { rerender } = render(
      <ErrorBoundary resetKey="A">
        <Boomer />
      </ErrorBoundary>,
    );

    expect(screen.getByRole('alert').textContent).toContain('still broken');

    rerender(
      <ErrorBoundary resetKey="A">
        <Boomer />
      </ErrorBoundary>,
    );

    expect(screen.getByRole('alert').textContent).toContain('still broken');
  });
});
