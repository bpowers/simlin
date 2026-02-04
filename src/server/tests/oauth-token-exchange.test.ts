// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

jest.mock('jose', () => ({
  createLocalJWKSet: jest.fn(),
  jwtVerify: jest.fn(),
}));

import * as crypto from 'crypto';

import { generateAppleClientSecret, verifyAppleIdToken, clearJwksCache } from '../auth/oauth-token-exchange';

describe('generateAppleClientSecret', () => {
  let testPrivateKey: string;
  let testPublicKey: crypto.KeyObject;

  beforeAll(() => {
    // Generate a test EC key pair for ES256
    const { privateKey, publicKey } = crypto.generateKeyPairSync('ec', {
      namedCurve: 'prime256v1',
    });
    testPrivateKey = privateKey.export({ type: 'pkcs8', format: 'pem' }) as string;
    testPublicKey = publicKey;
  });

  it('should generate a valid ES256 JWT with verifiable signature', () => {
    const teamId = 'TEST_TEAM';
    const clientId = 'com.test.app';
    const keyId = 'TEST_KEY_ID';

    const jwt = generateAppleClientSecret(teamId, clientId, keyId, testPrivateKey);

    // JWT should have three parts
    const parts = jwt.split('.');
    expect(parts).toHaveLength(3);

    const [headerB64, payloadB64, signatureB64] = parts;

    // Verify header
    const header = JSON.parse(Buffer.from(headerB64, 'base64url').toString());
    expect(header.alg).toBe('ES256');
    expect(header.kid).toBe(keyId);

    // Verify payload
    const payload = JSON.parse(Buffer.from(payloadB64, 'base64url').toString());
    expect(payload.iss).toBe(teamId);
    expect(payload.sub).toBe(clientId);
    expect(payload.aud).toBe('https://appleid.apple.com');
    expect(payload.iat).toBeDefined();
    expect(payload.exp).toBeDefined();

    // Verify the signature using crypto.verify with ieee-p1363 encoding
    const signingInput = `${headerB64}.${payloadB64}`;
    const signature = Buffer.from(signatureB64, 'base64url');

    const isValid = crypto.verify(
      'SHA256',
      Buffer.from(signingInput),
      {
        key: testPublicKey,
        dsaEncoding: 'ieee-p1363',
      },
      signature,
    );

    expect(isValid).toBe(true);
  });

  it('should set expiration to approximately 6 months', () => {
    const teamId = 'TEST_TEAM';
    const clientId = 'com.test.app';
    const keyId = 'TEST_KEY_ID';

    const jwt = generateAppleClientSecret(teamId, clientId, keyId, testPrivateKey);

    const parts = jwt.split('.');
    const payload = JSON.parse(Buffer.from(parts[1], 'base64url').toString());

    const sixMonthsInSeconds = 15777000;
    const expiresIn = payload.exp - payload.iat;
    expect(expiresIn).toBe(sixMonthsInSeconds);
  });

  it('should generate a 64-byte signature (ES256 r||s format)', () => {
    const teamId = 'TEST_TEAM';
    const clientId = 'com.test.app';
    const keyId = 'TEST_KEY_ID';

    const jwt = generateAppleClientSecret(teamId, clientId, keyId, testPrivateKey);

    const parts = jwt.split('.');
    const signature = Buffer.from(parts[2], 'base64url');

    // ES256 signature in ieee-p1363 format is exactly 64 bytes (32 bytes r + 32 bytes s)
    expect(signature.length).toBe(64);
  });
});

describe('verifyAppleIdToken', () => {
  const jose = require('jose');

  beforeEach(() => {
    clearJwksCache();
    jest.clearAllMocks();
  });

  it('should convert email_verified string "true" to boolean true', async () => {
    // Mock fetch for JWKS
    global.fetch = jest.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ keys: [] }),
    });

    // Mock jose functions
    jose.createLocalJWKSet.mockReturnValue(jest.fn());
    jose.jwtVerify.mockResolvedValue({
      payload: {
        sub: 'apple-user-123',
        email: 'test@example.com',
        email_verified: 'true', // Apple sends string, not boolean
      },
    });

    const result = await verifyAppleIdToken('mock-token', { clientId: 'test-client-id' });

    expect(result.sub).toBe('apple-user-123');
    expect(result.email).toBe('test@example.com');
    expect(result.email_verified).toBe(true); // Should be boolean, not string
    expect(typeof result.email_verified).toBe('boolean');
  });

  it('should convert email_verified string "false" to boolean false', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ keys: [] }),
    });

    jose.createLocalJWKSet.mockReturnValue(jest.fn());
    jose.jwtVerify.mockResolvedValue({
      payload: {
        sub: 'apple-user-456',
        email: 'test@example.com',
        email_verified: 'false', // Apple sends string, not boolean
      },
    });

    const result = await verifyAppleIdToken('mock-token', { clientId: 'test-client-id' });

    expect(result.email_verified).toBe(false);
    expect(typeof result.email_verified).toBe('boolean');
  });

  it('should handle boolean email_verified values', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ keys: [] }),
    });

    jose.createLocalJWKSet.mockReturnValue(jest.fn());
    jose.jwtVerify.mockResolvedValue({
      payload: {
        sub: 'apple-user-789',
        email: 'test@example.com',
        email_verified: true, // In case Apple sends actual boolean
      },
    });

    const result = await verifyAppleIdToken('mock-token', { clientId: 'test-client-id' });

    expect(result.email_verified).toBe(true);
    expect(typeof result.email_verified).toBe('boolean');
  });

  it('should handle missing email_verified', async () => {
    global.fetch = jest.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ keys: [] }),
    });

    jose.createLocalJWKSet.mockReturnValue(jest.fn());
    jose.jwtVerify.mockResolvedValue({
      payload: {
        sub: 'apple-user-000',
        email: 'test@example.com',
        // no email_verified
      },
    });

    const result = await verifyAppleIdToken('mock-token', { clientId: 'test-client-id' });

    expect(result.email_verified).toBeUndefined();
  });
});
