// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// End-to-end coverage for the passport-free session machinery: the
// session rides in a seshcookie-encrypted cookie under the historic
// `session.passport.user.id` shape (wire-compatible with sessions
// minted while passport was a dependency), sessionAuth() deserializes
// the user onto req.user, and DELETE /session clears the cookie.

import express from 'express';
import cookieParser from 'cookie-parser';
import { seshcookie } from '../seshcookie/seshcookie';
import http from 'http';

import { handleSessionDelete } from '../auth-helpers';
import { getSessionUserId, sessionAuth, setSessionUser } from '../session-auth';

interface FakeUser {
  getId(): string;
}

function fakeUser(id: string): FakeUser {
  return { getId: () => id };
}

// Minimal stand-in for the user Table: only findOne is consulted by
// sessionAuth.
function fakeUserTable(known: Record<string, FakeUser>) {
  return {
    findOne: (id: string): Promise<FakeUser | undefined> => Promise.resolve(known[id]),
  };
}

function createTestApp(): express.Express {
  const app = express();

  app.use(cookieParser());
  app.use(
    seshcookie({
      key: 'test-key-for-encryption-1234',
      cookieName: 'test_session',
      cookiePath: '/',
      httpOnly: true,
      secure: false,
    }),
  );

  const users = fakeUserTable({ 'test-user': fakeUser('test-user') });
  app.use(sessionAuth(users));

  app.post('/login', (req, res) => {
    setSessionUser(req, 'test-user');
    res.status(200).json({ ok: true });
  });

  app.delete('/session', handleSessionDelete);

  app.get('/check', (req, res) => {
    const sessionUserId = getSessionUserId(req);
    const user = req.user as FakeUser | undefined;
    if (sessionUserId && user) {
      res.status(200).json({ sessionUserId, userId: user.getId() });
    } else {
      res.status(401).json({ error: 'not authenticated' });
    }
  });

  return app;
}

function request(
  server: http.Server,
  method: string,
  path: string,
  headers?: Record<string, string>,
): Promise<{ status: number; body: unknown; headers: http.IncomingHttpHeaders }> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === 'string') {
      return reject(new Error('server not listening'));
    }
    const req = http.request(
      {
        hostname: '127.0.0.1',
        port: addr.port,
        path,
        method,
        headers,
      },
      (res) => {
        let data = '';
        res.on('data', (chunk) => (data += chunk));
        res.on('end', () => {
          let body: unknown;
          try {
            body = JSON.parse(data) as unknown;
          } catch {
            body = data;
          }
          resolve({ status: res.statusCode ?? 0, body, headers: res.headers });
        });
      },
    );
    req.on('error', reject);
    req.end();
  });
}

describe('seshcookie-backed session auth', () => {
  it('login mints a session cookie that authenticates subsequent requests', async () => {
    const app = createTestApp();
    const server = app.listen(0);
    try {
      const loginRes = await request(server, 'POST', '/login');
      expect(loginRes.status).toBe(200);

      const cookies = loginRes.headers['set-cookie'];
      expect(cookies).toBeDefined();
      const cookieHeader = Array.isArray(cookies) ? cookies.join('; ') : String(cookies);

      const checkRes = await request(server, 'GET', '/check', { cookie: cookieHeader });
      expect(checkRes.status).toBe(200);
      expect(checkRes.body).toEqual({ sessionUserId: 'test-user', userId: 'test-user' });
    } finally {
      server.close();
    }
  });

  it('an unauthenticated request has no session user and no req.user', async () => {
    const app = createTestApp();
    const server = app.listen(0);
    try {
      const res = await request(server, 'GET', '/check');
      expect(res.status).toBe(401);
    } finally {
      server.close();
    }
  });

  it('a session naming an unknown user does not populate req.user', async () => {
    const app = express();
    app.use(cookieParser());
    app.use(
      seshcookie({
        key: 'test-key-for-encryption-1234',
        cookieName: 'test_session',
        cookiePath: '/',
        httpOnly: true,
        secure: false,
      }),
    );
    app.use(sessionAuth(fakeUserTable({})));
    app.post('/login', (req, res) => {
      setSessionUser(req, 'deleted-user');
      res.sendStatus(200);
    });
    app.get('/check', (req, res) => {
      res.status(req.user ? 200 : 401).end();
    });

    const server = app.listen(0);
    try {
      const loginRes = await request(server, 'POST', '/login');
      const cookies = loginRes.headers['set-cookie'];
      const cookieHeader = Array.isArray(cookies) ? cookies.join('; ') : String(cookies);
      const checkRes = await request(server, 'GET', '/check', { cookie: cookieHeader });
      expect(checkRes.status).toBe(401);
    } finally {
      server.close();
    }
  });

  it('DELETE /session logs the user out and clears the session cookie', async () => {
    const app = createTestApp();
    const server = app.listen(0);
    try {
      const loginRes = await request(server, 'POST', '/login');
      expect(loginRes.status).toBe(200);
      const loginCookies = loginRes.headers['set-cookie'];
      const loginCookie = Array.isArray(loginCookies) ? loginCookies.join('; ') : String(loginCookies);

      // Sanity: the session authenticates before logout.
      const before = await request(server, 'GET', '/check', { cookie: loginCookie });
      expect(before.status).toBe(200);

      // Logout must respond and must rewrite the seshcookie session
      // cookie without the user.
      const logoutRes = await request(server, 'DELETE', '/session', { cookie: loginCookie });
      expect(logoutRes.status).toBe(200);
      const logoutCookies = logoutRes.headers['set-cookie'];
      expect(logoutCookies).toBeDefined();
      const logoutCookie = Array.isArray(logoutCookies) ? logoutCookies.join('; ') : String(logoutCookies);

      const after = await request(server, 'GET', '/check', { cookie: logoutCookie });
      expect(after.status).toBe(401);
    } finally {
      server.close();
    }
  });

  it('reads the historic passport session wire shape', () => {
    // Sessions minted before the passport dependency was removed store
    // {passport: {user: {id}}}; getSessionUserId must keep reading that
    // exact shape so existing logins survive the upgrade.
    const req = { session: { passport: { user: { id: 'legacy-user' } } } };
    expect(getSessionUserId(req as unknown as express.Request)).toBe('legacy-user');

    const empty = { session: {} };
    expect(getSessionUserId(empty as unknown as express.Request)).toBeUndefined();

    const malformed = { session: { passport: { user: { id: 42 } } } };
    expect(getSessionUserId(malformed as unknown as express.Request)).toBeUndefined();
  });
});
