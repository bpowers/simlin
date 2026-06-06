// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import styles from './ErrorBoundary.module.css';

interface ErrorBoundaryProps {
  children: React.ReactNode;
}

interface ErrorBoundaryState {
  error: Error | undefined;
}

// Last-resort React error boundary for the diagram editor. The Editor's own
// handlers wrap their async work in try/catch and surface contextual,
// user-readable messages (see openInitialProject / openEngineProject), which
// are strictly better than a generic boundary fallback. This boundary exists
// only to catch what those handlers cannot: a synchronous throw during the
// render phase of the Editor or one of its children. Without it such a throw
// unmounts the whole React tree and leaves the host with a blank page.
export class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: undefined };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
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
