// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Wire-level coverage for issue #930: a still-valid session cookie naming
// a user that no longer exists in the DB must degrade to a clean
// unauthenticated request (401 from authz, or the anonymous behavior on
// public carve-outs) with the dead cookie expired -- not a repeatable 500
// from getUser deep inside an API handler. These tests run over the shared
// wire harness (the real middleware chain in app.ts order) so the
// interaction between the pieces, not any one unit, is what's under test.

import { describe, it, expect } from '@rstest/core';

import express from 'express';
import http from 'http';

import { getUser } from '../api';
import {
  createHarness,
  extractSessionCookie,
  findClearingSetCookie,
  login,
  request,
  withServer,
  Harness,
} from './wire-harness';

// Mint a session cookie for alice, then delete her user record: the
// cookie still decrypts to a valid-looking session, but the user is gone.
async function mintStaleCookie(server: http.Server, harness: Harness): Promise<string> {
  const cookie = await login(server, 'alice');
  harness.users.delete('alice');
  return cookie;
}

describe('stale session across the real middleware chain (#930)', () => {
  it('answers 401 to GET /api/user, clears the cookie, and never reaches the API router', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const staleCookie = await mintStaleCookie(server, harness);

      const res = await request(server, 'GET', '/api/user', { cookie: staleCookie });
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
      expect(findClearingSetCookie(res.headers)).toBeDefined();
      expect(harness.apiRequestsPastAuthz()).toBe(0);
    });
  });

  it('behaves exactly like an anonymous request on the public-projects carve-out', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const staleCookie = await mintStaleCookie(server, harness);

      const anonRes = await request(server, 'GET', '/api/projects/bob/climate');
      const staleRes = await request(server, 'GET', '/api/projects/bob/climate', { cookie: staleCookie });

      expect(anonRes.status).toBe(200);
      expect(staleRes.status).toBe(anonRes.status);
      expect(staleRes.body).toEqual(anonRes.body);
      // ...but unlike the anonymous request, the dead cookie gets expired.
      expect(findClearingSetCookie(staleRes.headers)).toBeDefined();
    });
  });

  it('still authenticates a fresh session on GET /api/user (no regression)', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await request(server, 'GET', '/api/user', { cookie });
      expect(res.status).toBe(200);
      expect((res.body as { id?: string }).id).toBe('alice');
      // The live session must survive untouched: no Set-Cookie at all,
      // neither a rewrite nor a clearing.
      expect(res.headers['set-cookie']).toBeUndefined();
    });
  });

  it('answers 401 to GET /api/user with no session, without setting any cookie', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const res = await request(server, 'GET', '/api/user');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
      // No cookie came in, so there is nothing to clear.
      expect(res.headers['set-cookie']).toBeUndefined();
    });
  });

  it('lets a login over a stale cookie mint a fresh session, not a cleared one', async () => {
    // sessionAuth empties the stale session, but a handler that then
    // re-authenticates the request (POST /session) repopulates it, and
    // seshcookie must answer with a new session cookie -- not the
    // Max-Age=0 clearing it sends for sessions that stay empty.
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const staleCookie = await mintStaleCookie(server, harness);

      const reloginRes = await request(server, 'POST', '/login/bob', { cookie: staleCookie });
      expect(reloginRes.status).toBe(200);
      expect(findClearingSetCookie(reloginRes.headers)).toBeUndefined();
      const freshCookie = extractSessionCookie(reloginRes.headers);

      const res = await request(server, 'GET', '/api/user', { cookie: freshCookie });
      expect(res.status).toBe(200);
      expect((res.body as { id?: string }).id).toBe('bob');
    });
  });

  it('treats the HTML project route as anonymous and clears the cookie', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const staleCookie = await mintStaleCookie(server, harness);

      // Public project: same redirect an anonymous visitor gets.
      const anonPublic = await request(server, 'GET', '/bob/climate');
      const stalePublic = await request(server, 'GET', '/bob/climate', { cookie: staleCookie });
      expect(anonPublic.status).toBe(302);
      expect(stalePublic.status).toBe(anonPublic.status);
      expect(stalePublic.headers.location).toBe(anonPublic.headers.location);
      expect(findClearingSetCookie(stalePublic.headers)).toBeDefined();

      // Private project: the anonymous redirect-to-home, not a 500.
      const stalePrivate = await request(server, 'GET', '/alice/secret', { cookie: staleCookie });
      expect(stalePrivate.status).toBe(302);
      expect(stalePrivate.headers.location).toBe('/');
      expect(findClearingSetCookie(stalePrivate.headers)).toBeDefined();
    });
  });
});

describe('the /api/projects carve-out admits exactly the public detail shape', () => {
  // Express 5's routing is non-strict and case-insensitive, so
  // /api/projects/ dispatches to the authenticated LIST handler and
  // /api/PROJECTS/bob/climate dispatches to the public detail handler.
  // The carve-out in authz must mirror those dispatch semantics: a bare
  // prefix test admitted the LIST alias (anonymous 500 in getUser) while
  // 401ing the uppercase spelling of a genuinely public path.

  it('rejects the LIST alias /api/projects/ for anonymous and stale sessions', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const anonRes = await request(server, 'GET', '/api/projects/');
      expect(anonRes.status).toBe(401);
      expect(anonRes.body).toEqual({ error: 'unauthorized' });

      const staleCookie = await mintStaleCookie(server, harness);
      const staleRes = await request(server, 'GET', '/api/projects/', { cookie: staleCookie });
      expect(staleRes.status).toBe(401);
      expect(staleRes.body).toEqual({ error: 'unauthorized' });
      expect(findClearingSetCookie(staleRes.headers)).toBeDefined();

      expect(harness.apiRequestsPastAuthz()).toBe(0);
    });
  });

  it('rejects anonymous GET /api/projects (the exact LIST route)', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const res = await request(server, 'GET', '/api/projects');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
      expect(harness.apiRequestsPastAuthz()).toBe(0);
    });
  });

  it('admits the public detail route case-insensitively, as Express dispatches it', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const lower = await request(server, 'GET', '/api/projects/bob/climate');
      const upper = await request(server, 'GET', '/api/PROJECTS/bob/climate');
      expect(lower.status).toBe(200);
      expect(upper.status).toBe(lower.status);
      expect(upper.body).toEqual(lower.body);
    });
  });

  it('still lists projects on the authenticated trailing-slash alias /api/projects/', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');
      const res = await request(server, 'GET', '/api/projects/', { cookie });
      expect(res.status).toBe(200);
      expect((res.body as Array<{ id?: string }>).map((p) => p.id)).toEqual(['alice/secret']);
    });
  });
});

describe('getUser defense-in-depth envelope', () => {
  // If authz's carve-out and the router's dispatch ever disagree about a
  // path again, this branch fires; it must keep the API's {error: string}
  // envelope rather than the bare {} it historically sent.
  it('sends the JSON error envelope on the 500-and-throw path', () => {
    const statuses: number[] = [];
    const bodies: unknown[] = [];
    const res = {
      status(code: number) {
        statuses.push(code);
        return this;
      },
      json(body: unknown) {
        bodies.push(body);
        return this;
      },
    };

    expect(() => getUser({} as express.Request, res as unknown as express.Response)).toThrow(
      'user not found, but passed authz?',
    );
    expect(statuses).toEqual([500]);
    expect(bodies.length).toBe(1);
    const error = (bodies[0] as Record<string, unknown>).error;
    expect(typeof error).toBe('string');
    expect((error as string).length).toBeGreaterThan(0);
  });
});
