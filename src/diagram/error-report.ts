// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Pure formatting of a client-side error report into a single human-readable
 * text blob -- the thing the ErrorBoundary's "Copy details" button puts on the
 * clipboard (and, eventually, the body a reporting endpoint would POST). Kept
 * pure (no `Date`, no `window`) so it is trivially testable; the imperative
 * shell (ErrorBoundary) supplies the timestamp, url, and user agent.
 */
export interface ErrorReport {
  /** The thrown error's message. */
  readonly message: string;
  /** The error's JS stack, if any. */
  readonly stack?: string;
  /** React's component stack captured in componentDidCatch, if any. */
  readonly componentStack?: string;
  /** The page URL at the time of the error. */
  readonly url?: string;
  /** navigator.userAgent at the time of the error. */
  readonly userAgent?: string;
  /** ISO timestamp the report was generated. */
  readonly timestamp?: string;
  /** Host-supplied key/value context (e.g. project path, build revision). */
  readonly context?: Readonly<Record<string, string>>;
}

/**
 * Format an `ErrorReport` as plain text. The order is fixed (summary fields,
 * then the JS stack, then the component stack) so copied reports are
 * consistent. Absent optional fields are omitted entirely rather than rendered
 * as empty lines.
 */
export function formatErrorReport(report: ErrorReport): string {
  const lines: string[] = [];

  lines.push(`Error: ${report.message || '(no message)'}`);
  if (report.timestamp) {
    lines.push(`Time: ${report.timestamp}`);
  }
  if (report.url) {
    lines.push(`URL: ${report.url}`);
  }
  if (report.context) {
    for (const [key, value] of Object.entries(report.context)) {
      lines.push(`${key}: ${value}`);
    }
  }
  if (report.userAgent) {
    lines.push(`User agent: ${report.userAgent}`);
  }
  if (report.stack) {
    lines.push('', 'Stack:', report.stack);
  }
  if (report.componentStack) {
    lines.push('', 'Component stack:', report.componentStack.trim());
  }

  return lines.join('\n');
}
