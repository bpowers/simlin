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

// A /session failure split into what the client may see and what the
// server log should record. The login screen renders `message` verbatim
// (see App.tsx maybeLogin), so it must be a concise, user-appropriate
// sentence and never raw Firebase or DB exception text -- that detail
// belongs in `logDetail`.
interface SessionFailure {
  status: number;
  message: string;
  logDetail: string;
}

const GENERIC_LOGIN_FAILURE = "We couldn't finish signing you in. Please try again.";
const UNVERIFIED_SIGN_IN = "We couldn't verify your sign-in. Please try signing in again.";
// Only reachable from a hand-rolled request (the real client always sends
// idToken), but the envelope rule still applies: every message may be
// rendered verbatim on the login screen.
const MISSING_CREDENTIALS = 'Your sign-in request was missing credentials. Please try signing in again.';

function describeError(err: unknown): string {
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}

// firebase-admin errors carry a string `code` (e.g. 'auth/user-not-found')
// on top of the Error shape; read it defensively since any throw can land
// here.
function firebaseErrorCode(err: unknown): string | undefined {
  if (typeof err === 'object' && err !== null) {
    const code = (err as Record<string, unknown>).code;
    if (typeof code === 'string') {
      return code;
    }
  }
  return undefined;
}

async function getOrCreateUserFromProfile(
  users: Table<User>,
  firebaseAuthn: admin.auth.Auth,
  firebaseIdToken: string,
): Promise<[User, undefined] | [undefined, SessionFailure]> {
  if (!firebaseIdToken) {
    // The route validates this before calling us; kept as defense in depth.
    return [undefined, { status: 400, message: MISSING_CREDENTIALS, logDetail: 'no idToken in body' }];
  }

  let decodedToken: admin.auth.DecodedIdToken;
  try {
    decodedToken = await firebaseAuthn.verifyIdToken(firebaseIdToken);
  } catch (exception) {
    // An expired, malformed, or revoked token: re-authenticating with
    // Firebase is the remedy, which is what 401 signals.
    return [
      undefined,
      { status: 401, message: UNVERIFIED_SIGN_IN, logDetail: `verifyIdToken: ${describeError(exception)}` },
    ];
  }

  let fbUser: admin.auth.UserRecord;
  try {
    fbUser = await firebaseAuthn.getUser(decodedToken.uid);
  } catch (exception) {
    // Id tokens outlive account deletion by up to an hour, so a verified
    // token whose account is gone is still an authentication failure
    // (401); any other lookup failure (e.g. Firebase unreachable) is on
    // our side (500).
    const status = firebaseErrorCode(exception) === 'auth/user-not-found' ? 401 : 500;
    return [
      undefined,
      {
        status,
        message: status === 401 ? UNVERIFIED_SIGN_IN : GENERIC_LOGIN_FAILURE,
        logDetail: `getUser(${decodedToken.uid}): ${describeError(exception)}`,
      },
    ];
  }

  if (fbUser.disabled) {
    // 403 rather than 401: the credential is valid, so prompting a
    // re-authentication would not help.
    return [
      undefined,
      {
        status: 403,
        message: 'This account has been disabled.',
        logDetail: `account ${decodedToken.uid} is disabled`,
      },
    ];
  }

  if (!fbUser.email) {
    // Simlin keys user records by email; an authenticated account without
    // one cannot be provisioned, and retrying will not change that (403).
    return [
      undefined,
      {
        status: 403,
        message: 'Your account has no email address, which Simlin requires.',
        logDetail: `account ${decodedToken.uid} has no email`,
      },
    ];
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
    if (err instanceof Error && err.message.includes('expected single result document')) {
      // we have some eventual consistency problem where sometimes we don't
      // delete the temp user when completing the sign-up flow.  Resolve that
      // consistency issue manually for now.
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
    } else {
      // Any other failure -- Error or not; drivers can reject with plain
      // strings or objects -- is swallowed here (user stays undefined and
      // the generic 500 below is returned), so this is the only place its
      // detail can be captured.
      logger.error(`user lookup/create for ${email}: ${describeError(err)}`);
    }
  }

  if (!user) {
    return [
      undefined,
      { status: 500, message: GENERIC_LOGIN_FAILURE, logDetail: `unable to insert or find user ${email}` },
    ];
  }

  return [user, undefined];
}

export const authn = (app: Application, firebaseAuthn: admin.auth.Auth): void => {
  app.use(sessionAuth(app.db.user));

  // login: exchange a Firebase idToken for an authenticated session
  // cookie. Every failure branch answers with the same JSON {error}
  // envelope the rest of the API uses: the client renders that message on
  // the login screen, and the plain-text body of the previous blanket
  // res.sendStatus(500) made the client's response.json() throw
  // mid-error-handling, masking the real failure (#927).
  app.post('/session', (req: Request, res: Response, next: NextFunction): void => {
    const body: unknown = req.body;
    const idToken = typeof body === 'object' && body !== null ? (body as Record<string, unknown>).idToken : undefined;
    if (typeof idToken !== 'string' || idToken === '') {
      logger.error('POST /session: no idToken in body');
      res.status(400).json({ error: MISSING_CREDENTIALS });
      return;
    }

    getOrCreateUserFromProfile(app.db.user, firebaseAuthn, idToken)
      .then(([user, failure]) => {
        if (failure !== undefined || user === undefined) {
          const f = failure ?? { status: 500, message: GENERIC_LOGIN_FAILURE, logDetail: 'no user from profile' };
          logger.error(`POST /session failed (${f.status}): ${f.logDetail}`);
          res.status(f.status).json({ error: f.message });
          return;
        }
        logger.info(`session login for user: ${user.getId()}`);
        setSessionUser(req, user.getId());
        req.user = user;
        res.sendStatus(200);
      })
      .catch((err: unknown) => {
        // Defense in depth: even an unexpected throw must keep the JSON
        // error contract -- Express's default handler would answer with an
        // HTML page the client can't parse.
        logger.error(`POST /session: ${err instanceof Error ? (err.stack ?? err.message) : String(err)}`);
        if (res.headersSent) {
          next(err);
          return;
        }
        res.status(500).json({ error: GENERIC_LOGIN_FAILURE });
      });
  });

  app.delete('/session', handleSessionDelete);
};
