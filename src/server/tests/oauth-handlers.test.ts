// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

jest.mock('jose', () => ({
  createLocalJWKSet: jest.fn(),
  jwtVerify: jest.fn(),
}));

import { Request, Response } from 'express';
import * as admin from 'firebase-admin';

import {
  createGoogleOAuthInitiateHandler,
  createGoogleOAuthCallbackHandler,
  GoogleOAuthHandlerDeps,
  OAuthConfig,
} from '../auth/oauth-handlers';
import { OAuthStateStore } from '../auth/oauth-state';
import { Table } from '../models/table';
import { User } from '../schemas/user_pb';

const mockFetch = jest.fn();
global.fetch = mockFetch;

function createMockStateStore(): jest.Mocked<OAuthStateStore> {
  return {
    create: jest.fn(),
    validate: jest.fn(),
    invalidate: jest.fn(),
  };
}

function createMockFirebaseAdmin(): jest.Mocked<admin.auth.Auth> {
  return {
    getUserByEmail: jest.fn(),
    createUser: jest.fn(),
    updateUser: jest.fn(),
    listUsers: jest.fn(),
  } as unknown as jest.Mocked<admin.auth.Auth>;
}

function createMockUsers(): jest.Mocked<Table<User>> {
  return {
    init: jest.fn(),
    findOne: jest.fn(),
    findOneByScan: jest.fn(),
    findByScan: jest.fn(),
    find: jest.fn(),
    create: jest.fn(),
    update: jest.fn(),
    deleteOne: jest.fn(),
  };
}

function createMockRequest(
  query: Record<string, string> = {},
  body: Record<string, unknown> = {},
): Partial<Request> {
  const loginFn = jest.fn((user: unknown, cb: (err?: Error) => void) => cb());
  return {
    query,
    body,
    login: loginFn as unknown as Request['login'],
  };
}

interface MockResponseResult {
  res: Partial<Response>;
  getStatus: () => number | undefined;
  getBody: () => unknown;
  getRedirectUrl: () => string | undefined;
}

function createMockResponse(): MockResponseResult {
  let status: number | undefined;
  let body: unknown;
  let redirectUrl: string | undefined;

  const res: Partial<Response> = {
    status: jest.fn((s: number) => {
      status = s;
      return res as Response;
    }),
    json: jest.fn((b: unknown) => {
      body = b;
      return res as Response;
    }),
    redirect: jest.fn((url: string) => {
      redirectUrl = url;
      return res as Response;
    }) as unknown as Response['redirect'],
  };

  return {
    res,
    getStatus: () => status,
    getBody: () => body,
    getRedirectUrl: () => redirectUrl,
  };
}

function createGoogleConfig(): OAuthConfig {
  return {
    clientId: 'test-client-id',
    clientSecret: 'test-client-secret',
    authorizationUrl: 'https://accounts.google.com/o/oauth2/v2/auth',
    tokenUrl: 'https://oauth2.googleapis.com/token',
    scopes: ['openid', 'email', 'profile'],
    callbackPath: '/auth/google/callback',
  };
}

function createMockDeps(): GoogleOAuthHandlerDeps {
  return {
    config: createGoogleConfig(),
    stateStore: createMockStateStore(),
    firebaseAdmin: createMockFirebaseAdmin(),
    users: createMockUsers(),
    baseUrl: 'https://app.simlin.com',
  };
}

describe('createGoogleOAuthInitiateHandler', () => {
  beforeEach(() => {
    mockFetch.mockReset();
  });

  it('should redirect to Google authorization URL', async () => {
    const deps = createMockDeps();
    const handler = createGoogleOAuthInitiateHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).create.mockResolvedValue('test-state-123');

    const req = createMockRequest();
    const { res, getRedirectUrl } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    const redirectUrl = getRedirectUrl();
    expect(redirectUrl).toBeDefined();
    expect(redirectUrl).toContain('https://accounts.google.com/o/oauth2/v2/auth');
  });

  it('should include correct scopes', async () => {
    const deps = createMockDeps();
    const handler = createGoogleOAuthInitiateHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).create.mockResolvedValue('test-state-123');

    const req = createMockRequest();
    const { res, getRedirectUrl } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    const redirectUrl = getRedirectUrl()!;
    expect(redirectUrl).toContain('scope=openid+email+profile');
  });

  it('should include state parameter', async () => {
    const deps = createMockDeps();
    const handler = createGoogleOAuthInitiateHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).create.mockResolvedValue('test-state-123');

    const req = createMockRequest();
    const { res, getRedirectUrl } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    const redirectUrl = getRedirectUrl()!;
    expect(redirectUrl).toContain('state=test-state-123');
  });

  it('should store state in state store', async () => {
    const deps = createMockDeps();
    const handler = createGoogleOAuthInitiateHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).create.mockResolvedValue('test-state-123');

    const req = createMockRequest({ returnUrl: '/projects/test' });
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(deps.stateStore.create).toHaveBeenCalledWith('/projects/test');
  });

  it('should include redirect_uri pointing to callback', async () => {
    const deps = createMockDeps();
    const handler = createGoogleOAuthInitiateHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).create.mockResolvedValue('test-state-123');

    const req = createMockRequest();
    const { res, getRedirectUrl } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    const redirectUrl = getRedirectUrl()!;
    expect(redirectUrl).toContain('redirect_uri=https%3A%2F%2Fapp.simlin.com%2Fauth%2Fgoogle%2Fcallback');
  });
});

