// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Pins the API-LINE log format -- specifically the user="..." suffix read
// out of the session -- so the hand-rolled passport shape walk can be
// consolidated onto getSessionUserId without changing what gets logged.

import { describe, it, expect, beforeEach, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

import express from 'express';
import http from 'http';

import * as logger from '../logger';
import { requestLogger } from '../request-logger';

// { spy: true } for the same CommonJS reasons as authn.test.ts.
rs.mock('../logger', { spy: true });

const infoLogMock = logger.info as unknown as Mock<typeof logger.info>;

beforeEach(() => {
  infoLogMock.mockReset();
  infoLogMock.mockImplementation(() => {});
});

function get(server: http.Server, path: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === 'string') {
      return reject(new Error('server not listening'));
    }
    http.get({ hostname: '127.0.0.1', port: addr.port, path }, (res) => {
      res.resume();
      res.on('end', () => resolve(res.statusCode ?? 0));
    });
  });
}

function apiLines(): string[] {
  return infoLogMock.mock.calls.map((call) => String(call[0])).filter((line) => line.startsWith('API-LINE'));
}

async function requestWithSession(session: Record<string, unknown> | undefined): Promise<string> {
  const app = express();
  app.use((req, _res, next) => {
    req.session = session as express.Request['session'];
    next();
  });
  app.use(requestLogger);
  app.get('/hello', (_req, res) => {
    res.sendStatus(200);
  });

  const server = app.listen(0);
  try {
    expect(await get(server, '/hello')).toBe(200);
  } finally {
    server.closeIdleConnections();
    await new Promise<void>((resolve, reject) => {
      server.close((err) => (err ? reject(err) : resolve()));
    });
  }

  const lines = apiLines();
  expect(lines.length).toBe(1);
  return lines[0];
}

describe('requestLogger', () => {
  it('appends user="..." for an authenticated session', async () => {
    const line = await requestWithSession({ passport: { user: { id: 'alice' } } });
    expect(line).toContain('status=200');
    expect(line).toContain('method="GET"');
    expect(line).toContain('path="/hello"');
    expect(line).toContain(' user="alice"');
  });

  it('omits the user suffix for an anonymous request', async () => {
    const line = await requestWithSession({});
    expect(line).toContain('status=200');
    expect(line).not.toContain(' user=');
  });

  it('tolerates a malformed session shape', async () => {
    const line = await requestWithSession({ passport: 'junk' });
    expect(line).toContain('status=200');
    expect(line).not.toContain(' user=');
  });

  it('tolerates a missing session (requestLogger sits before seshcookie in app.ts)', async () => {
    const line = await requestWithSession(undefined);
    expect(line).toContain('status=200');
    expect(line).not.toContain(' user=');
  });
});
