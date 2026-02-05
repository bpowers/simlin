// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import express from 'express';
import cookieParser from 'cookie-parser';
import passport from 'passport';
import { Strategy as BaseStrategy } from 'passport-strategy';
import { seshcookie } from 'seshcookie';
import http from 'http';

// Minimal passport strategy that always succeeds with a test user.
class TestStrategy extends BaseStrategy implements passport.Strategy {
  readonly name = 'test';

  authenticate(_req: express.Request): void {
    this.success({ id: 'test-user' });
  }
}

function createTestApp(options?: { addSessionCompat?: boolean }): express.Express {
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

  if (options?.addSessionCompat) {
    app.use((req: express.Request, _res: express.Response, next: express.NextFunction) => {
      const addSessionMethods = (session: Record<string, any>): void => {
        session.regenerate = (cb: (err?: any) => void) => {
          req.session = {};
          addSessionMethods(req.session);
          cb();
        };
        session.save = (cb: (err?: any) => void) => {
          cb();
        };
      };
      addSessionMethods(req.session);
      next();
    });
  }

  passport.use(new TestStrategy());
  passport.serializeUser((user: any, done) => done(undefined, user));
  passport.deserializeUser((user: any, done) => done(undefined, user));

  app.use(passport.initialize());
  app.use(passport.session());

  app.post('/login', passport.authenticate('test'), (_req, res) => {
    res.status(200).json({ ok: true });
  });

  app.get('/check', (req, res) => {
    if (req.session?.passport?.user) {
      res.status(200).json({ user: req.session.passport.user });
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
): Promise<{ status: number; body: any; headers: http.IncomingHttpHeaders }> {
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
          let body: any;
          try {
            body = JSON.parse(data);
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

describe('seshcookie + passport session compatibility', () => {
  it('fails to login without session.regenerate/save', async () => {
    const app = createTestApp({ addSessionCompat: false });
    const server = app.listen(0);
    try {
      const res = await request(server, 'POST', '/login');
      expect(res.status).toBe(500);
    } finally {
      server.close();
    }
  });

  it('succeeds with session compat middleware', async () => {
    const app = createTestApp({ addSessionCompat: true });
    const server = app.listen(0);
    try {
      const loginRes = await request(server, 'POST', '/login');
      expect(loginRes.status).toBe(200);
      expect(loginRes.body).toEqual({ ok: true });

      // Extract set-cookie header and verify session persists
      const cookies = loginRes.headers['set-cookie'];
      expect(cookies).toBeDefined();
      const cookieHeader = Array.isArray(cookies) ? cookies.join('; ') : String(cookies);

      const checkRes = await request(server, 'GET', '/check', { cookie: cookieHeader });
      expect(checkRes.status).toBe(200);
      expect(checkRes.body.user).toEqual({ id: 'test-user' });
    } finally {
      server.close();
    }
  });
});
