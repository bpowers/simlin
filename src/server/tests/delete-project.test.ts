// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response } from 'express';

import { createDeleteProjectHandler, DeleteProjectHandlerDeps } from '../route-handlers';

interface MockProjectRecord {
  getId(): string;
  getOwnerId(): string;
  getIsPublic(): boolean;
  getFileId(): string | undefined;
}

function mockProject(id: string, ownerId?: string): MockProjectRecord {
  const owner = ownerId ?? (id.split('/')[0] as string);
  return {
    getId: () => id,
    getOwnerId: () => owner,
    getIsPublic: () => false,
    getFileId: () => 'file-abc',
  };
}

interface MockDeps {
  deps: DeleteProjectHandlerDeps;
  projectFindOne: jest.Mock;
  projectDeleteCalls: string[];
  previewDeleteCalls: string[];
}

function mockDeps(opts: { project?: MockProjectRecord; previewDeleteRejects?: boolean } = {}): MockDeps {
  const projectDeleteCalls: string[] = [];
  const previewDeleteCalls: string[] = [];
  const projectFindOne = jest.fn().mockResolvedValue(opts.project);
  return {
    projectFindOne,
    projectDeleteCalls,
    previewDeleteCalls,
    deps: {
      db: {
        project: {
          findOne: projectFindOne,
          deleteOne: jest.fn(async (id: string) => {
            projectDeleteCalls.push(id);
          }),
        },
        preview: {
          deleteOne: jest.fn(async (id: string) => {
            previewDeleteCalls.push(id);
            if (opts.previewDeleteRejects) {
              throw new Error('preview delete failed');
            }
          }),
        },
      },
    },
  };
}

interface MockResponse {
  res: Response;
  statusCode: () => number | undefined;
  jsonBody: () => unknown;
}

function mockResponse(): MockResponse {
  let code: number | undefined;
  let body: unknown;
  const res = {
    status: jest.fn((c: number) => {
      code = c;
      return res;
    }),
    json: jest.fn((b: unknown) => {
      body = b;
      return res;
    }),
  } as unknown as Response;
  return { res, statusCode: () => code, jsonBody: () => body };
}

function mockRequest(opts: { username?: string; projectName?: string; userId?: string } = {}): Request {
  const username = opts.username ?? 'alice';
  const projectName = opts.projectName ?? 'climate';
  const authed = opts.userId !== undefined;
  return {
    params: { username, projectName },
    // passport stores { id } in the session; the deserialized User (with
    // getId()) lives on req.user. getAuthenticatedUser() requires both.
    session: authed ? { passport: { user: { id: opts.userId } } } : undefined,
    user: authed ? { getId: () => opts.userId } : undefined,
  } as unknown as Request;
}

describe('createDeleteProjectHandler', () => {
  it('deletes the project and preview docs and returns 200 for the owner', async () => {
    const { deps, projectDeleteCalls, previewDeleteCalls } = mockDeps({ project: mockProject('alice/climate') });
    const handler = createDeleteProjectHandler(deps);
    const { res, statusCode, jsonBody } = mockResponse();

    await handler(mockRequest({ username: 'alice', projectName: 'climate', userId: 'alice' }), res);

    expect(projectDeleteCalls).toEqual(['alice/climate']);
    expect(previewDeleteCalls).toEqual(['alice/climate']);
    expect(statusCode()).toBe(200);
    expect(jsonBody()).toEqual({});
  });

  it('still returns 200 when preview deletion fails', async () => {
    // (route-handlers logs a winston warning here on purpose; winston's
    // module-level methods are non-configurable so we can't silence it.)
    const { deps, projectDeleteCalls } = mockDeps({
      project: mockProject('alice/climate'),
      previewDeleteRejects: true,
    });
    const handler = createDeleteProjectHandler(deps);
    const { res, statusCode } = mockResponse();

    await handler(mockRequest({ userId: 'alice' }), res);

    expect(projectDeleteCalls).toEqual(['alice/climate']);
    expect(statusCode()).toBe(200);
  });

  it('returns 404 when the project does not exist', async () => {
    const { deps, projectDeleteCalls, previewDeleteCalls } = mockDeps({ project: undefined });
    const handler = createDeleteProjectHandler(deps);
    const { res, statusCode } = mockResponse();

    await handler(mockRequest({ userId: 'alice' }), res);

    expect(statusCode()).toBe(404);
    expect(projectDeleteCalls).toEqual([]);
    expect(previewDeleteCalls).toEqual([]);
  });

  it('returns 401 when the request is unauthenticated', async () => {
    const { deps, projectFindOne, projectDeleteCalls } = mockDeps({ project: mockProject('alice/climate') });
    const handler = createDeleteProjectHandler(deps);
    const { res, statusCode } = mockResponse();

    await handler(mockRequest({}), res);

    expect(statusCode()).toBe(401);
    expect(projectFindOne).not.toHaveBeenCalled();
    expect(projectDeleteCalls).toEqual([]);
  });

  it('returns 401 without consulting the database when the auth user is not the URL owner', async () => {
    // No project-existence leak: a logged-in user must not learn whether
    // someone else's private project exists by observing 404 vs 401.
    const { deps, projectFindOne, projectDeleteCalls } = mockDeps({ project: mockProject('alice/climate') });
    const handler = createDeleteProjectHandler(deps);
    const { res, statusCode } = mockResponse();

    await handler(mockRequest({ username: 'alice', projectName: 'climate', userId: 'bob' }), res);

    expect(statusCode()).toBe(401);
    expect(projectFindOne).not.toHaveBeenCalled();
    expect(projectDeleteCalls).toEqual([]);
  });

  it('returns 401 when the stored project owner differs from the authenticated user', async () => {
    // The URL username matches the auth user, but the project record's
    // ownerId says someone else owns it (legacy/inconsistent doc key).
    const { deps, projectDeleteCalls } = mockDeps({ project: mockProject('alice/climate', 'realowner') });
    const handler = createDeleteProjectHandler(deps);
    const { res, statusCode } = mockResponse();

    await handler(mockRequest({ username: 'alice', projectName: 'climate', userId: 'alice' }), res);

    expect(statusCode()).toBe(401);
    expect(projectDeleteCalls).toEqual([]);
  });
});
