// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Pins the API behaviors that ride on the requester's identity, so the
// session-shape handling inside api.ts can be consolidated onto the
// session-auth helpers without regressing them: the preview route's
// owner check (owner allowed, non-owner denied, public bypass) and
// PATCH /api/user's session rewrite after a username is chosen.

import { describe, it, expect } from '@rstest/core';

import { createHarness, extractSessionCookie, findClearingSetCookie, login, request, withServer } from './wire-harness';

describe('GET /api/preview/:username/:projectName ownership', () => {
  it('serves the preview of a private project to its owner', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await request(server, 'GET', '/api/preview/alice/secret', { cookie });
      expect(res.status).toBe(200);
      expect(res.headers['content-type']).toMatch(/image\/png/);
    });
  });

  it('denies the preview of a private project to a different user', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'bob');

      const res = await request(server, 'GET', '/api/preview/alice/secret', { cookie });
      expect(res.status).toBe(401);
    });
  });

  it('serves the preview of a public project to a non-owner', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await request(server, 'GET', '/api/preview/bob/climate', { cookie });
      expect(res.status).toBe(200);
      expect(res.headers['content-type']).toMatch(/image\/png/);
    });
  });

  it('is not part of the unauthenticated carve-out: anonymous requests get 401', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const res = await request(server, 'GET', '/api/preview/bob/climate');
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    });
  });

  it('answers a failed preview regeneration with the {error} envelope, not {}', async () => {
    const harness = createHarness();
    // No cached preview plus a missing File doc forces updatePreview to
    // throw before any rendering happens.
    harness.previews.delete('alice/secret');
    harness.files.delete('file-2');

    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await request(server, 'GET', '/api/preview/alice/secret', { cookie });
      expect(res.status).toBe(500);
      expect(res.body).toEqual({ error: 'unable to render preview' });
    });
  });
});

describe('PATCH /api/user session rewrite', () => {
  it('reauthenticates the session as the newly chosen username', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const tempCookie = await login(server, 'temp-1');

      const patchRes = await request(
        server,
        'PATCH',
        '/api/user',
        { cookie: tempCookie, 'content-type': 'application/json' },
        JSON.stringify({ username: 'carol', agreeToTermsAndPrivacyPolicy: true }),
      );
      expect(patchRes.status).toBe(200);

      // The response must rewrite the session cookie to name the new id;
      // presenting it authenticates as the renamed user.
      const renamedCookie = extractSessionCookie(patchRes.headers);
      const userRes = await request(server, 'GET', '/api/user', { cookie: renamedCookie });
      expect(userRes.status).toBe(200);
      expect((userRes.body as { id?: string }).id).toBe('carol');

      // The rename deleted the temp- record, so the pre-rename cookie is
      // now the #930 stale-session case: unauthenticated plus a clearing
      // Set-Cookie, never a 500.
      const staleRes = await request(server, 'GET', '/api/user', { cookie: tempCookie });
      expect(staleRes.status).toBe(401);
      expect(findClearingSetCookie(staleRes.headers)).toBeDefined();
    });
  });

  it('400s a rename to an existing username without touching either record', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const tempCookie = await login(server, 'temp-1');

      const res = await request(
        server,
        'PATCH',
        '/api/user',
        { cookie: tempCookie, 'content-type': 'application/json' },
        JSON.stringify({ username: 'alice', agreeToTermsAndPrivacyPolicy: true }),
      );
      expect(res.status).toBe(400);
      expect(res.body).toEqual({ error: 'username already taken' });

      // both the existing user and the temp record must be intact, and the
      // temp session must still authenticate for a retry
      expect(harness.users.get('alice')?.getEmail()).toBe('alice@example.com');
      expect(harness.users.has('temp-1')).toBe(true);
      const whoami = await request(server, 'GET', '/api/user', { cookie: tempCookie });
      expect(whoami.status).toBe(200);
      expect((whoami.body as { id?: string }).id).toBe('temp-1');
    });
  });

  it('500s a rename that fails for reasons other than a duplicate, instead of lying "taken"', async () => {
    const harness = createHarness();
    harness.db.user.create = () => Promise.reject(new Error('firestore unavailable'));

    await withServer(harness.app, async (server) => {
      const tempCookie = await login(server, 'temp-1');

      const res = await request(
        server,
        'PATCH',
        '/api/user',
        { cookie: tempCookie, 'content-type': 'application/json' },
        JSON.stringify({ username: 'carol', agreeToTermsAndPrivacyPolicy: true }),
      );
      expect(res.status).toBe(500);
      expect(res.body).toEqual({ error: 'internal error' });
      expect(harness.users.has('temp-1')).toBe(true);
    });
  });
});
