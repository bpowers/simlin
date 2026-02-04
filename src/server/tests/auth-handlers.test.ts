// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response } from 'express';
import * as admin from 'firebase-admin';

import {
  createLoginHandler,
  createSignupHandler,
  createProvidersHandler,
  createResetPasswordHandler,
  createLogoutHandler,
  AuthHandlerDeps,
} from '../auth/auth-handlers';
import { FirebaseRestClient, FirebaseAuthError } from '../auth/firebase-rest-client';
import { Table } from '../models/table';
import { User } from '../schemas/user_pb';

function createMockFirebaseRestClient(): jest.Mocked<FirebaseRestClient> {
  return {
    signInWithPassword: jest.fn(),
    signUp: jest.fn(),
    fetchProviders: jest.fn(),
    sendPasswordResetEmail: jest.fn(),
  };
}

function createMockFirebaseAdmin(): jest.Mocked<admin.auth.Auth> {
  return {
    verifyIdToken: jest.fn(),
    getUser: jest.fn(),
    updateUser: jest.fn(),
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

function createMockUser(id: string, email: string, displayName: string): User {
  const user = new User();
  user.setId(id);
  user.setEmail(email);
  user.setDisplayName(displayName);
  return user;
}

function createMockRequest(body: object = {}): Partial<Request> {
  const loginFn = jest.fn((user: unknown, cb: (err?: Error) => void) => cb());
  const logoutFn = jest.fn((cb: (err?: Error) => void) => cb());
  return {
    body,
    session: {} as Request['session'],
    login: loginFn as unknown as Request['login'],
    logout: logoutFn as unknown as Request['logout'],
  };
}

interface MockResponseResult {
  res: Partial<Response>;
  getStatus: () => number;
  getBody: () => unknown;
}

function createMockResponse(): MockResponseResult {
  let status = 200;
  let body: unknown;
  const res: Partial<Response> = {
    status: jest.fn((s: number) => {
      status = s;
      return res as Response;
    }),
    json: jest.fn((b: unknown) => {
      body = b;
      return res as Response;
    }),
    sendStatus: jest.fn((s: number) => {
      status = s;
      return res as Response;
    }),
  };
  return { res, getStatus: () => status, getBody: () => body };
}

function createMockDeps(): AuthHandlerDeps & {
  firebaseRestClient: jest.Mocked<FirebaseRestClient>;
  firebaseAdmin: jest.Mocked<admin.auth.Auth>;
  users: jest.Mocked<Table<User>>;
} {
  return {
    firebaseRestClient: createMockFirebaseRestClient(),
    firebaseAdmin: createMockFirebaseAdmin(),
    users: createMockUsers(),
    baseUrl: 'http://localhost:3030',
  };
}

describe('createLoginHandler', () => {
  describe('validation', () => {
    it('should return 400 if email missing', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      const req = createMockRequest({ password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Email is required' });
    });

    it('should return 400 if password missing', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      const req = createMockRequest({ email: 'test@example.com' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Password is required' });
    });

    it('should return 400 if email is empty string', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      const req = createMockRequest({ email: '   ', password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Email is required' });
    });

    it('should trim whitespace from email but not password', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockResolvedValue({
        idToken: 'token123',
        email: 'test@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'test@example.com',
        emailVerified: true,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(createMockUser('testuser', 'test@example.com', 'Test User'));

      // Email should be trimmed, but password should NOT be trimmed
      // (Firebase treats leading/trailing spaces in passwords as significant)
      const req = createMockRequest({ email: '  test@example.com  ', password: '  password123  ' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      // Email trimmed, password preserved with spaces
      expect(deps.firebaseRestClient.signInWithPassword).toHaveBeenCalledWith('test@example.com', '  password123  ');
    });
  });

  describe('successful login', () => {
    it('should call firebaseRestClient.signInWithPassword with credentials', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockResolvedValue({
        idToken: 'token123',
        email: 'test@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'test@example.com',
        emailVerified: true,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(createMockUser('testuser', 'test@example.com', 'Test User'));

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.firebaseRestClient.signInWithPassword).toHaveBeenCalledWith('test@example.com', 'password123');
    });

    it('should verify idToken with admin SDK', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockResolvedValue({
        idToken: 'token123',
        email: 'test@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'test@example.com',
        emailVerified: true,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(createMockUser('testuser', 'test@example.com', 'Test User'));

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.firebaseAdmin.verifyIdToken).toHaveBeenCalledWith('token123');
    });

    it('should call req.login to create session', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockResolvedValue({
        idToken: 'token123',
        email: 'test@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'test@example.com',
        emailVerified: true,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      const mockUser = createMockUser('testuser', 'test@example.com', 'Test User');
      deps.users.findOneByScan.mockResolvedValue(mockUser);

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(req.login).toHaveBeenCalledWith(mockUser, expect.any(Function));
    });

    it('should return 200 with user data', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockResolvedValue({
        idToken: 'token123',
        email: 'test@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'test@example.com',
        emailVerified: true,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(createMockUser('testuser', 'test@example.com', 'Test User'));

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(200);
      expect(getBody()).toEqual({
        success: true,
        user: {
          id: 'testuser',
          email: 'test@example.com',
          displayName: 'Test User',
        },
      });
    });
  });

  describe('error handling', () => {
    it('should return 401 for INVALID_PASSWORD error', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockRejectedValue(
        new FirebaseAuthError('INVALID_PASSWORD', 'Incorrect password'),
      );

      const req = createMockRequest({ email: 'test@example.com', password: 'wrongpass' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(401);
      expect(getBody()).toEqual({ error: 'Incorrect password' });
    });

    it('should return 401 for EMAIL_NOT_FOUND error', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockRejectedValue(
        new FirebaseAuthError('EMAIL_NOT_FOUND', 'No account found with this email'),
      );

      const req = createMockRequest({ email: 'unknown@example.com', password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(401);
      expect(getBody()).toEqual({ error: 'No account found with this email' });
    });

    it('should return 403 for USER_DISABLED error', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockRejectedValue(
        new FirebaseAuthError('USER_DISABLED', 'This account has been disabled'),
      );

      const req = createMockRequest({ email: 'disabled@example.com', password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(403);
      expect(getBody()).toEqual({ error: 'This account has been disabled' });
    });

    it('should return 500 for unexpected errors', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      deps.firebaseRestClient.signInWithPassword.mockRejectedValue(new Error('Network error'));

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(500);
      expect(getBody()).toEqual({ error: 'An unexpected error occurred' });
    });

    it('should not include stack traces in error responses', async () => {
      const deps = createMockDeps();
      const handler = createLoginHandler(deps);

      const errorWithStack = new Error('Network error');
      errorWithStack.stack = 'Error: Network error\n    at someFunction (file.js:123)';
      deps.firebaseRestClient.signInWithPassword.mockRejectedValue(errorWithStack);

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      const body = getBody() as { error: string; stack?: string };
      expect(body.stack).toBeUndefined();
    });
  });
});

describe('createSignupHandler', () => {
  describe('validation', () => {
    it('should return 400 if displayName missing', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      const req = createMockRequest({ email: 'test@example.com', password: 'password123' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Display name is required' });
    });

    it('should return 400 if password too short', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockRejectedValue(
        new FirebaseAuthError('WEAK_PASSWORD', 'Password must be at least 6 characters'),
      );

      const req = createMockRequest({ email: 'test@example.com', password: '123', displayName: 'Test' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Password must be at least 6 characters' });
    });
  });

  describe('successful signup', () => {
    it('should create Firebase user', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockResolvedValue({
        idToken: 'token123',
        email: 'new@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.updateUser.mockResolvedValue({} as admin.auth.UserRecord);
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'new@example.com',
        displayName: 'New User',
        emailVerified: false,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(undefined);
      deps.users.create.mockResolvedValue();

      const req = createMockRequest({ email: 'new@example.com', password: 'password123', displayName: 'New User' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.firebaseRestClient.signUp).toHaveBeenCalledWith('new@example.com', 'password123', 'New User');
    });

    it('should set displayName via updateUser', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockResolvedValue({
        idToken: 'token123',
        email: 'new@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.updateUser.mockResolvedValue({} as admin.auth.UserRecord);
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'new@example.com',
        displayName: 'New User',
        emailVerified: false,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(undefined);
      deps.users.create.mockResolvedValue();

      const req = createMockRequest({ email: 'new@example.com', password: 'password123', displayName: 'New User' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.firebaseAdmin.updateUser).toHaveBeenCalledWith('uid123', { displayName: 'New User' });
    });

    it('should create local user record with temp- prefix', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockResolvedValue({
        idToken: 'token123',
        email: 'new@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.updateUser.mockResolvedValue({} as admin.auth.UserRecord);
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'new@example.com',
        displayName: 'New User',
        emailVerified: false,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(undefined);
      deps.users.create.mockResolvedValue();

      const req = createMockRequest({ email: 'new@example.com', password: 'password123', displayName: 'New User' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(deps.users.create).toHaveBeenCalled();
      const [userId] = deps.users.create.mock.calls[0];
      expect(userId).toMatch(/^temp-/);
    });

    it('should create session', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockResolvedValue({
        idToken: 'token123',
        email: 'new@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.updateUser.mockResolvedValue({} as admin.auth.UserRecord);
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'new@example.com',
        displayName: 'New User',
        emailVerified: false,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(undefined);
      deps.users.create.mockResolvedValue();

      const req = createMockRequest({ email: 'new@example.com', password: 'password123', displayName: 'New User' });
      const { res } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(req.login).toHaveBeenCalled();
    });

    it('should return 201 with user data', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockResolvedValue({
        idToken: 'token123',
        email: 'new@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
      });
      deps.firebaseAdmin.verifyIdToken.mockResolvedValue({
        uid: 'uid123',
        aud: '',
        auth_time: 0,
        exp: 0,
        iat: 0,
        iss: '',
        sub: '',
        firebase: { identities: {}, sign_in_provider: 'password' },
      });
      deps.firebaseAdmin.updateUser.mockResolvedValue({} as admin.auth.UserRecord);
      deps.firebaseAdmin.getUser.mockResolvedValue({
        uid: 'uid123',
        email: 'new@example.com',
        displayName: 'New User',
        emailVerified: false,
        disabled: false,
        metadata: { creationTime: '', lastSignInTime: '' },
        providerData: [{ providerId: 'password', uid: 'uid123' }],
        toJSON: () => ({}),
      } as admin.auth.UserRecord);
      deps.users.findOneByScan.mockResolvedValue(undefined);
      deps.users.create.mockResolvedValue();

      const req = createMockRequest({ email: 'new@example.com', password: 'password123', displayName: 'New User' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(201);
      const body = getBody() as { success: boolean; user: { id: string; email: string; displayName: string } };
      expect(body.success).toBe(true);
      expect(body.user.email).toBe('new@example.com');
      expect(body.user.displayName).toBe('New User');
      expect(body.user.id).toMatch(/^temp-/);
    });
  });

  describe('error handling', () => {
    it('should return 409 for EMAIL_EXISTS error', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockRejectedValue(
        new FirebaseAuthError('EMAIL_EXISTS', 'An account with this email already exists'),
      );

      const req = createMockRequest({ email: 'existing@example.com', password: 'password123', displayName: 'Test' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(409);
      expect(getBody()).toEqual({ error: 'An account with this email already exists' });
    });

    it('should return 400 for WEAK_PASSWORD error', async () => {
      const deps = createMockDeps();
      const handler = createSignupHandler(deps);

      deps.firebaseRestClient.signUp.mockRejectedValue(
        new FirebaseAuthError('WEAK_PASSWORD', 'Password must be at least 6 characters'),
      );

      const req = createMockRequest({ email: 'test@example.com', password: '123', displayName: 'Test' });
      const { res, getStatus, getBody } = createMockResponse();

      await handler(req as Request, res as Response, jest.fn());

      expect(getStatus()).toBe(400);
      expect(getBody()).toEqual({ error: 'Password must be at least 6 characters' });
    });
  });
});

describe('createProvidersHandler', () => {
  it('should return providers for registered email', async () => {
    const deps = createMockDeps();
    const handler = createProvidersHandler(deps);

    deps.firebaseRestClient.fetchProviders.mockResolvedValue({
      registered: true,
      providers: ['password', 'google.com'],
    });

    const req = createMockRequest({ email: 'test@example.com' });
    const { res, getStatus, getBody } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(getStatus()).toBe(200);
    expect(getBody()).toEqual({
      registered: true,
      providers: ['password', 'google.com'],
    });
  });

  it('should return empty array for unregistered email', async () => {
    const deps = createMockDeps();
    const handler = createProvidersHandler(deps);

    deps.firebaseRestClient.fetchProviders.mockResolvedValue({
      registered: false,
      providers: [],
    });

    const req = createMockRequest({ email: 'unknown@example.com' });
    const { res, getStatus, getBody } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(getStatus()).toBe(200);
    expect(getBody()).toEqual({
      registered: false,
      providers: [],
    });
  });

  it('should return 400 for missing email', async () => {
    const deps = createMockDeps();
    const handler = createProvidersHandler(deps);

    const req = createMockRequest({});
    const { res, getStatus, getBody } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(getStatus()).toBe(400);
    expect(getBody()).toEqual({ error: 'Email is required' });
  });

  it('should use baseUrl for continueUri', async () => {
    const deps = createMockDeps();
    deps.baseUrl = 'https://app.simlin.com';
    const handler = createProvidersHandler(deps);

    deps.firebaseRestClient.fetchProviders.mockResolvedValue({
      registered: true,
      providers: ['password'],
    });

    const req = createMockRequest({ email: 'test@example.com' });
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(deps.firebaseRestClient.fetchProviders).toHaveBeenCalledWith(
      'test@example.com',
      'https://app.simlin.com/auth/callback',
    );
  });
});

describe('createResetPasswordHandler', () => {
  it('should call sendPasswordResetEmail', async () => {
    const deps = createMockDeps();
    const handler = createResetPasswordHandler(deps);

    deps.firebaseRestClient.sendPasswordResetEmail.mockResolvedValue();

    const req = createMockRequest({ email: 'test@example.com' });
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(deps.firebaseRestClient.sendPasswordResetEmail).toHaveBeenCalledWith('test@example.com');
  });

  it('should return 200 success even for non-existent email', async () => {
    const deps = createMockDeps();
    const handler = createResetPasswordHandler(deps);

    deps.firebaseRestClient.sendPasswordResetEmail.mockResolvedValue();

    const req = createMockRequest({ email: 'unknown@example.com' });
    const { res, getStatus, getBody } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(getStatus()).toBe(200);
    expect(getBody()).toEqual({ success: true });
  });

  it('should return 400 for missing email', async () => {
    const deps = createMockDeps();
    const handler = createResetPasswordHandler(deps);

    const req = createMockRequest({});
    const { res, getStatus, getBody } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(getStatus()).toBe(400);
    expect(getBody()).toEqual({ error: 'Email is required' });
  });
});

describe('createLogoutHandler', () => {
  it('should call req.logout', async () => {
    const handler = createLogoutHandler();

    const req = createMockRequest({});
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(req.logout).toHaveBeenCalled();
  });

  it('should clear session', async () => {
    const handler = createLogoutHandler();

    const req = createMockRequest({});
    (req.session as Record<string, unknown>).passport = { user: { id: 'test' } };
    const { res } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(req.session).toEqual({});
  });

  it('should return 200', async () => {
    const handler = createLogoutHandler();

    const req = createMockRequest({});
    const { res, getStatus } = createMockResponse();

    await handler(req as Request, res as Response, jest.fn());

    expect(getStatus()).toBe(200);
  });
});
