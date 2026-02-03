// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { getOrCreateUserFromVerifiedInfo, VerifiedUserInfo } from '../authn';
import { Table } from '../models/table';
import { User } from '../schemas/user_pb';

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

describe('getOrCreateUserFromVerifiedInfo', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  describe('when user exists by providerUserId', () => {
    it('should return existing user', async () => {
      const users = createMockUsers();

      const existingUser = new User();
      existingUser.setId('user-123');
      existingUser.setEmail('test@example.com');
      existingUser.setProvider('google');
      existingUser.setProviderUserId('google-123');

      users.findOneByScan.mockResolvedValueOnce(existingUser);

      const info: VerifiedUserInfo = {
        email: 'test@example.com',
        displayName: 'Test User',
        provider: 'google',
        providerUserId: 'google-123',
      };

      const [user, err] = await getOrCreateUserFromVerifiedInfo(users, info);

      expect(err).toBeUndefined();
      expect(user).toBe(existingUser);
      expect(users.findOneByScan).toHaveBeenCalledWith({ providerUserId: 'google-123' });
    });
  });

  describe('when user exists by email but not providerUserId', () => {
    it('should update providerUserId on the existing user', async () => {
      const users = createMockUsers();

      // User exists with password auth (no providerUserId)
      const existingUser = new User();
      existingUser.setId('user-123');
      existingUser.setEmail('test@example.com');
      existingUser.setProvider('password');
      existingUser.setProviderUserId('');

      // First call: findOneByScan by providerUserId returns nothing
      // Second call: findOneByScan by email returns existing user
      users.findOneByScan.mockResolvedValueOnce(undefined).mockResolvedValueOnce(existingUser);
      users.update.mockResolvedValue(existingUser);

      const info: VerifiedUserInfo = {
        email: 'test@example.com',
        displayName: 'Test User',
        provider: 'apple',
        providerUserId: 'apple-sub-456',
      };

      const [user, err] = await getOrCreateUserFromVerifiedInfo(users, info);

      expect(err).toBeUndefined();
      expect(user).toBeDefined();

      // Should have searched by providerUserId first
      expect(users.findOneByScan).toHaveBeenNthCalledWith(1, { providerUserId: 'apple-sub-456' });
      // Then by email
      expect(users.findOneByScan).toHaveBeenNthCalledWith(2, { email: 'test@example.com' });

      // Should have updated the user with the new providerUserId
      expect(users.update).toHaveBeenCalledWith('user-123', {}, expect.any(User));
      const updatedUser = users.update.mock.calls[0][2] as User;
      expect(updatedUser.getProviderUserId()).toBe('apple-sub-456');
      expect(updatedUser.getProvider()).toBe('apple');
    });

    it('should not update if providerUserId already matches', async () => {
      const users = createMockUsers();

      const existingUser = new User();
      existingUser.setId('user-123');
      existingUser.setEmail('test@example.com');
      existingUser.setProvider('apple');
      existingUser.setProviderUserId('apple-sub-456');

      // findOneByScan by providerUserId returns nothing (edge case: different email lookup first)
      // findOneByScan by email returns existing user
      users.findOneByScan.mockResolvedValueOnce(undefined).mockResolvedValueOnce(existingUser);

      const info: VerifiedUserInfo = {
        email: 'test@example.com',
        displayName: 'Test User',
        provider: 'apple',
        providerUserId: 'apple-sub-456',
      };

      const [user, err] = await getOrCreateUserFromVerifiedInfo(users, info);

      expect(err).toBeUndefined();
      expect(user).toBe(existingUser);

      // Should NOT have updated since providerUserId already matches
      expect(users.update).not.toHaveBeenCalled();
    });
  });

  describe('when no user exists', () => {
    it('should create new user with providerUserId', async () => {
      const users = createMockUsers();

      // No user found by providerUserId or email
      users.findOneByScan.mockResolvedValue(undefined);
      users.create.mockResolvedValue();

      const info: VerifiedUserInfo = {
        email: 'newuser@example.com',
        displayName: 'New User',
        provider: 'google',
        providerUserId: 'google-new-789',
        photoUrl: 'https://example.com/photo.jpg',
      };

      const [user, err] = await getOrCreateUserFromVerifiedInfo(users, info);

      expect(err).toBeUndefined();
      expect(user).toBeDefined();
      expect(user!.getEmail()).toBe('newuser@example.com');
      expect(user!.getDisplayName()).toBe('New User');
      expect(user!.getProvider()).toBe('google');
      expect(user!.getProviderUserId()).toBe('google-new-789');
      expect(user!.getPhotoUrl()).toBe('https://example.com/photo.jpg');

      expect(users.create).toHaveBeenCalled();
    });
  });

  describe('error handling', () => {
    it('should return error if no email provided', async () => {
      const users = createMockUsers();

      const info: VerifiedUserInfo = {
        email: '',
        displayName: 'Test User',
        provider: 'google',
        providerUserId: 'google-123',
      };

      const [user, err] = await getOrCreateUserFromVerifiedInfo(users, info);

      expect(user).toBeUndefined();
      expect(err).toBeDefined();
      expect(err!.message).toContain('expected user to have an email');
    });
  });
});
