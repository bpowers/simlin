// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Contract tests for POST /session (#927): the login client renders the
// response body's `error` field on the Login screen, so every failure
// branch must answer with the same JSON {error} envelope the rest of the
// API uses (res.sendStatus's plain-text body made the client's
// response.json() throw, masking the real failure). These tests go over
// real HTTP -- the bug was in the serialized response, so asserting on
// status/Content-Type/body at the wire level is the point.

import { describe, it, expect, beforeEach, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

import express from 'express';
import http from 'http';
import type * as admin from 'firebase-admin';

import type { Application } from '../application';
import { authn } from '../authn';
import * as logger from '../logger';
import type { Database } from '../models/db-interfaces';
import type { Table } from '../models/table';
import { seshcookie } from '../seshcookie/seshcookie';
import { User } from '../schemas/user_pb';

// Silence and capture the structured log lines: several assertions below
// check that raw Firebase/DB failure detail goes to the server log and
// NOT into the client-facing envelope. `{ spy: true }` is hoisted above
// the imports; a factory can't be used here for the same CommonJS reasons
// as static-config.test.ts.
rs.mock('../logger', { spy: true });

const errorLogMock = logger.error as unknown as Mock<typeof logger.error>;
const infoLogMock = logger.info as unknown as Mock<typeof logger.info>;

beforeEach(() => {
  errorLogMock.mockReset();
  errorLogMock.mockImplementation(() => {});
  infoLogMock.mockReset();
  infoLogMock.mockImplementation(() => {});
});

function makeUser(id: string, email: string): User {
  const user = new User();
  user.setId(id);
  user.setEmail(email);
  return user;
}

// Only the methods /session actually touches: findOne (sessionAuth
// middleware) plus the findOneByScan/create/findByScan/deleteOne cluster
// in getOrCreateUserFromProfile.
function fakeUserTable(overrides: Partial<Table<User>> = {}): Table<User> {
  const base: Partial<Table<User>> = {
    findOne: () => Promise.resolve(undefined),
    findOneByScan: () => Promise.resolve(undefined),
    findByScan: () => Promise.resolve(undefined),
    create: () => Promise.resolve(),
    deleteOne: () => Promise.resolve(),
  };
  return { ...base, ...overrides } as Table<User>;
}

interface FirebaseAuthStub {
  verifyIdToken?: (token: string) => Promise<admin.auth.DecodedIdToken>;
  getUser?: (uid: string) => Promise<admin.auth.UserRecord>;
}

function decodedToken(uid: string): admin.auth.DecodedIdToken {
  return { uid } as admin.auth.DecodedIdToken;
}

function firebaseUserRecord(fields: {
  disabled?: boolean;
  email?: string;
  displayName?: string;
}): admin.auth.UserRecord {
  return {
    uid: 'uid-1',
    disabled: fields.disabled ?? false,
    email: fields.email,
    displayName: fields.displayName,
  } as admin.auth.UserRecord;
}

function fakeFirebaseAuth(stub: FirebaseAuthStub = {}): admin.auth.Auth {
  return {
    verifyIdToken: stub.verifyIdToken ?? (() => Promise.resolve(decodedToken('uid-1'))),
    getUser: stub.getUser ?? (() => Promise.resolve(firebaseUserRecord({ email: 'alice@example.com' }))),
  } as unknown as admin.auth.Auth;
}

// Mirror the app.ts middleware /session depends on: the seshcookie
// session (setSessionUser writes req.session on success) and the JSON
// body parser.
function createSessionApp(users: Table<User>, firebaseAuthn: admin.auth.Auth): express.Express {
  const app = express() as unknown as Application;
  app.use(
    seshcookie({
      key: 'test-key-for-encryption-1234',
      cookieName: 'test_session',
      cookiePath: '/',
      httpOnly: true,
      secure: false,
    }),
  );
  app.use(express.json());
  app.db = { user: users } as unknown as Database;
  authn(app, firebaseAuthn);
  return app as unknown as express.Express;
}

interface WireResponse {
  status: number;
  contentType: string | undefined;
  rawBody: string;
  body: unknown;
  headers: http.IncomingHttpHeaders;
}

function postSession(server: http.Server, requestBody: string): Promise<WireResponse> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === 'string') {
      return reject(new Error('server not listening'));
    }
    const req = http.request(
      {
        hostname: '127.0.0.1',
        port: addr.port,
        path: '/session',
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      },
      (res) => {
        let data = '';
        res.on('data', (chunk) => (data += chunk));
        res.on('end', () => {
          let body: unknown;
          try {
            body = JSON.parse(data) as unknown;
          } catch {
            body = undefined;
          }
          resolve({
            status: res.statusCode ?? 0,
            contentType: res.headers['content-type'],
            rawBody: data,
            body,
            headers: res.headers,
          });
        });
      },
    );
    req.on('error', reject);
    req.write(requestBody);
    req.end();
  });
}

