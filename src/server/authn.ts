// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import { Request, Response } from 'express';
import { Strategy as BaseStrategy } from 'passport-strategy';
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

interface StrategyOptions {}

interface VerifyFunction {
  (firestoreIdToken: string, done: (error: any, user?: any) => void): Promise<void>;
}

class FirestoreAuthStrategy extends BaseStrategy implements passport.Strategy {
  readonly name: 'firestore-auth';
  private readonly verify: VerifyFunction;

  constructor(options: StrategyOptions, verify: VerifyFunction) {
    super();
    this.name = 'firestore-auth';
    this.verify = verify;
  }

  authenticate(req: Request, _options?: any): void {
    if (!req.body || !req.body.idToken) {
      this.error(new Error('no idToken in body'));
      return;
    }

    const idToken = req.body.idToken as string;

    const verified = (error: any, user?: any): void => {
      if (error) {
        return this.error(error);
      }
      if (!user) {
        return this.fail(401);
      }
      this.success(user);
    };

    this.verify(idToken, verified)
      .then(() => {})
      .catch((err) => {
        this.error(err);
      });
  }
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
    if (err instanceof Error) {
      // we have some eventual consistency problem where sometimes we don't
      // delete the temp user when completing the sign-up flow.  Resolve that
      // consistency issue manually for now.
      if (err.message.includes('expected single result document')) {
        const userDocs = await users.findByScan({ email });
        if (userDocs) {
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
          user = await users.findOneByScan({ email });
        }
      }
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
      user = await users.findOneByScan({ providerUserId: info.providerUserId });
    }
    if (!user && info.email) {
      user = await users.findOneByScan({ email: info.email });
      if (user) {
        matchedByEmail = true;
      }
    }
    if (user && matchedByEmail && info.providerUserId && user.getProviderUserId() !== info.providerUserId) {
      // User was found by email but has different (or no) providerUserId.
      // Update to use the new provider info so future logins without email work.
      user.setProviderUserId(info.providerUserId);
      user.setProvider(info.provider);
      await users.update(user.getId(), {}, user);
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
    if (err instanceof Error) {
      if (err.message.includes('expected single result document')) {
        const userDocs = await users.findByScan({ email: info.email });
        if (userDocs) {
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
                logger.info(`fixing inconsistency with ${info.email} -- deleting '${userId}' in DB`);
                await users.deleteOne(userId);
              }
            }
          }
          user = await users.findOneByScan({ email: info.email });
        }
      }
    }
  }

  if (!user) {
    return [undefined, new Error(`unable to insert or find user ${info.email}`)];
  }

  return [user, undefined];
}

export const authn = (app: Application, firebaseAuthn: admin.auth.Auth): void => {
  // const config = app.get('authentication');

  // DEPRECATED: Use /auth/login instead. This endpoint exists for backward
  // compatibility with existing mobile apps and will be removed in a future release.
  passport.use(
    new FirestoreAuthStrategy({}, async (firestoreIdToken: string, done: (error: any, user?: any) => void) => {
      const [user, err] = await getOrCreateUserFromIdToken(app.db.user, firebaseAuthn, firestoreIdToken);
      if (err !== undefined) {
        logger.error(err);
        done(err);
      } else if (user) {
        done(undefined, user);
      } else {
        throw new Error('unreachable');
      }
    }),
  );

  passport.serializeUser((rawUser: any, done: (error: any, user?: any) => void) => {
    const user = rawUser as User;
    console.log(`serialize user: ${user.getId()}`);
    const serializedUser: any = {
      id: user.getId(),
    };
    done(undefined, serializedUser);
  });

  passport.deserializeUser(async (user: any, done: (error: any, user?: any) => void) => {
    if (!user || !user.id) {
      done(new Error(`no or incorrectly serialized User: ${user}`));
      return;
    }

    const userModel = await app.db.user.findOne(user.id);
    if (!userModel) {
      logger.info(`couldn't find user '${user.id}' in DB`);
      done(undefined, null);
      return;
    }
    done(undefined, userModel);
  });

  app.use(passport.initialize());
  app.use(passport.session());

  app.post('/session', passport.authenticate('firestore-auth', {}), (req: Request, res: Response): void => {
    res.sendStatus(200);
  });

  app.delete('/session', (_req: Request, _res: Response): void => {
    console.log(`TODO: unset cookie`);
  });
};
