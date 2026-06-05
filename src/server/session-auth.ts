// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Cookie-session authentication helpers, replacing the passport
// dependency. Passport's role here was small: stash {user: {id}} in the
// session on login, look the user up from that id on every request, and
// clear it on logout. The session itself lives in the seshcookie-encrypted
// cookie, so the on-the-wire session shape is part of our compatibility
// surface: the nested key is still named `passport`
// (session.passport.user.id) because renaming it would invalidate every
// login minted before the dependency was removed.

import { NextFunction, Request, RequestHandler, Response } from 'express';

import * as logger from './logger';

declare global {
  // eslint-disable-next-line @typescript-eslint/no-namespace
  namespace Express {
    interface Request {
      /**
       * The deserialized user record for the authenticated session,
       * populated by sessionAuth(). (Previously declared by
       * @types/passport.)
       */
      user?: unknown;
    }
  }
}

/** The slice of the user table sessionAuth needs. */
export interface UserLookup<T> {
  findOne(id: string): Promise<T | undefined>;
}

/**
 * Read the authenticated user id out of the session, tolerating any
 * malformed shape (the cookie is client-supplied bytes; decryption
 * authenticates it, but defensiveness here is free).
 */
export function getSessionUserId(req: Request): string | undefined {
  const session: unknown = req.session;
  if (typeof session !== 'object' || session === null) {
    return undefined;
  }
  const passport = (session as Record<string, unknown>).passport;
  if (typeof passport !== 'object' || passport === null) {
    return undefined;
  }
  const user = (passport as Record<string, unknown>).user;
  if (typeof user !== 'object' || user === null) {
    return undefined;
  }
  const id = (user as Record<string, unknown>).id;
  return typeof id === 'string' ? id : undefined;
}

/** Mark the session as authenticated for `id` (login). */
export function setSessionUser(req: Request, id: string): void {
  (req.session as Record<string, unknown>).passport = { user: { id } };
}

/**
 * Middleware that deserializes the session's user id into req.user on
 * every request. A session naming a user that no longer exists is
 * treated as unauthenticated rather than an error, matching the
 * previous passport deserializeUser behavior.
 */
export function sessionAuth<T>(users: UserLookup<T>): RequestHandler {
  return (req: Request, _res: Response, next: NextFunction): void => {
    const id = getSessionUserId(req);
    if (id === undefined) {
      next();
      return;
    }
    users
      .findOne(id)
      .then((user) => {
        if (user) {
          req.user = user;
        } else {
          logger.info(`couldn't find user '${id}' in DB`);
        }
        next();
      })
      .catch((err: unknown) => {
        next(err);
      });
  };
}
