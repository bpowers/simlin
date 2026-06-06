// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import styles from './ErrorBoundary.module.css';

interface ErrorBoundaryProps {
  children: React.ReactNode;
  // When this value changes, the boundary clears any caught error and
  // re-renders its children. Hosts pass an identifier for the current
  // content (e.g. the project path) so navigating to a different, healthy
  // project recovers from a crash that was specific to the previous one.
  // Without this the fallback would stick forever once tripped.
  resetKey?: unknown;
}

interface ErrorBoundaryState {
  error: Error | undefined;
  // The resetKey value the current `error` was caught under, so
  // getDerivedStateFromProps can tell when a *new* resetKey arrives and
  // clear the error. Tracked in state because getDerivedStateFromProps is
  // static and has no access to previous props.
  lastResetKey: unknown;
}

// Last-resort React error boundary for the diagram editor. The Editor's own
// handlers wrap their async work in try/catch and surface contextual,
// user-readable messages (see openInitialProject / openEngineProject), which
// are strictly better than a generic boundary fallback. This boundary exists
// only to catch what those handlers cannot: a synchronous throw during the
// render phase of the Editor or one of its children. Without it such a throw
// unmounts the whole React tree and leaves the host with a blank page.
export class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: undefined, lastResetKey: this.props.resetKey };

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error };
  }

  static getDerivedStateFromProps(
    props: ErrorBoundaryProps,
    state: ErrorBoundaryState,
  ): Partial<ErrorBoundaryState> | null {
    // A changed resetKey means the host swapped to different content; drop
    // any caught error so the new children get a chance to render.
    if (props.resetKey !== state.lastResetKey) {
      return { error: undefined, lastResetKey: props.resetKey };
    }
    return null;
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    // Log the error and component stack so the failure is diagnosable even
    // though we render a minimal fallback to the user.
    console.error('ErrorBoundary caught an error during render:', error, info.componentStack);
  }

  render(): React.ReactNode {
    const { error } = this.state;
    if (error) {
      return (
        <div className={styles.container} role="alert">
          <div className={styles.box}>
            <p className={styles.title}>Something went wrong</p>
            <p className={styles.message}>{error.message || 'An unexpected error occurred.'}</p>
            <p className={styles.hint}>Try reloading the page. If the problem persists, the model may be invalid.</p>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
