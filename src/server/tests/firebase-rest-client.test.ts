// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { createFirebaseRestClient, FirebaseRestClient, FirebaseAuthError } from '../auth/firebase-rest-client';

const mockFetch = jest.fn();
global.fetch = mockFetch;

function createSuccessResponse(data: object) {
  return {
    ok: true,
    json: async () => data,
  };
}

function createErrorResponse(code: number, message: string) {
  return {
    ok: false,
    status: code,
    json: async () => ({ error: { code, message } }),
  };
}

describe('FirebaseRestClient', () => {
  let client: FirebaseRestClient;
  const apiKey = 'test-api-key';

  beforeEach(() => {
    mockFetch.mockReset();
    client = createFirebaseRestClient({ apiKey });
  });

  describe('signInWithPassword', () => {
    it('should construct correct URL with API key', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          idToken: 'token123',
          email: 'test@example.com',
          refreshToken: 'refresh123',
          expiresIn: '3600',
          localId: 'uid123',
        }),
      );

      await client.signInWithPassword('test@example.com', 'password123');

      expect(mockFetch).toHaveBeenCalledWith(
        `https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key=${apiKey}`,
        expect.any(Object),
      );
    });

    it('should send email, password, returnSecureToken in body', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          idToken: 'token123',
          email: 'test@example.com',
          refreshToken: 'refresh123',
          expiresIn: '3600',
          localId: 'uid123',
        }),
      );

      await client.signInWithPassword('test@example.com', 'password123');

      const [, options] = mockFetch.mock.calls[0];
      const body = JSON.parse(options.body);
      expect(body).toEqual({
        email: 'test@example.com',
        password: 'password123',
        returnSecureToken: true,
      });
    });

    it('should return parsed response on 200', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          idToken: 'token123',
          email: 'test@example.com',
          refreshToken: 'refresh123',
          expiresIn: '3600',
          localId: 'uid123',
          displayName: 'Test User',
        }),
      );

      const result = await client.signInWithPassword('test@example.com', 'password123');

      expect(result).toEqual({
        idToken: 'token123',
        email: 'test@example.com',
        refreshToken: 'refresh123',
        expiresIn: '3600',
        localId: 'uid123',
        displayName: 'Test User',
      });
    });

    it('should use emulator URL when emulatorHost configured', async () => {
      const emulatorClient = createFirebaseRestClient({
        apiKey,
        emulatorHost: '127.0.0.1:9099',
      });

      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          idToken: 'token123',
          email: 'test@example.com',
          refreshToken: 'refresh123',
          expiresIn: '3600',
          localId: 'uid123',
        }),
      );

      await emulatorClient.signInWithPassword('test@example.com', 'password123');

      expect(mockFetch).toHaveBeenCalledWith(
        `http://127.0.0.1:9099/identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key=${apiKey}`,
        expect.any(Object),
      );
    });

    it('should throw typed error on INVALID_PASSWORD', async () => {
      mockFetch.mockResolvedValueOnce(createErrorResponse(400, 'INVALID_PASSWORD'));

      await expect(client.signInWithPassword('test@example.com', 'wrongpass')).rejects.toMatchObject({
        code: 'INVALID_PASSWORD',
        message: 'Incorrect password',
      });
    });

    it('should throw typed error on EMAIL_NOT_FOUND', async () => {
      mockFetch.mockResolvedValueOnce(createErrorResponse(400, 'EMAIL_NOT_FOUND'));

      await expect(client.signInWithPassword('unknown@example.com', 'password')).rejects.toMatchObject({
        code: 'EMAIL_NOT_FOUND',
        message: 'No account found with this email',
      });
    });

    it('should throw typed error on USER_DISABLED', async () => {
      mockFetch.mockResolvedValueOnce(createErrorResponse(400, 'USER_DISABLED'));

      await expect(client.signInWithPassword('disabled@example.com', 'password')).rejects.toMatchObject({
        code: 'USER_DISABLED',
        message: 'This account has been disabled',
      });
    });
  });

  describe('signUp', () => {
    it('should include displayName in request body', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          idToken: 'token123',
          email: 'new@example.com',
          refreshToken: 'refresh123',
          expiresIn: '3600',
          localId: 'uid123',
        }),
      );

      await client.signUp('new@example.com', 'password123', 'New User');

      const [, options] = mockFetch.mock.calls[0];
      const body = JSON.parse(options.body);
      expect(body).toEqual({
        email: 'new@example.com',
        password: 'password123',
        displayName: 'New User',
        returnSecureToken: true,
      });
    });

    it('should throw on EMAIL_EXISTS', async () => {
      mockFetch.mockResolvedValueOnce(createErrorResponse(400, 'EMAIL_EXISTS'));

      await expect(client.signUp('existing@example.com', 'password123', 'User')).rejects.toMatchObject({
        code: 'EMAIL_EXISTS',
        message: 'An account with this email already exists',
      });
    });

    it('should throw on WEAK_PASSWORD', async () => {
      mockFetch.mockResolvedValueOnce(createErrorResponse(400, 'WEAK_PASSWORD : Password should be at least 6'));

      await expect(client.signUp('new@example.com', '123', 'User')).rejects.toMatchObject({
        code: 'WEAK_PASSWORD',
        message: 'Password must be at least 6 characters',
      });
    });
  });

  describe('fetchProviders', () => {
    it('should return providers array for registered user', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          registered: true,
          allProviders: ['password', 'google.com'],
          signinMethods: ['password'],
        }),
      );

      const result = await client.fetchProviders('test@example.com', 'http://localhost');

      expect(result).toEqual({
        registered: true,
        providers: ['password', 'google.com'],
      });
    });

    it('should return empty providers and registered=false for unknown email', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          registered: false,
        }),
      );

      const result = await client.fetchProviders('unknown@example.com', 'http://localhost');

      expect(result).toEqual({
        registered: false,
        providers: [],
      });
    });

    it('should send correct request body', async () => {
      mockFetch.mockResolvedValueOnce(
        createSuccessResponse({
          registered: false,
        }),
      );

      await client.fetchProviders('test@example.com', 'http://localhost/callback');

      const [url, options] = mockFetch.mock.calls[0];
      expect(url).toContain('accounts:createAuthUri');
      const body = JSON.parse(options.body);
      expect(body).toEqual({
        identifier: 'test@example.com',
        continueUri: 'http://localhost/callback',
      });
    });
  });

  describe('sendPasswordResetEmail', () => {
    it('should send requestType PASSWORD_RESET', async () => {
      mockFetch.mockResolvedValueOnce(createSuccessResponse({ email: 'test@example.com' }));

      await client.sendPasswordResetEmail('test@example.com');

      const [url, options] = mockFetch.mock.calls[0];
      expect(url).toContain('accounts:sendOobCode');
      const body = JSON.parse(options.body);
      expect(body).toEqual({
        requestType: 'PASSWORD_RESET',
        email: 'test@example.com',
      });
    });

    it('should not throw for non-existent email', async () => {
      mockFetch.mockResolvedValueOnce(createErrorResponse(400, 'EMAIL_NOT_FOUND'));

      await expect(client.sendPasswordResetEmail('unknown@example.com')).resolves.not.toThrow();
    });
  });
});

describe('FirebaseAuthError', () => {
  it('should be an instance of Error', () => {
    const error = new FirebaseAuthError('TEST_CODE', 'Test message');
    expect(error).toBeInstanceOf(Error);
  });

  it('should have code and message properties', () => {
    const error = new FirebaseAuthError('TEST_CODE', 'Test message');
    expect(error.code).toBe('TEST_CODE');
    expect(error.message).toBe('Test message');
  });
});
