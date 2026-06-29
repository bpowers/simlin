// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { formatErrorReport } from './error-report';
import styles from './ErrorBoundary.module.css';

interface ErrorBoundaryProps {
  children: React.ReactNode;
  // When this value changes, the boundary clears any caught error and
  // re-renders its children. Hosts pass an identifier for the current
  // content (e.g. the project path) so navigating to a different, healthy
  // project recovers from a crash that was specific to the previous one.
  // Without this the fallback would stick forever once tripped.
  resetKey?: unknown;
  // Host-supplied key/value context folded into the copyable error report
  // (e.g. the project path). Stays optional so the boundary remains a
  // standalone, host-agnostic component.
  context?: Readonly<Record<string, string>>;
}

interface ErrorBoundaryState {
  error: Error | undefined;
  // React's component stack from componentDidCatch, surfaced in the copyable
  // report so a pasted bug carries where in the tree it threw.
  componentStack: string | undefined;
  // Set once "Copy details" succeeds, so the button can confirm the copy.
  copied: boolean;
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
  state: ErrorBoundaryState = {
    error: undefined,
    componentStack: undefined,
    copied: false,
    lastResetKey: this.props.resetKey,
  };

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
      return { error: undefined, componentStack: undefined, copied: false, lastResetKey: props.resetKey };
    }
    return null;
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    // Log the error and component stack so the failure is diagnosable even
    // though we render a minimal fallback to the user.
    console.error('ErrorBoundary caught an error during render:', error, info.componentStack);
    this.setState({ componentStack: info.componentStack ?? undefined });
  }

  // Build the copyable report from the caught error plus environment details
  // read at copy time. Kept out of render (formatErrorReport is the pure part)
  // so the timestamp/url/userAgent reads stay in the imperative shell.
  private buildReport(): string {
    const { error, componentStack } = this.state;
    return formatErrorReport({
      message: error?.message ?? '',
      stack: error?.stack,
      componentStack,
      url: typeof window !== 'undefined' ? window.location.href : undefined,
      userAgent: typeof navigator !== 'undefined' ? navigator.userAgent : undefined,
      timestamp: new Date().toISOString(),
      context: this.props.context,
    });
  }

  private handleCopy = (): void => {
    const report = this.buildReport();
    const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined;
    if (!clipboard?.writeText) {
      return;
    }
    // Fire-and-forget: a clipboard rejection (permissions, insecure context)
    // must not throw out of the click handler.
    clipboard.writeText(report).then(
      () => this.setState({ copied: true }),
      () => {
        /* clipboard denied; leave the button unchanged */
      },
    );
  };

  render(): React.ReactNode {
    const { error, copied } = this.state;
    if (error) {
      return (
        <div className={styles.container} role="alert">
          <div className={styles.box}>
            <p className={styles.title}>Something went wrong</p>
            <p className={styles.message}>{error.message || 'An unexpected error occurred.'}</p>
            <p className={styles.hint}>Try reloading the page. If the problem persists, the model may be invalid.</p>
            <div className={styles.actions}>
              <button type="button" className={styles.copyButton} onClick={this.handleCopy}>
                {copied ? 'Copied' : 'Copy details'}
              </button>
            </div>
            <details className={styles.details}>
              <summary className={styles.detailsSummary}>Technical details</summary>
              <pre className={styles.stack}>{this.buildReport()}</pre>
            </details>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
