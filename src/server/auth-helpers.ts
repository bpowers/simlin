// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request } from 'express';

/**
 * Information about the authenticated user extracted from the session.
 */
export interface AuthenticatedUser {
  email: string;
  userId: string;
}

/**
 * Interface for the deserialized user object set by passport on req.user.
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

  const email = (passportUser as Record<string, unknown>).email;
  if (typeof email !== 'string') {
    return undefined;
  }

  if (!isUserRecord(req.user)) {
    return undefined;
  }

  const userId = req.user.getId();
  if (typeof userId !== 'string') {
    return undefined;
  }

  return { email, userId };
}

/**
 * Check if the authenticated user owns a resource.
 */
export function isResourceOwner(authUser: AuthenticatedUser | undefined, ownerId: string): boolean {
  return authUser !== undefined && authUser.userId === ownerId;
}
