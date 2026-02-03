// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

jest.mock('jose', () => ({
  createLocalJWKSet: jest.fn(),
  jwtVerify: jest.fn(),
}));

jest.mock('../auth/oauth-token-exchange', () => {
  const actual = jest.requireActual('../auth/oauth-token-exchange');
  return {
    ...actual,
    generateAppleClientSecret: jest.fn(() => 'mock-client-secret'),
    exchangeAppleCode: jest.fn(),
    verifyAppleIdToken: jest.fn(),
  };
});

import { Request, Response } from 'express';
import * as admin from 'firebase-admin';

import {
  createGoogleOAuthInitiateHandler,
  createGoogleOAuthCallbackHandler,
  createAppleOAuthCallbackHandler,
  GoogleOAuthHandlerDeps,
  AppleOAuthHandlerDeps,
  OAuthConfig,
  AppleOAuthConfig,
} from '../auth/oauth-handlers';
import { exchangeAppleCode, verifyAppleIdToken } from '../auth/oauth-token-exchange';
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
    getUserByProviderUid: jest.fn(),
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
  cookies: Record<string, string> = {},
): Partial<Request> {
  const loginFn = jest.fn((user: unknown, cb: (err?: Error) => void) => cb());
  return {
    query,
    body,
    cookies,
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
    cookie: jest.fn(() => res as Response) as unknown as Response['cookie'],
    clearCookie: jest.fn(() => res as Response) as unknown as Response['clearCookie'],
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

    expect(deps.stateStore.create).toHaveBeenCalledWith({
      returnUrl: '/projects/test',
      bindingSecret: expect.any(String),
    });
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

  it('should set a per-state binding cookie', async () => {
    const deps = createMockDeps();
    const handler = createGoogleOAuthInitiateHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).create.mockResolvedValue('test-state-123');

    const req = createMockRequest();
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(res.cookie).toHaveBeenCalledWith(
      'oauth_state_test-state-123',
      expect.any(String),
      expect.objectContaining({
        httpOnly: true,
        path: '/auth',
        sameSite: 'none',
        secure: true,
      }),
    );
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

    it('should pass the binding cookie to state validation', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({ valid: false });

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.stateStore.validate).toHaveBeenCalledWith({
        state: 'valid-state',
        bindingSecret: 'binding-cookie',
      });
    });

    it('should reject a callback when the binding cookie is missing', async () => {
      const deps = createMockDeps();
      const handler = createGoogleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({ valid: false });

      const req = createMockRequest({ code: 'test-code', state: 'valid-state' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Invalid or expired state' });
      expect(deps.stateStore.validate).toHaveBeenCalledWith({
        state: 'valid-state',
        bindingSecret: undefined,
      });
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

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
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

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
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

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
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

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
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

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
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

      const req = createMockRequest(
        { code: 'invalid-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
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
      expect(res.clearCookie).toHaveBeenCalledWith(
        'oauth_state_valid-state',
        expect.objectContaining({
          httpOnly: true,
          path: '/auth',
          sameSite: 'none',
          secure: true,
        }),
      );
    });

    it('should reject disabled Firebase users', async () => {
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
            email: 'disabled@example.com',
            email_verified: true,
            name: 'Disabled User',
          }),
        });

      // Return a disabled Firebase user
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'disabled@example.com',
        disabled: true,
      } as admin.auth.UserRecord);

      const req = createMockRequest(
        { code: 'test-code', state: 'valid-state' },
        {},
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should redirect with account disabled error
      expect(getRedirectUrl()).toBe('/?error=account_disabled');

      // Should NOT have called login
      expect(req.login).not.toHaveBeenCalled();
    });
  });
});

function createAppleConfig(): AppleOAuthConfig {
  return {
    clientId: 'com.simlin.app',
    clientSecret: '', // Not used directly, generated dynamically
    authorizationUrl: 'https://appleid.apple.com/auth/authorize',
    tokenUrl: 'https://appleid.apple.com/auth/token',
    scopes: ['name', 'email'],
    callbackPath: '/auth/apple/callback',
    teamId: 'TEAM123',
    keyId: 'KEY456',
    privateKey: '-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----',
  };
}

