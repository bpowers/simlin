// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { formatErrorReport } from '../error-report';

describe('formatErrorReport', () => {
  it('includes the message, timestamp, url, context, user agent, and both stacks', () => {
    const out = formatErrorReport({
      message: 'expected non-undefined object',
      stack: 'Error: expected non-undefined object\n    at defined (common.ts:16)',
      componentStack: '\n    at Canvas\n    at Editor',
      url: 'http://localhost:3000/bpowers/fooz',
      userAgent: 'test-agent',
      timestamp: '2026-06-29T00:00:00.000Z',
      context: { project: 'bpowers/fooz' },
    });

    expect(out).toContain('Error: expected non-undefined object');
    expect(out).toContain('Time: 2026-06-29T00:00:00.000Z');
    expect(out).toContain('URL: http://localhost:3000/bpowers/fooz');
    expect(out).toContain('project: bpowers/fooz');
    expect(out).toContain('User agent: test-agent');
    expect(out).toContain('Stack:');
    expect(out).toContain('at defined (common.ts:16)');
    expect(out).toContain('Component stack:');
    expect(out).toContain('at Canvas');
  });

  it('substitutes a placeholder for an empty message and omits absent optional fields', () => {
    const out = formatErrorReport({ message: '' });

    expect(out).toContain('(no message)');
    expect(out).not.toContain('URL:');
    expect(out).not.toContain('Time:');
    expect(out).not.toContain('User agent:');
    expect(out).not.toContain('Stack:');
    expect(out).not.toContain('Component stack:');
  });

  it('renders each context entry on its own line', () => {
    const out = formatErrorReport({
      message: 'boom',
      context: { project: 'bpowers/fooz', build: 'abc123' },
    });

    expect(out).toContain('project: bpowers/fooz');
    expect(out).toContain('build: abc123');
  });
});
