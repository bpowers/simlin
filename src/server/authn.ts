// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { randomUUID } from 'node:crypto';

import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import { NextFunction, Request, Response } from 'express';
import * as admin from 'firebase-admin';

import { Application } from './application';
import { handleSessionDelete } from './auth-helpers';
import * as logger from './logger';
import { sessionAuth, setSessionUser } from './session-auth';
import { Table } from './models/table';
import { User } from './schemas/user_pb';

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

  // TODO: should we verify the email?

  const displayName = fbUser.displayName ?? email;
  const photoUrl = fbUser.photoURL;

  // since a document with the email already exists, just get the
  // document with it
  let user: User | undefined;
  try {
    user = await users.findOneByScan({ email });
    if (!user) {
      const created = new Timestamp();
      created.fromDate(new Date());

      user = new User();
      user.setId(`temp-${randomUUID()}`);
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
          // it should work now
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

export const authn = (app: Application, firebaseAuthn: admin.auth.Auth): void => {
  app.use(sessionAuth(app.db.user));

  // login: exchange a Firebase idToken for an authenticated session
  // cookie. Failures are 500s, matching the status the previous
  // passport strategy's error() path produced.
  app.post('/session', (req: Request, res: Response, next: NextFunction): void => {
    const body: unknown = req.body;
    const idToken =
      typeof body === 'object' && body !== null ? (body as Record<string, unknown>).idToken : undefined;
    if (typeof idToken !== 'string' || idToken === '') {
      logger.error('no idToken in body');
      res.sendStatus(500);
      return;
    }

    getOrCreateUserFromProfile(app.db.user, firebaseAuthn, idToken)
      .then(([user, err]) => {
        if (err !== undefined || !user) {
          logger.error(err ?? 'no user from profile');
          res.sendStatus(500);
          return;
        }
        logger.info(`session login for user: ${user.getId()}`);
        setSessionUser(req, user.getId());
        req.user = user;
        res.sendStatus(200);
      })
      .catch((err: unknown) => {
        next(err);
      });
  });

  app.delete('/session', handleSessionDelete);
};
