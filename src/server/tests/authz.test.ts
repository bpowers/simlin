// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import express, { Request } from 'express';
import http from 'http';

import authz from '../authz';

// Why these tests exist:
//
// authz is mounted via `app.use('/api', authz, apiRouter)`. Express
// dispatches a middleware as a request handler only when its
// Function.length is 0..3; a function with length 4 is treated as an
// error handler and skipped during normal request flow. If the
// compiled `authz` ever drifts back to 4 args the test below catches
// it before it reaches production -- the symptom in production is
// silent: every authenticated /api/* write returns 500 (because
// downstream `getUser(req, res)` throws on `req.user === undefined`)
// instead of 401.

type SessionShape = {
  passport?: { user?: { id: string } };
};

type RequestWithSession = Request & { session: SessionShape };

// Simulates the middleware that runs before authz in app.ts: seshcookie
// (req.session) and, when userFactory is given, sessionAuth (req.user).
// Returns a handle on the last request seen so tests can observe authz's
// session clearing even when authz itself terminates the request.
function installSession(
  app: express.Express,
  sessionFactory: () => SessionShape,
  userFactory?: () => unknown,
): { lastRequest: () => RequestWithSession | undefined } {
  let last: RequestWithSession | undefined;
  app.use((req, _res, next) => {
    (req as RequestWithSession).session = sessionFactory();
    if (userFactory !== undefined) {
      req.user = userFactory();
    }
    last = req as RequestWithSession;
    next();
  });
  return { lastRequest: () => last };
}

function makeRequest(server: http.Server, method: string, path: string): Promise<{ status: number; body: unknown }> {
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
          resolve({ status: res.statusCode ?? 0, body });
        });
      },
    );
    req.on('error', reject);
    req.end();
  });
}

describe('authz middleware', () => {
  it('has Function.length === 3 so Express dispatches it as request middleware', () => {
    // Express treats a function with length 4 as an error handler and
    // skips it during normal request flow. The default export of authz
    // must therefore have exactly 3 declared parameters.
    expect(authz.length).toBe(3);
  });

  it('returns 401 on unauthenticated POST when mounted via app.use', async () => {
    const app = express();
    installSession(app, () => ({}));
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'POST', '/api/projects');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    } finally {
      server.close();
    }
  });

  it('lets authenticated requests through to the downstream handler', async () => {
    const app = express();
    installSession(
      app,
      () => ({ passport: { user: { id: 'test-user' } } }),
      () => ({ getId: () => 'test-user' }),
    );
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'POST', '/api/projects');
      expect(res.status).toBe(200);
      expect(res.body).toEqual({ reachedDownstream: true });
    } finally {
      server.close();
    }
  });

  it('returns 401 for a stale session: valid shape but no deserialized user (#930)', async () => {
    // sessionAuth leaves req.user unset when the session names a user
    // that no longer exists in the DB. Authorization must key off the
    // deserialized user, not the session shape, or every API call from a
    // deleted account turns into a 500 downstream.
    const app = express();
    const { lastRequest } = installSession(app, () => ({ passport: { user: { id: 'deleted-user' } } }));
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'POST', '/api/projects');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
      // authz must empty the session so seshcookie expires the dead cookie.
      expect(lastRequest()?.session).toEqual({});
    } finally {
      server.close();
    }
  });

  it('treats a stale session as unauthenticated on the GET /projects/* carve-out', async () => {
    const app = express();
    const { lastRequest } = installSession(app, () => ({ passport: { user: { id: 'deleted-user' } } }));
    app.use('/api', authz, (req, res) => {
      res.status(200).json({ sawUser: req.user !== undefined });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'GET', '/api/projects/alice/my-model');
      expect(res.status).toBe(200);
      expect(res.body).toEqual({ sawUser: false });
      expect(lastRequest()?.session).toEqual({});
    } finally {
      server.close();
    }
  });

  it('allows unauthenticated GET to /projects/* (embedding case)', async () => {
    const app = express();
    installSession(app, () => ({}));
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'GET', '/api/projects/alice/my-model');
      expect(res.status).toBe(200);
      expect(res.body).toEqual({ reachedDownstream: true });
    } finally {
      server.close();
    }
  });

  it('matches the carve-out exactly as Express dispatches project routes', async () => {
    // Express routing is non-strict and case-insensitive, so the
    // carve-out must accept the same aliases of the public detail route
    // (case variants, one trailing slash) while rejecting everything
    // else under /projects/ -- notably the bare LIST alias /projects/,
    // which dispatches to a handler that requires authentication.
    const app = express();
    installSession(app, () => ({}));
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const allowed = ['/api/projects/alice/my-model', '/api/PROJECTS/alice/my-model', '/api/projects/alice/my-model/'];
    const denied = ['/api/projects/', '/api/projects', '/api/projects/alice', '/api/projects/alice/my-model/extra'];

    const server = app.listen(0);
    try {
      for (const path of allowed) {
        const res = await makeRequest(server, 'GET', path);
        expect([path, res.status]).toEqual([path, 200]);
      }
      for (const path of denied) {
        const res = await makeRequest(server, 'GET', path);
        expect([path, res.status]).toEqual([path, 401]);
      }
    } finally {
      server.close();
    }
  });

  it('returns 401 on POST to /api/projects/* (write to a project requires auth)', async () => {
    const app = express();
    installSession(app, () => ({}));
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'POST', '/api/projects/alice/my-model');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    } finally {
      server.close();
    }
  });

  it('returns 401 when session has passport but no user', async () => {
    const app = express();
    installSession(app, () => ({ passport: {} }));
    app.use('/api', authz, (_req, res) => {
      res.status(200).json({ reachedDownstream: true });
    });

    const server = app.listen(0);
    try {
      const res = await makeRequest(server, 'POST', '/api/user');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    } finally {
      server.close();
    }
  });
});
