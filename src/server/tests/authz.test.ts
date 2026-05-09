// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

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

function installSession(app: express.Express, sessionFactory: () => SessionShape): void {
  app.use((req, _res, next) => {
    (req as RequestWithSession).session = sessionFactory();
    next();
  });
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
    installSession(app, () => ({ passport: { user: { id: 'test-user' } } }));
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