async function withServer(app: express.Express, fn: (server: http.Server) => Promise<void>): Promise<void> {
  const server = app.listen(0);
  try {
    await fn(server);
  } finally {
    // Node's default global agent keeps sockets alive, so close() on its
    // own would sit waiting for the idle keep-alive connection; drop idle
    // sockets and await the close so a lingering handle can't flake the
    // suite.
    server.closeIdleConnections();
    await new Promise<void>((resolve, reject) => {
      server.close((err) => (err ? reject(err) : resolve()));
    });
  }
}

// The contract every failure branch must satisfy: the given status, a
// JSON content type, and a non-empty {error: string} envelope.
function expectErrorEnvelope(res: WireResponse, status: number): string {
  expect(res.status).toBe(status);
  expect(res.contentType).toMatch(/application\/json/);
  expect(typeof res.body).toBe('object');
  const error = (res.body as Record<string, unknown>).error;
  expect(typeof error).toBe('string');
  expect((error as string).length).toBeGreaterThan(0);
  return error as string;
}

function loggedErrorLines(): string {
  return errorLogMock.mock.calls.map((call) => String(call[0])).join('\n');
}

describe('POST /session error contract', () => {
  it('answers 400 with the JSON envelope when idToken is missing', async () => {
    const app = createSessionApp(fakeUserTable(), fakeFirebaseAuth());
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({}));
      expectErrorEnvelope(res, 400);
    });
  });

  it('answers 400 with the JSON envelope when idToken is empty', async () => {
    const app = createSessionApp(fakeUserTable(), fakeFirebaseAuth());
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: '' }));
      expectErrorEnvelope(res, 400);
    });
  });

  it('answers 401 when the Firebase token is rejected, logging the detail but not leaking it', async () => {
    const app = createSessionApp(
      fakeUserTable(),
      fakeFirebaseAuth({
        verifyIdToken: () => Promise.reject(new Error('SECRET-DETAIL: Firebase ID token has expired')),
      }),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'expired-token' }));
      const clientError = expectErrorEnvelope(res, 401);
      // The raw exception text is for the server log only.
      expect(clientError).not.toContain('SECRET-DETAIL');
      expect(res.rawBody).not.toContain('SECRET-DETAIL');
      expect(loggedErrorLines()).toContain('SECRET-DETAIL');
    });
  });

  it('answers 401 when the account behind a still-valid token no longer exists', async () => {
    // Firebase id tokens outlive account deletion by up to an hour, so
    // verifyIdToken succeeds but getUser reports auth/user-not-found.
    const notFound = Object.assign(new Error('no user record'), { code: 'auth/user-not-found' });
    const app = createSessionApp(fakeUserTable(), fakeFirebaseAuth({ getUser: () => Promise.reject(notFound) }));
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      expectErrorEnvelope(res, 401);
    });
  });

  it('answers 500 when the Firebase user lookup fails for infrastructure reasons', async () => {
    const app = createSessionApp(
      fakeUserTable(),
      fakeFirebaseAuth({ getUser: () => Promise.reject(new Error('SECRET-DETAIL: firebase unreachable')) }),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      const clientError = expectErrorEnvelope(res, 500);
      expect(clientError).not.toContain('SECRET-DETAIL');
      expect(loggedErrorLines()).toContain('SECRET-DETAIL');
    });
  });

  it('answers 403 for a disabled account', async () => {
    const app = createSessionApp(
      fakeUserTable(),
      fakeFirebaseAuth({ getUser: () => Promise.resolve(firebaseUserRecord({ disabled: true, email: 'a@b.com' })) }),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      expectErrorEnvelope(res, 403);
    });
  });

  it('answers 403 for an account without an email address', async () => {
    const app = createSessionApp(
      fakeUserTable(),
      fakeFirebaseAuth({ getUser: () => Promise.resolve(firebaseUserRecord({})) }),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      expectErrorEnvelope(res, 403);
    });
  });

  it('answers 500 with the JSON envelope when the user table fails', async () => {
    const app = createSessionApp(
      fakeUserTable({ findOneByScan: () => Promise.reject(new Error('SECRET-DETAIL: firestore down')) }),
      fakeFirebaseAuth(),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      const clientError = expectErrorEnvelope(res, 500);
      expect(clientError).not.toContain('SECRET-DETAIL');
    });
  });

  it('logs the detail even when the user table rejects with a non-Error value', async () => {
    // Firestore client libraries (and buggy fakes) can reject with plain
    // strings or objects; the swallowing catch in getOrCreateUserFromProfile
    // is the only place that detail is observable, so it must be logged for
    // non-Error rejections too.
    const app = createSessionApp(
      fakeUserTable({ findOneByScan: () => Promise.reject('SECRET-DETAIL: string rejection') }),
      fakeFirebaseAuth(),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      const clientError = expectErrorEnvelope(res, 500);
      expect(clientError).not.toContain('SECRET-DETAIL');
      expect(loggedErrorLines()).toContain('SECRET-DETAIL');
    });
  });

  it('keeps the JSON envelope even for an unexpected rejection (defense in depth)', async () => {
    // Trip the temp-user consistency-recovery path (the 'expected single
    // result document' message) and make its recovery query reject too:
    // getOrCreateUserFromProfile itself rejects, exercising the route's
    // final catch, which previously handed off to Express's HTML error
    // page.
    const app = createSessionApp(
      fakeUserTable({
        findOneByScan: () => Promise.reject(new Error('expected single result document, found 2')),
        findByScan: () => Promise.reject(new Error('SECRET-DETAIL: recovery scan failed')),
      }),
      fakeFirebaseAuth(),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      const clientError = expectErrorEnvelope(res, 500);
      expect(clientError).not.toContain('SECRET-DETAIL');
      expect(res.rawBody).not.toContain('SECRET-DETAIL');
    });
  });
});

describe('POST /session success', () => {
  it('answers 200 and mints a session cookie for an existing user', async () => {
    const alice = makeUser('alice', 'alice@example.com');
    const app = createSessionApp(fakeUserTable({ findOneByScan: () => Promise.resolve(alice) }), fakeFirebaseAuth());
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      expect(res.status).toBe(200);
      expect(res.headers['set-cookie']).toBeDefined();
    });
  });

  it('answers 200 and creates a temp user record for a first-time login', async () => {
    const created: Array<{ id: string; user: User }> = [];
    const app = createSessionApp(
      fakeUserTable({
        create: (id: string, user: User) => {
          created.push({ id, user });
          return Promise.resolve();
        },
      }),
      fakeFirebaseAuth(),
    );
    await withServer(app, async (server) => {
      const res = await postSession(server, JSON.stringify({ idToken: 'valid-token' }));
      expect(res.status).toBe(200);
      expect(created.length).toBe(1);
      expect(created[0].id).toMatch(/^temp-/);
      expect(created[0].user.getEmail()).toBe('alice@example.com');
    });
  });
});
