// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { formatLogEntry } from '../logger';

describe('formatLogEntry', () => {
  const when = new Date('2026-06-04T12:34:56.789Z');

  it('emits the winston-compatible {level, message, timestamp} JSON shape', () => {
    const line = formatLogEntry('info', 'hello world', when);
    expect(JSON.parse(line)).toEqual({
      level: 'info',
      message: 'hello world',
      timestamp: '2026-06-04T12:34:56.789Z',
    });
  });

  it('renders an Error as its stack trace', () => {
    const err = new Error('boom');
    const parsed = JSON.parse(formatLogEntry('error', err, when)) as { message: string };
    expect(parsed.message).toContain('boom');
    // a stack trace, not the lossy {} winston produced for Error instances
    expect(parsed.message).toContain('logger.test');
  });

  it('stringifies non-string, non-Error values', () => {
    const parsed = JSON.parse(formatLogEntry('warn', 42, when)) as { message: string };
    expect(parsed.message).toBe('42');
  });

  it('always produces a single line', () => {
    const line = formatLogEntry('info', 'a\nb', when);
    expect(line).not.toContain('\n');
  });
});
