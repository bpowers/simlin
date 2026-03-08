// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response, RequestHandler } from 'express';
import * as admin from 'firebase-admin';
import * as logger from 'winston';

import { FirebaseRestClient, FirebaseAuthError } from './firebase-rest-client';
import { getOrCreateUserFromIdToken } from '../authn';
import { Table } from '../models/table';
import { User } from '../schemas/user_pb';

export interface AuthHandlerDeps {
  firebaseRestClient: FirebaseRestClient;
  firebaseAdmin: admin.auth.Auth;
  users: Table<User>;
  baseUrl: string;
}

function getHttpStatusForError(err: FirebaseAuthError): number {
  switch (err.code) {
    case 'INVALID_PASSWORD':
    case 'EMAIL_NOT_FOUND':
      return 401;
    case 'USER_DISABLED':
      return 403;
    case 'EMAIL_EXISTS':
      return 409;
    case 'WEAK_PASSWORD':
    case 'INVALID_EMAIL':
      return 400;
    case 'TOO_MANY_ATTEMPTS_TRY_LATER':
      return 429;
    default:
      return 500;
  }
}

function loginUser(req: Request, user: User): Promise<void> {
  return new Promise((resolve, reject) => {
    req.login(user, (err) => {
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    });
  });
}

export function createLoginHandler(deps: AuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    const email = typeof req.body?.email === 'string' ? req.body.email.trim() : '';
    const password = typeof req.body?.password === 'string' ? req.body.password : '';

    if (!email) {
      res.status(400).json({ error: 'Email is required' });
      return;
    }

    if (!password) {
      res.status(400).json({ error: 'Password is required' });
      return;
    }

    try {
      const signInResult = await deps.firebaseRestClient.signInWithPassword(email, password);

      const [user, err] = await getOrCreateUserFromIdToken(deps.users, deps.firebaseAdmin, signInResult.idToken);

      if (err) {
        logger.error('Error getting or creating user:', err);
        if (err.message === 'account is disabled') {
          res.status(403).json({ error: 'This account has been disabled' });
        } else {
          res.status(500).json({ error: 'An unexpected error occurred' });
        }
        return;
      }

      await loginUser(req, user);

      res.status(200).json({
        success: true,
        user: {
          id: user.getId(),
          email: user.getEmail(),
          displayName: user.getDisplayName(),
        },
      });
    } catch (err) {
      if (err instanceof FirebaseAuthError) {
        const status = getHttpStatusForError(err);
        res.status(status).json({ error: err.message });
      } else {
        logger.error('Unexpected login error:', err);
        res.status(500).json({ error: 'An unexpected error occurred' });
      }
    }
  };
}

export function createSignupHandler(deps: AuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    const email = typeof req.body?.email === 'string' ? req.body.email.trim() : '';
    const password = typeof req.body?.password === 'string' ? req.body.password : '';
    const displayName = typeof req.body?.displayName === 'string' ? req.body.displayName.trim() : '';

    if (!email) {
      res.status(400).json({ error: 'Email is required' });
      return;
    }

    if (!password) {
      res.status(400).json({ error: 'Password is required' });
      return;
    }

    if (!displayName) {
      res.status(400).json({ error: 'Display name is required' });
      return;
    }

    try {
      const signUpResult = await deps.firebaseRestClient.signUp(email, password, displayName);

      const decodedToken = await deps.firebaseAdmin.verifyIdToken(signUpResult.idToken);
      await deps.firebaseAdmin.updateUser(decodedToken.uid, { displayName });

      const [user, err] = await getOrCreateUserFromIdToken(deps.users, deps.firebaseAdmin, signUpResult.idToken);

      if (err) {
        logger.error('Error creating user:', err);
        res.status(500).json({ error: 'An unexpected error occurred' });
        return;
      }

      await loginUser(req, user);

      res.status(201).json({
        success: true,
        user: {
          id: user.getId(),
          email: user.getEmail(),
          displayName: user.getDisplayName(),
        },
      });
    } catch (err) {
      if (err instanceof FirebaseAuthError) {
        const status = getHttpStatusForError(err);
        res.status(status).json({ error: err.message });
      } else {
        logger.error('Unexpected signup error:', err);
        res.status(500).json({ error: 'An unexpected error occurred' });
      }
    }
  };
}

export function createProvidersHandler(deps: AuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    const email = typeof req.body?.email === 'string' ? req.body.email.trim() : '';

    if (!email) {
      res.status(400).json({ error: 'Email is required' });
      return;
    }

    try {
      const continueUri = `${deps.baseUrl}/auth/callback`;
      const result = await deps.firebaseRestClient.fetchProviders(email, continueUri);

      res.status(200).json({
        registered: result.registered,
        providers: result.providers,
      });
    } catch (err) {
      logger.error('Error fetching providers:', err);
      res.status(500).json({ error: 'An unexpected error occurred' });
    }
  };
}

export function createResetPasswordHandler(deps: AuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    const email = typeof req.body?.email === 'string' ? req.body.email.trim() : '';

    if (!email) {
      res.status(400).json({ error: 'Email is required' });
      return;
    }

    try {
      await deps.firebaseRestClient.sendPasswordResetEmail(email);
      res.status(200).json({ success: true });
    } catch (err) {
      logger.error('Error sending password reset email:', err);
      res.status(200).json({ success: true });
    }
  };
}

export function createLogoutHandler(): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    return new Promise((resolve) => {
      req.logout((err) => {
        if (err) {
          logger.error('Error during logout:', err);
        }
        Object.keys(req.session as Record<string, unknown>).forEach((key) => {
          delete (req.session as Record<string, unknown>)[key];
        });
        res.sendStatus(200);
        resolve();
      });
    });
  };
}
