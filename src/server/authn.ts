// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import { Request, Response } from 'express';
import { Strategy as BaseStrategy } from 'passport-strategy';
import { Set } from 'immutable';
import passport from 'passport';
import { v4 as uuidV4 } from 'uuid';
import * as logger from 'winston';
import * as admin from 'firebase-admin';

import { Application } from './application';
import { Table } from './models/table';
import { User } from './schemas/user_pb';

// allowlist users for now
let AllowedUsers = Set<string>();

// eslint-disable-next-line @typescript-eslint/no-empty-interface
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

  authenticate(req: Request, options?: any): void {
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

async function getOrCreateUserFromProfile(
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
    return [undefined, exception];
  }

  let fbUser: admin.auth.UserRecord;
  try {
    fbUser = await firebaseAuthn.getUser(decodedToken.uid);
  } catch (exception) {
    return [undefined, exception];
  }

  if (fbUser.disabled) {
    return [undefined, new Error('account is disabled')];
  }

  if (!fbUser.email) {
    return [undefined, new Error('expected user to have an email')];
  }
  const email = fbUser.email;

  // TODO: should we verify the email?

  if (!AllowedUsers.has(email)) {
    return [undefined, new Error(`user not in allowlist`)];
  }

  const displayName = fbUser.displayName ?? email;
  const photoUrl = fbUser.photoURL;

  // since a document with the email already exists, just get the
  // document with it
  let user: User | undefined = await users.findOneByScan({ email });
  if (!user) {
    const created = new Timestamp();
    created.fromDate(new Date());

    user = new User();
    user.setId(`temp-${uuidV4()}`);
    user.setEmail(email);
    user.setDisplayName(displayName);
    user.setProvider('google');
    if (photoUrl) {
      user.setPhotoUrl(photoUrl);
    }
    user.setCreated(created);
    user.setCanCreateProjects(false);

    await users.create(user.getId(), user);
  }

  if (!user) {
    return [undefined, new Error(`unable to insert or find user ${email}`)];
  }

  return [user, undefined];
}

export const authn = (app: Application, firebaseAuthn: admin.auth.Auth): void => {
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment,@typescript-eslint/no-unused-vars
  // const config = app.get('authentication');

  const userAllowlistKey = 'userAllowlist';
  // eslint-disable-next-line @typescript-eslint/no-unsafe-call
  const userAllowlist = (app.get(userAllowlistKey) || '').split(',') as string[];
  if (userAllowlist === undefined || userAllowlist.length === 0) {
    throw new Error(`expected ${userAllowlistKey} in config`);
  }
  AllowedUsers = Set(userAllowlist);

  passport.use(
    new FirestoreAuthStrategy({}, async (firestoreIdToken: string, done: (error: any, user?: any) => void) => {
      const [user, err] = await getOrCreateUserFromProfile(app.db.user, firebaseAuthn, firestoreIdToken);
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

  // eslint-disable-next-line @typescript-eslint/no-misused-promises
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

  app.delete(
    '/session',
    // eslint-disable-next-line @typescript-eslint/no-misused-promises
    (req: Request, res: Response): void => {
      console.log(`TODO: unset cookie`);
    },
  );
};
