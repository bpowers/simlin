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
 * Check if the request has a valid authenticated session.
 * Returns the authenticated user info if present, undefined otherwise.
 *
 * This safely checks all levels of the session object to avoid TypeError
 * when accessing properties on undefined/null objects.
 */
export function getAuthenticatedUser(req: Request): AuthenticatedUser | undefined {
  // Check session exists
  if (!req.session) {
    return undefined;
  }

  // Check passport exists in session
  const passport = (req.session as Record<string, unknown>).passport;
  if (!passport || typeof passport !== 'object') {
    return undefined;
  }

  // Check user exists in passport
  const passportUser = (passport as Record<string, unknown>).user;
  if (!passportUser || typeof passportUser !== 'object') {
    return undefined;
  }

  // Extract email and user ID
  const email = (passportUser as Record<string, unknown>).email;
  if (typeof email !== 'string') {
    return undefined;
  }

  // Get user from req.user (set by passport deserialize)
  const user = req.user as Record<string, unknown> | undefined;
  if (!user) {
    return undefined;
  }

  // Get user ID from the deserialized user object
  const getId = user.getId;
  if (typeof getId !== 'function') {
    return undefined;
  }

  const userId = getId.call(user);
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
