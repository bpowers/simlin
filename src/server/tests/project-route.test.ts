// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response } from 'express';

import { createProjectRouteHandler, ProjectRecord, ProjectRouteHandlerDeps } from '../route-handlers';
import { getAuthenticatedUser, isResourceOwner, AuthenticatedUser } from '../auth-helpers';

// Mock project factory
function createMockProject(opts: { id: string; isPublic: boolean; ownerId?: string; fileId?: string | undefined }): ProjectRecord {
  const ownerId = opts.ownerId ?? opts.id.split('/')[0];
  return {
    getId: () => opts.id,
    getOwnerId: () => ownerId,
    getIsPublic: () => opts.isPublic,
    getFileId: () => opts.fileId,
  };
}

// Mock database factory
function createMockDb(project: ProjectRecord | undefined): ProjectRouteHandlerDeps['db'] {
  return {
    project: {
      findOne: jest.fn().mockResolvedValue(project),
    },
  };
}

// Mock request factory
interface MockRequestOptions {
  username?: string;
  projectName?: string;
  path?: string;
  session?: Record<string, unknown> | undefined;
  user?: Record<string, unknown> | undefined;
}

function createMockRequest(options: MockRequestOptions = {}): Partial<Request> {
  const username = options.username ?? 'testuser';
  const projectName = options.projectName ?? 'testproject';
  return {
    params: { username, projectName },
    path: options.path ?? `/${username}/${projectName}`,
    url: `/${username}/${projectName}`,
    session: options.session as Request['session'],
    user: options.user,
  };
}

// Mock response factory
interface MockResponseResult {
  res: Partial<Response>;
  getRedirectUrl: () => string | undefined;
  getStatusCode: () => number | undefined;
}

function createMockResponse(): MockResponseResult {
  let redirectUrl: string | undefined;
  let statusCode: number | undefined;

  const res: Partial<Response> = {
    status: jest.fn((code: number) => {
      statusCode = code;
      return res as Response;
    }),
    json: jest.fn().mockReturnThis(),
    redirect: jest.fn((statusOrUrl: number | string, url?: string) => {
      if (typeof statusOrUrl === 'string') {
        redirectUrl = statusOrUrl;
      } else {
        redirectUrl = url;
      }
    }) as Response['redirect'],
    set: jest.fn().mockReturnThis(),
  };

  return {
    res,
    getRedirectUrl: () => redirectUrl,
    getStatusCode: () => statusCode,
  };
}

// Mock authenticated user for session
function createAuthenticatedSession(email: string): Record<string, unknown> {
  return {
    passport: {
      user: { email },
    },
  };
}

// Mock user object (as deserialized by passport)
function createMockUser(userId: string): Record<string, unknown> {
  return {
    getId: () => userId,
  };
}

