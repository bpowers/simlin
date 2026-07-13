// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Shared wire-level test harness: the real middleware chain in app.ts
// order (seshcookie -> sessionAuth -> authz -> apiRouter, plus the HTML
// project route) over a Map-backed database, so tests exercise the
// interaction between the auth pieces rather than any one unit. Helpers
// throw (rather than expect()) on harness misuse so failures are loud
// without coupling this module to the test runner.

import express from 'express';
import http from 'http';

import { apiRouter } from '../api';
import type { Application } from '../application';
import authz from '../authz';
import type { Database } from '../models/db-interfaces';
import { createProjectRouteHandler } from '../route-handlers';
import { seshcookie } from '../seshcookie/seshcookie';
import { sessionAuth, setSessionUser } from '../session-auth';
import { File } from '../schemas/file_pb';
import { Preview } from '../schemas/preview_pb';
import { Project } from '../schemas/project_pb';
import { User } from '../schemas/user_pb';

export const COOKIE_NAME = 'test_session';

export function makeUser(id: string): User {
  const user = new User();
  user.setId(id);
  user.setEmail(`${id}@example.com`);
  return user;
}

export function makeProject(id: string, ownerId: string, isPublic: boolean, fileId: string): Project {
  const project = new Project();
  project.setId(id);
  project.setOwnerId(ownerId);
  project.setIsPublic(isPublic);
  project.setFileId(fileId);
  return project;
}

export function makeFile(id: string): File {
  const file = new File();
  file.setId(id);
  file.setJsonContents('{"name":"climate"}');
  return file;
}

export function makePreview(id: string): Preview {
  const preview = new Preview();
  preview.setId(id);
  // Any bytes will do; the route only round-trips them.
  preview.setPng(new Uint8Array([0x89, 0x50, 0x4e, 0x47]));
  return preview;
}

export interface Harness {
  app: express.Express;
  users: Map<string, User>;
  projects: Map<string, Project>;
  files: Map<string, File>;
  previews: Map<string, Preview>;
  // How many /api requests made it past authz to the API router.
  apiRequestsPastAuthz: () => number;
}

export function createHarness(): Harness {
  const users = new Map<string, User>();
  users.set('alice', makeUser('alice'));
  users.set('bob', makeUser('bob'));
  users.set('temp-1', makeUser('temp-1'));

  const projects = new Map<string, Project>();
  projects.set('bob/climate', makeProject('bob/climate', 'bob', true, 'file-1'));
  projects.set('alice/secret', makeProject('alice/secret', 'alice', false, 'file-2'));

  const files = new Map<string, File>();
  files.set('file-1', makeFile('file-1'));
  files.set('file-2', makeFile('file-2'));

  const previews = new Map<string, Preview>();
  previews.set('bob/climate', makePreview('bob/climate'));
  previews.set('alice/secret', makePreview('alice/secret'));

  const byId = <T>(records: Map<string, T>) => ({
    findOne: (id: string): Promise<T | undefined> => Promise.resolve(records.get(id)),
  });

  const app = express() as unknown as Application;
  app.use(
    seshcookie({
      key: 'test-key-for-encryption-1234',
      cookieName: COOKIE_NAME,
      cookiePath: '/',
      httpOnly: true,
      secure: false,
    }),
  );
  app.use(express.json());
  app.db = {
    // The user table also backs PATCH /api/user's rename (create + delete).
    user: {
      ...byId(users),
      create: (id: string, user: User): Promise<void> => {
        users.set(id, user);
        return Promise.resolve();
      },
      deleteOne: (id: string): Promise<void> => {
        users.delete(id);
        return Promise.resolve();
      },
    },
    project: {
      ...byId(projects),
      // Backs GET /api/projects (list-by-owner does a prefix scan).
      find: (idPrefix: string): Promise<Project[]> =>
        Promise.resolve([...projects.values()].filter((p) => p.getId().startsWith(idPrefix))),
    },
    file: byId(files),
    preview: byId(previews),
  } as unknown as Database;
  app.use(sessionAuth(app.db.user));

  // Stand-in for POST /session: mint a session cookie for the named user
  // without dragging Firebase into the harness. The session written is
  // byte-for-byte what the real login writes (setSessionUser).
  app.post('/login/:userId', (req, res) => {
    setSessionUser(req, req.params.userId as string);
    res.sendStatus(200);
  });

  let pastAuthz = 0;
  app.use(
    '/api',
    authz,
    (_req, _res, next) => {
      pastAuthz++;
      next();
    },
    apiRouter(app),
  );

  app.get('/:username/:projectName', createProjectRouteHandler({ db: app.db }), (_req, res) => {
    res.status(200).send('app-shell');
  });

  return {
    app: app as unknown as express.Express,
    users,
    projects,
    files,
    previews,
    apiRequestsPastAuthz: () => pastAuthz,
  };
}

export interface WireResponse {
  status: number;
  body: unknown;
  headers: http.IncomingHttpHeaders;
}

export function request(
  server: http.Server,
  method: string,
  path: string,
  headers?: Record<string, string>,
  body?: string,
): Promise<WireResponse> {
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
          let parsed: unknown;
          try {
            parsed = JSON.parse(data) as unknown;
          } catch {
            parsed = data;
          }
          resolve({ status: res.statusCode ?? 0, body: parsed, headers: res.headers });
        });
      },
    );
    req.on('error', reject);
    if (body !== undefined) {
      req.write(body);
    }
    req.end();
  });
}

export async function withServer(app: express.Express, fn: (server: http.Server) => Promise<void>): Promise<void> {
  const server = app.listen(0);
  try {
    await fn(server);
  } finally {
    // Node's default agent keeps sockets alive; drop idle connections and
    // await the close so a lingering handle can't flake the suite.
    server.closeIdleConnections();
    await new Promise<void>((resolve, reject) => {
      server.close((err) => (err ? reject(err) : resolve()));
    });
  }
}

export function setCookieList(headers: http.IncomingHttpHeaders): string[] {
  const setCookie = headers['set-cookie'];
  if (setCookie === undefined) {
    return [];
  }
  return Array.isArray(setCookie) ? setCookie : [String(setCookie)];
}

// The name=value pair to send back on the next request's Cookie header.
export function extractSessionCookie(headers: http.IncomingHttpHeaders): string {
  const raw = setCookieList(headers).find((c) => c.startsWith(`${COOKIE_NAME}=`));
  if (raw === undefined) {
    throw new Error('expected a session Set-Cookie on the response');
  }
  return raw.split(';')[0];
}

// seshcookie expires an emptied session's cookie with an empty value and
// Max-Age=0; its presence is how "the dead cookie stops coming back".
export function findClearingSetCookie(headers: http.IncomingHttpHeaders): string | undefined {
  return setCookieList(headers).find((c) => c.startsWith(`${COOKIE_NAME}=;`) && c.includes('Max-Age=0'));
}

// Log in as `userId` and return the session cookie to send on subsequent
// requests.
export async function login(server: http.Server, userId: string): Promise<string> {
  const loginRes = await request(server, 'POST', `/login/${userId}`);
  if (loginRes.status !== 200) {
    throw new Error(`login as ${userId} failed: ${loginRes.status}`);
  }
  return extractSessionCookie(loginRes.headers);
}