describe('createGoogleOAuthCallbackHandler', () => {
  beforeEach(() => {
    mockFetch.mockReset();
  });

  describe('state validation', () => {
    it('should return 400 for missing state', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      const req = createMockRequest({ code: 'test-code' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Missing state parameter' });
    });

    it('should return 400 for invalid state', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({ valid: false });

      const req = createMockRequest({ code: 'test-code', state: 'invalid-state' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Invalid or expired state' });
    });

    it('should invalidate state after successful use', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            access_token: 'test-access-token',
            id_token: 'test-id-token',
            expires_in: 3600,
            token_type: 'Bearer',
          }),
        })
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            sub: 'google-123',
            email: 'test@example.com',
            email_verified: true,
            name: 'Test User',
            picture: 'https://example.com/photo.jpg',
          }),
        });

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'test@example.com',
      } as admin.auth.UserRecord);

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(undefined);
      (deps.users as jest.Mocked<Table<User>>).create.mockResolvedValue();

      const req = createMockRequest({ code: 'test-code', state: 'valid-state' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.stateStore.invalidate).toHaveBeenCalledWith('valid-state');
    });
  });

  describe('returnUrl validation', () => {
    it('should redirect to validated returnUrl from state', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/test',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            access_token: 'test-access-token',
            id_token: 'test-id-token',
            expires_in: 3600,
            token_type: 'Bearer',
          }),
        })
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            sub: 'google-123',
            email: 'test@example.com',
            email_verified: true,
            name: 'Test User',
          }),
        });

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'test@example.com',
      } as admin.auth.UserRecord);

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(undefined);
      (deps.users as jest.Mocked<Table<User>>).create.mockResolvedValue();

      const req = createMockRequest({ code: 'test-code', state: 'valid-state' });
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getRedirectUrl()).toBe('/projects/test');
    });

    it('should redirect to / if no returnUrl', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: undefined,
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            access_token: 'test-access-token',
            id_token: 'test-id-token',
            expires_in: 3600,
            token_type: 'Bearer',
          }),
        })
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            sub: 'google-123',
            email: 'test@example.com',
            email_verified: true,
            name: 'Test User',
          }),
        });

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'test@example.com',
      } as admin.auth.UserRecord);

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(undefined);
      (deps.users as jest.Mocked<Table<User>>).create.mockResolvedValue();

      const req = createMockRequest({ code: 'test-code', state: 'valid-state' });
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getRedirectUrl()).toBe('/');
    });
  });

  describe('user creation', () => {
    it('should create session', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            access_token: 'test-access-token',
            id_token: 'test-id-token',
            expires_in: 3600,
            token_type: 'Bearer',
          }),
        })
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            sub: 'google-123',
            email: 'test@example.com',
            email_verified: true,
            name: 'Test User',
          }),
        });

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'test@example.com',
      } as admin.auth.UserRecord);

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(undefined);
      (deps.users as jest.Mocked<Table<User>>).create.mockResolvedValue();

      const req = createMockRequest({ code: 'test-code', state: 'valid-state' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(req.login).toHaveBeenCalled();
    });

    it('should store provider=google for Google OAuth users', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            access_token: 'test-access-token',
            id_token: 'test-id-token',
            expires_in: 3600,
            token_type: 'Bearer',
          }),
        })
        .mockResolvedValueOnce({
          ok: true,
          json: async () => ({
            sub: 'google-123',
            email: 'test@example.com',
            email_verified: true,
            name: 'Test User',
          }),
        });

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'test@example.com',
      } as admin.auth.UserRecord);

      let createdUser: User | undefined;
      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(undefined);
      (deps.users as jest.Mocked<Table<User>>).create.mockImplementation(async (_id, user) => {
        createdUser = user;
      });

      const req = createMockRequest({ code: 'test-code', state: 'valid-state' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(createdUser).toBeDefined();
      expect(createdUser!.getProvider()).toBe('google');
      expect(createdUser!.getProviderUserId()).toBe('google-123');
    });
  });

  describe('error handling', () => {
    it('should redirect to login page with error on failure', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      mockFetch.mockResolvedValueOnce({
        ok: false,
        text: async () => 'Invalid code',
      });

      const req = createMockRequest({ code: 'invalid-code', state: 'valid-state' });
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getRedirectUrl()).toBe('/?error=oauth_callback_failed');
    });

    it('should handle OAuth error responses', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      const req = createMockRequest({
        error: 'access_denied',
        error_description: 'User denied access',
        state: 'valid-state',
      });
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getRedirectUrl()).toBe('/?error=oauth_denied');
    });
  });
});