describe('createProjectRouteHandler', () => {
  describe('project not found', () => {
    it('should return 404 when project does not exist', async () => {
      const db = createMockDb(undefined);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest();
      const { res, getStatusCode } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getStatusCode()).toBe(404);
      expect(res.json).toHaveBeenCalledWith({});
      expect(next).not.toHaveBeenCalled();
    });
  });

  describe('public project', () => {
    it('should redirect to /?project=... for public projects', async () => {
      const project = createMockProject({
        id: 'testuser/myproject',
        isPublic: true,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({ username: 'testuser', projectName: 'myproject' });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/?project=testuser/myproject');
      expect(next).not.toHaveBeenCalled();
    });

    it('should URL-encode project IDs with special characters', async () => {
      const project = createMockProject({
        id: 'testuser/my project',
        isPublic: true,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({ username: 'testuser', projectName: 'my project' });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/?project=testuser/my%20project');
      expect(next).not.toHaveBeenCalled();
    });
  });

  describe('private project - unauthenticated user', () => {
    it('should redirect to / when session is undefined', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: undefined,
      });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/');
      expect(next).not.toHaveBeenCalled();
    });

    it('should redirect to / when session.passport is undefined', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: {},
      });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/');
      expect(next).not.toHaveBeenCalled();
    });

    it('should redirect to / when session.passport.user is undefined', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: { passport: {} },
      });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/');
      expect(next).not.toHaveBeenCalled();
    });

    it('should redirect to / when req.user is undefined', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: createAuthenticatedSession('test@example.com'),
        user: undefined,
      });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/');
      expect(next).not.toHaveBeenCalled();
    });
  });

  describe('private project - wrong owner', () => {
    it('should redirect to / when user does not own the project', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: createAuthenticatedSession('other@example.com'),
        user: createMockUser('otheruser'),
      });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/');
      expect(next).not.toHaveBeenCalled();
    });
  });

  describe('private project - owner ID mismatch with URL', () => {
    it('should redirect to / when project ownerId differs from URL username even if URL matches auth user', async () => {
      // The project record says the owner is "realowner", but the URL says "testuser".
      // The authenticated user is "testuser", matching the URL but NOT the project record.
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        ownerId: 'realowner',
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: createAuthenticatedSession('test@example.com'),
        user: createMockUser('testuser'),
      });
      const { res, getRedirectUrl } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getRedirectUrl()).toBe('/');
      expect(next).not.toHaveBeenCalled();
    });
  });

  describe('private project - correct owner', () => {
    it('should serve index.html for authenticated owner', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: createAuthenticatedSession('test@example.com'),
        user: createMockUser('testuser'),
      });
      const { res } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(req.url).toBe('/index.html');
      expect(res.set).toHaveBeenCalledWith('Cache-Control', 'no-store');
      expect(res.set).toHaveBeenCalledWith('Max-Age', '0');
      expect(next).toHaveBeenCalled();
    });
  });

  describe('private project - missing file', () => {
    it('should return 404 when project has no fileId', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: undefined,
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        session: createAuthenticatedSession('test@example.com'),
        user: createMockUser('testuser'),
      });
      const { res, getStatusCode } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getStatusCode()).toBe(404);
      expect(res.json).toHaveBeenCalledWith({});
      expect(next).not.toHaveBeenCalled();
    });
  });

  describe('path validation', () => {
    it('should return 404 when path does not match expected format', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        path: '/testuser/private/extra/path',
        session: createAuthenticatedSession('test@example.com'),
        user: createMockUser('testuser'),
      });
      const { res, getStatusCode } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(getStatusCode()).toBe(404);
      expect(res.json).toHaveBeenCalledWith({});
      expect(next).not.toHaveBeenCalled();
    });

    it('should accept path with trailing slash', async () => {
      const project = createMockProject({
        id: 'testuser/private',
        isPublic: false,
        fileId: 'file123',
      });
      const db = createMockDb(project);
      const handler = createProjectRouteHandler({ db });

      const req = createMockRequest({
        username: 'testuser',
        projectName: 'private',
        path: '/testuser/private/',
        session: createAuthenticatedSession('test@example.com'),
        user: createMockUser('testuser'),
      });
      const { res } = createMockResponse();
      const next = jest.fn();

      await handler(req as Request, res as Response, next);

      expect(req.url).toBe('/index.html');
      expect(next).toHaveBeenCalled();
    });
  });
});

describe('getAuthenticatedUser', () => {
  it('should return undefined for undefined session', () => {
    const req = { session: undefined } as Partial<Request>;
    expect(getAuthenticatedUser(req as Request)).toBeUndefined();
  });

  it('should return undefined for empty session', () => {
    const req = { session: {} } as Partial<Request>;
    expect(getAuthenticatedUser(req as Request)).toBeUndefined();
  });

  it('should return undefined for session without passport.user', () => {
    const req = { session: { passport: {} } } as Partial<Request>;
    expect(getAuthenticatedUser(req as Request)).toBeUndefined();
  });

  it('should return undefined when email is not a string', () => {
    const req = {
      session: { passport: { user: { email: 123 } } },
      user: { getId: () => 'testuser' },
    } as unknown as Request;
    expect(getAuthenticatedUser(req)).toBeUndefined();
  });

  it('should return undefined when getId is not a function', () => {
    const req = {
      session: { passport: { user: { email: 'test@example.com' } } },
      user: { getId: 'not-a-function' },
    } as unknown as Request;
    expect(getAuthenticatedUser(req)).toBeUndefined();
  });

  it('should return user info for valid authenticated session', () => {
    const req = {
      session: { passport: { user: { email: 'test@example.com' } } },
      user: { getId: () => 'testuser' },
    } as unknown as Request;

    const result = getAuthenticatedUser(req);
    expect(result).toEqual({
      email: 'test@example.com',
      userId: 'testuser',
    });
  });
});

describe('isResourceOwner', () => {
  it('should return false for undefined authUser', () => {
    expect(isResourceOwner(undefined, 'owner')).toBe(false);
  });

  it('should return false when userId does not match ownerId', () => {
    const authUser: AuthenticatedUser = { email: 'test@example.com', userId: 'testuser' };
    expect(isResourceOwner(authUser, 'otheruser')).toBe(false);
  });

  it('should return true when userId matches ownerId', () => {
    const authUser: AuthenticatedUser = { email: 'test@example.com', userId: 'testuser' };
    expect(isResourceOwner(authUser, 'testuser')).toBe(true);
  });
});
