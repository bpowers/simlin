// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Minimal structured logger: one JSON object per line on stdout, in the
// same {level, message, timestamp} shape the previous winston
// combine(timestamp(), json()) configuration emitted, so existing
// log-based queries keep working. The whole logging surface the server
// uses is four level functions, which does not justify a logging
// framework's dependency tree.

export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

/**
 * Render one log line. Pure (timestamp injected) so tests can pin the
 * format. Errors render as their stack trace -- winston's json format
 * serialized Error instances as a lossy `{}`.
 */
export function formatLogEntry(level: LogLevel, message: unknown, timestamp: Date): string {
  const rendered = message instanceof Error ? (message.stack ?? message.message) : String(message);
  return JSON.stringify({ level, message: rendered, timestamp: timestamp.toISOString() });
}

function write(level: LogLevel, message: unknown): void {
  process.stdout.write(formatLogEntry(level, message, new Date()) + '\n');
}

export function debug(message: unknown): void {
  write('debug', message);
}

export function info(message: unknown): void {
  write('info', message);
}

export function warn(message: unknown): void {
  write('warn', message);
}

export function error(message: unknown): void {
  write('error', message);
}
