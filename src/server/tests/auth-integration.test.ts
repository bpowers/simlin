// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { createFirebaseRestClient, FirebaseRestClient } from '../auth/firebase-rest-client';

const EMULATOR_HOST = process.env.FIREBASE_AUTH_EMULATOR_HOST;
const describeWithEmulator = EMULATOR_HOST ? describe : describe.skip;

async function clearEmulatorUsers(): Promise<void> {
  if (!EMULATOR_HOST) return;

  try {
    await fetch(`http://${EMULATOR_HOST}/emulator/v1/projects/simlin/accounts`, {
      method: 'DELETE',
    });
  } catch {
    // Ignore errors - emulator might not be fully ready
  }
}

describeWithEmulator('Auth Integration Tests', () => {
  let client: FirebaseRestClient;

  beforeAll(() => {
    client = createFirebaseRestClient({
      apiKey: 'fake-api-key',
      emulatorHost: EMULATOR_HOST,
    });
  });

  afterEach(async () => {
    await clearEmulatorUsers();
  });

  describe('signup + login flow', () => {
    it('should create user and login successfully', async () => {
      const signupResult = await client.signUp('test@example.com', 'password123', 'Test User');
      expect(signupResult.email).toBe('test@example.com');
      expect(signupResult.idToken).toBeDefined();
      expect(signupResult.localId).toBeDefined();

      const loginResult = await client.signInWithPassword('test@example.com', 'password123');
      expect(loginResult.email).toBe('test@example.com');
      expect(loginResult.idToken).toBeDefined();
    });

    it('should reject login with wrong password', async () => {
      await client.signUp('test@example.com', 'password123', 'Test User');

      await expect(client.signInWithPassword('test@example.com', 'wrongpassword')).rejects.toMatchObject({
        code: 'INVALID_PASSWORD',
      });
    });

    it('should reject signup with existing email', async () => {
      await client.signUp('test@example.com', 'password123', 'Test User');

      await expect(client.signUp('test@example.com', 'password456', 'Test User 2')).rejects.toMatchObject({
        code: 'EMAIL_EXISTS',
      });
    });

    it('should reject login for non-existent user', async () => {
      await expect(client.signInWithPassword('nonexistent@example.com', 'password123')).rejects.toMatchObject({
        code: 'EMAIL_NOT_FOUND',
      });
    });
  });

  describe('providers check', () => {
    it('should return password provider for email/password user', async () => {
      await client.signUp('test@example.com', 'password123', 'Test User');

      const result = await client.fetchProviders('test@example.com', 'http://localhost');
      expect(result.registered).toBe(true);
      expect(result.providers).toContain('password');
    });

    it('should return registered=false for unknown email', async () => {
      const result = await client.fetchProviders('unknown@example.com', 'http://localhost');
      expect(result.registered).toBe(false);
    });
  });

  describe('password reset', () => {
    it('should not throw for existing user', async () => {
      await client.signUp('test@example.com', 'password123', 'Test User');
      await expect(client.sendPasswordResetEmail('test@example.com')).resolves.not.toThrow();
    });

    it('should not throw for non-existent user', async () => {
      await expect(client.sendPasswordResetEmail('unknown@example.com')).resolves.not.toThrow();
    });
  });
});

describe('Auth Integration Tests (no emulator)', () => {
  it('should indicate whether emulator is available', () => {
    if (EMULATOR_HOST) {
      console.log(`Firebase Auth emulator available at ${EMULATOR_HOST}`);
    } else {
      console.log('Firebase Auth emulator not available, skipping integration tests');
      console.log('Set FIREBASE_AUTH_EMULATOR_HOST=127.0.0.1:9099 to run integration tests');
    }
    expect(true).toBe(true);
  });
});
