// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response } from 'express';

/**
 * Information about the authenticated user extracted from the session.
 */
export interface AuthenticatedUser {
  userId: string;
}

/**
 * Interface for the deserialized user object set on req.user by the
 * sessionAuth middleware.
 */
interface UserRecord {
  getId(): string;
}

function isUserRecord(obj: unknown): obj is UserRecord {
  return obj !== null && typeof obj === 'object' && typeof (obj as Record<string, unknown>).getId === 'function';
}

/**
 * Check if the request has a valid authenticated session.
 * Returns the authenticated user info if present, undefined otherwise.
 *
 * This safely checks all levels of the session object to avoid TypeError
 * when accessing properties on undefined/null objects.
 *
 * Login stores { id: userId } under session.passport.user (see
 * session-auth.ts for why the key keeps its historic name), so we check
 * for that field to confirm the session is authenticated. The full user
 * object (with getId(), getEmail(), etc.) is on req.user, populated by
 * the sessionAuth middleware.
 */
export function getAuthenticatedUser(req: Request): AuthenticatedUser | undefined {
  if (!req.session) {
    return undefined;
  }

  const passport = (req.session as Record<string, unknown>).passport;
  if (!passport || typeof passport !== 'object') {
    return undefined;
  }

  const passportUser = (passport as Record<string, unknown>).user;
  if (!passportUser || typeof passportUser !== 'object') {
    return undefined;
  }

  const sessionId = (passportUser as Record<string, unknown>).id;
  if (typeof sessionId !== 'string') {
    return undefined;
  }

  if (!isUserRecord(req.user)) {
    return undefined;
  }

  const userId = req.user.getId();
  if (typeof userId !== 'string') {
    return undefined;
  }

  return { userId };
}

/**
 * Check if the authenticated user owns a resource.
 */
export function isResourceOwner(authUser: AuthenticatedUser | undefined, ownerId: string): boolean {
  return authUser !== undefined && authUser.userId === ownerId;
}

/**
 * DELETE /session handler: terminates the login session.
 *
 * Sessions live in a seshcookie-encrypted cookie, so there is no
 * server-side store to purge: replacing req.session with a fresh object
 * drops the authenticated state, and seshcookie's response hook rewrites
 * the Set-Cookie with the now-unauthenticated session. The handler must
 * always respond -- an earlier version never wrote a response, so a
 * client awaiting the fetch hung indefinitely.
 */
export function handleSessionDelete(req: Request, res: Response): void {
  req.session = {};
  res.sendStatus(200);
}
