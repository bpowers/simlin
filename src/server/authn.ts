// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import passport from 'passport';
import { v4 as uuidV4 } from 'uuid';
import * as logger from 'winston';
import * as admin from 'firebase-admin';

import { Application } from './application';
import { Table } from './models/table';
import { User } from './schemas/user_pb';

export type AuthProvider = 'google' | 'apple' | 'password';

export interface VerifiedUserInfo {
  email: string;
  displayName: string;
  photoUrl?: string;
  provider: AuthProvider;
  providerUserId: string;
}

interface SerializedSessionUser {
  id: string;
}

function isSerializedSessionUser(value: unknown): value is SerializedSessionUser {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Record<string, unknown>).id === 'string'
  );
}

function getProviderFromFirebaseUser(fbUser: admin.auth.UserRecord): AuthProvider {
  if (!fbUser.providerData || fbUser.providerData.length === 0) {
    return 'password';
  }
  const providerIds = fbUser.providerData.map((p) => p.providerId);
  if (providerIds.includes('google.com')) {
    return 'google';
  }
  if (providerIds.includes('apple.com')) {
    return 'apple';
  }
  return 'password';
}

// We have an eventual consistency problem where sometimes the temp user isn't
// deleted when completing the sign-up flow, leaving duplicate documents for
// the same email.  When findOneByScan fails with "expected single result
// document", clean up temp- users and retry the lookup.
async function recoverFromDuplicateUsers(users: Table<User>, email: string): Promise<User | undefined> {
  const userDocs = await users.findByScan({ email });
  if (!userDocs) {
    return undefined;
  }

  let fullUserFound = false;
  for (const user of userDocs) {
    if (!user.getId().startsWith('temp-')) {
      fullUserFound = true;
      break;
    }
  }

  if (fullUserFound) {
    for (const user of userDocs) {
      const userId = user.getId();
      if (userId.startsWith('temp-')) {
        logger.info(`fixing inconsistency with ${email} -- deleting '${userId}' in DB`);
        await users.deleteOne(userId);
      }
    }
  }

  return users.findOneByScan({ email });
}

export async function getOrCreateUserFromIdToken(
  users: Table<User>,
  firebaseAuthn: admin.auth.Auth,
  firebaseIdToken: string,
): Promise<[User, undefined] | [undefined, Error]> {
  if (!firebaseIdToken) {
    return [undefined, new Error('no idToken')];
  }

  let decodedToken: admin.auth.DecodedIdToken;
  try {
    decodedToken = await firebaseAuthn.verifyIdToken(firebaseIdToken);
  } catch (exception) {
    return [undefined, exception as Error];
  }

  let fbUser: admin.auth.UserRecord;
  try {
    fbUser = await firebaseAuthn.getUser(decodedToken.uid);
  } catch (exception) {
    return [undefined, exception as Error];
  }

  if (fbUser.disabled) {
    return [undefined, new Error('account is disabled')];
  }

  if (!fbUser.email) {
    return [undefined, new Error('expected user to have an email')];
  }
  const email = fbUser.email;

  const displayName = fbUser.displayName ?? email;
  const photoUrl = fbUser.photoURL;
  const provider = getProviderFromFirebaseUser(fbUser);

  let user: User | undefined;
  try {
    user = await users.findOneByScan({ email });
    if (!user) {
      const created = new Timestamp();
      created.fromDate(new Date());

      user = new User();
      user.setId(`temp-${uuidV4()}`);
      user.setEmail(email);
      user.setDisplayName(displayName);
      user.setProvider(provider);
      user.setProviderUserId(decodedToken.uid);
      if (photoUrl) {
        user.setPhotoUrl(photoUrl);
      }
      user.setCreated(created);
      user.setCanCreateProjects(false);

      await users.create(user.getId(), user);
    }
  } catch (err) {
    if (err instanceof Error && err.message.includes('expected single result document')) {
      user = await recoverFromDuplicateUsers(users, email);
    }
  }

  if (!user) {
    return [undefined, new Error(`unable to insert or find user ${email}`)];
  }

  return [user, undefined];
}

export async function getOrCreateUserFromVerifiedInfo(
  users: Table<User>,
  info: VerifiedUserInfo,
): Promise<[User, undefined] | [undefined, Error]> {
  if (!info.email) {
    return [undefined, new Error('expected user to have an email')];
  }

  let user: User | undefined;
  let matchedByEmail = false;
  try {
    if (info.providerUserId) {
      // Include provider in lookup to prevent cross-provider collisions
      user = await users.findOneByScan({ providerUserId: info.providerUserId, provider: info.provider });
    }
    if (!user && info.email) {
      user = await users.findOneByScan({ email: info.email });
      if (user) {
        matchedByEmail = true;
      }
    }
    if (user && matchedByEmail && info.providerUserId) {
      const existingProvider = user.getProvider();
      // Update if: user has no providerUserId, OR existing provider is 'password'
      // (password provider uses Firebase UID as providerUserId, not useful for lookups).
      // DON'T update if existing provider is an OAuth provider (google/apple) -
      // that would break re-login via the original OAuth provider since they
      // often omit email on subsequent logins.
      if (!user.getProviderUserId() || existingProvider === 'password') {
        user.setProviderUserId(info.providerUserId);
        user.setProvider(info.provider);
        await users.update(user.getId(), {}, user);
      }
    }
    if (!user) {
      const created = new Timestamp();
      created.fromDate(new Date());

      user = new User();
      user.setId(`temp-${uuidV4()}`);
      user.setEmail(info.email);
      user.setDisplayName(info.displayName);
      user.setProvider(info.provider);
      user.setProviderUserId(info.providerUserId);
      if (info.photoUrl) {
        user.setPhotoUrl(info.photoUrl);
      }
      user.setCreated(created);
      user.setCanCreateProjects(false);

      await users.create(user.getId(), user);
    }
  } catch (err) {
    if (err instanceof Error && err.message.includes('expected single result document')) {
      user = await recoverFromDuplicateUsers(users, info.email);
    }
  }

  if (!user) {
    return [undefined, new Error(`unable to insert or find user ${info.email}`)];
  }

  return [user, undefined];
}

export const authn = (app: Application): void => {
  passport.serializeUser((rawUser, done) => {
    if (!(rawUser instanceof User)) {
      done(new Error('serializeUser expected a User instance'));
      return;
    }
    const user = rawUser;
    console.log(`serialize user: ${user.getId()}`);
    const serializedUser: SerializedSessionUser = {
      id: user.getId(),
    };
    done(null, serializedUser);
  });

  passport.deserializeUser(async (user, done) => {
    if (!isSerializedSessionUser(user)) {
      done(new Error(`no or incorrectly serialized User: ${String(user)}`));
      return;
    }

    const userModel = await app.db.user.findOne(user.id);
    if (!userModel) {
      logger.info(`couldn't find user '${user.id}' in DB`);
      done(null, null);
      return;
    }
    done(null, userModel);
  });

  app.use(passport.initialize());
  app.use(passport.session());
};