function createAppleMockDeps(): AppleOAuthHandlerDeps {
  return {
    config: createAppleConfig(),
    stateStore: createMockStateStore(),
    firebaseAdmin: createMockFirebaseAdmin(),
    users: createMockUsers(),
    baseUrl: 'https://app.simlin.com',
  };
}

describe('createAppleOAuthCallbackHandler', () => {
  beforeEach(() => {
    mockFetch.mockReset();
    jest.clearAllMocks();
  });

  it('should pass the binding cookie to Apple state validation', async () => {
    const deps = createAppleMockDeps();
    const handler = createAppleOAuthCallbackHandler(deps);

    (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({ valid: false });

    const req = createMockRequest(
      {},
      { code: 'test-code', state: 'valid-state' },
      { 'oauth_state_valid-state': 'binding-cookie' },
    );
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(deps.stateStore.validate).toHaveBeenCalledWith({
      state: 'valid-state',
      bindingSecret: 'binding-cookie',
    });
  });

  describe('disabled user handling', () => {
    it('should reject disabled Firebase users', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      // Mock Apple token exchange
      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      // Mock verifyAppleIdToken
      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-123',
        email: 'disabled@example.com',
      });

      // Return a disabled Firebase user
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'disabled@example.com',
        disabled: true,
      } as admin.auth.UserRecord);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should redirect with account disabled error
      expect(getRedirectUrl()).toBe('/?error=account_disabled');

      // Should NOT have called login
      expect(req.login).not.toHaveBeenCalled();
    });
  });

  describe('Apple provider linking', () => {
    it('should link Apple in Firebase for an existing Google user', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-sub-123',
        email: 'existing@example.com',
        email_verified: true,
      });

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-uid-123',
        email: 'existing@example.com',
        disabled: false,
        providerData: [{ providerId: 'google.com', uid: 'google-sub-123' }],
      } as admin.auth.UserRecord);

      const existingUser = new User();
      existingUser.setId('user-123');
      existingUser.setEmail('existing@example.com');
      existingUser.setProvider('google');
      existingUser.setProviderUserId('google-sub-123');

      (deps.users as jest.Mocked<Table<User>>).findOneByScan
        .mockResolvedValueOnce(undefined)
        .mockResolvedValueOnce(existingUser);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.firebaseAdmin.updateUser).toHaveBeenCalledWith(
        'fb-uid-123',
        expect.objectContaining({
          providerToLink: expect.objectContaining({
            providerId: 'apple.com',
            uid: 'apple-sub-123',
            email: 'existing@example.com',
          }),
        }),
      );
      expect(deps.users.update).not.toHaveBeenCalled();
      expect(req.login).toHaveBeenCalledWith(existingUser, expect.any(Function));
      expect(getRedirectUrl()).toBe('/');
    });
  });

  describe('returning user without email', () => {
    it('should reject disabled Firebase users even in no-email path', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/test',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      // Mock Apple token exchange
      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      // Mock verifyAppleIdToken to return claims WITHOUT email
      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-disabled-user',
        // no email
      });

      // User exists in local database by providerUserId
      const existingUser = new User();
      existingUser.setId('user-disabled-123');
      existingUser.setEmail('disabled@example.com');
      existingUser.setProvider('apple');
      existingUser.setProviderUserId('apple-disabled-user');

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(existingUser);

      // Firebase says user is disabled
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByProviderUid.mockResolvedValue({
        uid: 'fb-disabled-user',
        disabled: true,
      } as admin.auth.UserRecord);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should check Firebase disabled status
      expect(deps.firebaseAdmin.getUserByProviderUid).toHaveBeenCalledWith('apple.com', 'apple-disabled-user');

      // Should redirect with account disabled error
      expect(getRedirectUrl()).toBe('/?error=account_disabled');

      // Should NOT have called login
      expect(req.login).not.toHaveBeenCalled();
    });

    it('should fallback to email check when provider lookup fails and block disabled users', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/test',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      // Apple ID token without email
      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-user-no-link',
      });

      // User exists in local database
      const existingUser = new User();
      existingUser.setId('user-123');
      existingUser.setEmail('disabled@example.com');
      existingUser.setProvider('apple');
      existingUser.setProviderUserId('apple-user-no-link');

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(existingUser);

      // getUserByProviderUid throws (no Apple provider link in Firebase)
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByProviderUid.mockRejectedValue(
        new Error('User not found'),
      );

      // getUserByEmail finds the user but they're disabled
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockResolvedValue({
        uid: 'fb-user-123',
        email: 'disabled@example.com',
        disabled: true,
      } as admin.auth.UserRecord);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should fallback to email check
      expect(deps.firebaseAdmin.getUserByEmail).toHaveBeenCalledWith('disabled@example.com');

      // Should redirect with account disabled error
      expect(getRedirectUrl()).toBe('/?error=account_disabled');

      // Should NOT have called login
      expect(req.login).not.toHaveBeenCalled();
    });

    it('should reject login when Firebase disabled status cannot be verified (fail-closed)', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/test',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-user-unverifiable',
      });

      const existingUser = new User();
      existingUser.setId('user-unverifiable');
      existingUser.setEmail('user@example.com');
      existingUser.setProvider('apple');
      existingUser.setProviderUserId('apple-user-unverifiable');

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(existingUser);

      // Both Firebase lookups fail (e.g., Firebase is temporarily unavailable)
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByProviderUid.mockRejectedValue(
        new Error('Service unavailable'),
      );
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByEmail.mockRejectedValue(
        new Error('Service unavailable'),
      );

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should reject login when disabled status cannot be verified
      // (returns account_disabled since we fail closed)
      expect(getRedirectUrl()).toBe('/?error=account_disabled');
      expect(req.login).not.toHaveBeenCalled();
    });

    it('should login existing user by providerUserId when Apple omits email', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/test',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      // Mock Apple token exchange
      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      // Mock verifyAppleIdToken to return claims WITHOUT email (returning user)
      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-user-123',
        // no email - common for returning Apple users
      });

      // User exists in local database by providerUserId
      const existingUser = new User();
      existingUser.setId('user-id-123');
      existingUser.setEmail('user@example.com');
      existingUser.setProvider('apple');
      existingUser.setProviderUserId('apple-user-123');

      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(existingUser);

      // Firebase says user is NOT disabled
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByProviderUid.mockResolvedValue({
        uid: 'fb-user-123',
        disabled: false,
      } as admin.auth.UserRecord);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should find user by providerUserId AND provider (prevents cross-provider collisions)
      expect(deps.users.findOneByScan).toHaveBeenCalledWith({ providerUserId: 'apple-user-123', provider: 'apple' });

      // Should login the existing user
      expect(req.login).toHaveBeenCalledWith(existingUser, expect.any(Function));

      // Should redirect to the returnUrl
      expect(getRedirectUrl()).toBe('/projects/test');
    });

    it('should return error only if no email AND user not found by providerUserId', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      // Mock Apple token exchange
      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      // Mock verifyAppleIdToken to return claims WITHOUT email
      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-user-unknown',
        // no email
      });

      // User does NOT exist in local database
      (deps.users as jest.Mocked<Table<User>>).findOneByScan.mockResolvedValue(undefined);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should try to find user by providerUserId AND provider
      expect(deps.users.findOneByScan).toHaveBeenCalledWith({ providerUserId: 'apple-user-unknown', provider: 'apple' });

      // Should redirect with error since user not found and no email to create one
      expect(getRedirectUrl()).toBe('/?error=apple_no_email');
    });

    it('should fall back to Firebase provider lookup for pre-migration users without email', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/migrated',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      // Mock Apple token exchange
      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      // Mock verifyAppleIdToken to return claims WITHOUT email (returning user)
      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-pre-migration-user',
        // no email - common for returning Apple users
      });

      // User does NOT exist by providerUserId (wasn't stored before migration)
      // But DOES exist by email (found via Firebase provider lookup)
      const existingUser = new User();
      existingUser.setId('user-legacy-456');
      existingUser.setEmail('legacy@example.com');
      existingUser.setProvider('password'); // Was originally password user who added Apple
      existingUser.setProviderUserId(''); // No providerUserId before migration

      // First findOneByScan (by providerUserId) returns nothing
      // Second findOneByScan (by email) returns the user
      (deps.users as jest.Mocked<Table<User>>).findOneByScan
        .mockResolvedValueOnce(undefined) // providerUserId lookup
        .mockResolvedValueOnce(existingUser); // email lookup

      // Firebase has this user with Apple provider linked
      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByProviderUid.mockResolvedValue({
        uid: 'fb-legacy-user',
        email: 'legacy@example.com',
        disabled: false,
      } as admin.auth.UserRecord);

      (deps.users as jest.Mocked<Table<User>>).update.mockResolvedValue(existingUser);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Should have looked up Firebase user by Apple provider
      expect(deps.firebaseAdmin.getUserByProviderUid).toHaveBeenCalledWith('apple.com', 'apple-pre-migration-user');

      // Should have updated the user's providerUserId for future logins
      expect(deps.users.update).toHaveBeenCalled();
      const updateCall = (deps.users.update as jest.Mock).mock.calls[0];
      expect(updateCall[0]).toBe('user-legacy-456');
      const updatedUser = updateCall[2] as User;
      expect(updatedUser.getProviderUserId()).toBe('apple-pre-migration-user');
      expect(updatedUser.getProvider()).toBe('apple');

      // Should login and redirect
      expect(req.login).toHaveBeenCalled();
      expect(getRedirectUrl()).toBe('/projects/migrated');
    });

    it('should login an existing Google user by Firebase Apple link without rewriting local provider info', async () => {
      const deps = createAppleMockDeps();
      const handler = createAppleOAuthCallbackHandler(deps);

      (deps.stateStore as jest.Mocked<OAuthStateStore>).validate.mockResolvedValue({
        valid: true,
        returnUrl: '/projects/apple-linked',
      });
      (deps.stateStore as jest.Mocked<OAuthStateStore>).invalidate.mockResolvedValue();

      (exchangeAppleCode as jest.Mock).mockResolvedValue({
        access_token: 'test-access-token',
        id_token: 'test-id-token',
        expires_in: 3600,
        token_type: 'Bearer',
      });

      (verifyAppleIdToken as jest.Mock).mockResolvedValue({
        sub: 'apple-linked-user',
      });

      const existingUser = new User();
      existingUser.setId('user-google-123');
      existingUser.setEmail('existing@example.com');
      existingUser.setProvider('google');
      existingUser.setProviderUserId('google-sub-123');

      (deps.users as jest.Mocked<Table<User>>).findOneByScan
        .mockResolvedValueOnce(undefined)
        .mockResolvedValueOnce(existingUser);

      (deps.firebaseAdmin as jest.Mocked<admin.auth.Auth>).getUserByProviderUid.mockResolvedValue({
        uid: 'fb-google-123',
        email: 'existing@example.com',
        disabled: false,
      } as admin.auth.UserRecord);

      const req = createMockRequest(
        {},
        { code: 'test-code', state: 'valid-state' },
        { 'oauth_state_valid-state': 'binding-cookie' },
      );
      const { res, getRedirectUrl } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.firebaseAdmin.getUserByProviderUid).toHaveBeenCalledWith('apple.com', 'apple-linked-user');
      expect(deps.users.update).not.toHaveBeenCalled();
      expect(existingUser.getProvider()).toBe('google');
      expect(existingUser.getProviderUserId()).toBe('google-sub-123');
      expect(req.login).toHaveBeenCalledWith(existingUser, expect.any(Function));
      expect(getRedirectUrl()).toBe('/projects/apple-linked');
    });
  });
});
