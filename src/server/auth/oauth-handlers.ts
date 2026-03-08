// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response, RequestHandler } from 'express';
import * as admin from 'firebase-admin';
import * as logger from 'winston';

import { OAuthStateStore } from './oauth-state';
import { validateReturnUrl } from './url-validation';
import {
  exchangeGoogleCode,
  fetchGoogleUserInfo,
  exchangeAppleCode,
  verifyAppleIdToken,
  generateAppleClientSecret,
} from './oauth-token-exchange';
import { getOrCreateUserFromVerifiedInfo } from '../authn';
import { Table } from '../models/table';
import { User } from '../schemas/user_pb';

export interface OAuthConfig {
  clientId: string;
  clientSecret: string;
  authorizationUrl: string;
  tokenUrl: string;
  scopes: string[];
  callbackPath: string;
}

export interface AppleOAuthConfig extends OAuthConfig {
  teamId: string;
  keyId: string;
  privateKey: string;
}

export interface OAuthHandlerDeps {
  stateStore: OAuthStateStore;
  firebaseAdmin: admin.auth.Auth;
  users: Table<User>;
  baseUrl: string;
}

export interface GoogleOAuthHandlerDeps extends OAuthHandlerDeps {
  config: OAuthConfig;
}

export interface AppleOAuthHandlerDeps extends OAuthHandlerDeps {
  config: AppleOAuthConfig;
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

export function createGoogleOAuthInitiateHandler(deps: GoogleOAuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    try {
      const returnUrl = typeof req.query.returnUrl === 'string' ? req.query.returnUrl : undefined;
      const state = await deps.stateStore.create(returnUrl);

      const redirectUri = `${deps.baseUrl}${deps.config.callbackPath}`;
      const params = new URLSearchParams({
        client_id: deps.config.clientId,
        redirect_uri: redirectUri,
        response_type: 'code',
        scope: deps.config.scopes.join(' '),
        state,
        access_type: 'offline',
        prompt: 'select_account',
      });

      res.redirect(`${deps.config.authorizationUrl}?${params.toString()}`);
    } catch (err) {
      logger.error('Error initiating Google OAuth:', err);
      res.redirect('/?error=oauth_init_failed');
    }
  };
}

export function createGoogleOAuthCallbackHandler(deps: GoogleOAuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    const { code, state, error } = req.query;

    if (error) {
      logger.error('Google OAuth error:', error);
      res.redirect('/?error=oauth_denied');
      return;
    }

    if (typeof state !== 'string' || !state) {
      res.status(400).json({ error: 'Missing state parameter' });
      return;
    }

    if (typeof code !== 'string' || !code) {
      res.status(400).json({ error: 'Missing code parameter' });
      return;
    }

    try {
      const stateResult = await deps.stateStore.validate(state);
      if (!stateResult.valid) {
        res.status(400).json({ error: 'Invalid or expired state' });
        return;
      }

      const returnUrl = validateReturnUrl(stateResult.returnUrl, deps.baseUrl);

      await deps.stateStore.invalidate(state);

      const redirectUri = `${deps.baseUrl}${deps.config.callbackPath}`;
      const tokens = await exchangeGoogleCode(deps.config.clientId, deps.config.clientSecret, code, redirectUri);

      const userInfo = await fetchGoogleUserInfo(tokens.access_token);

      let fbUser: admin.auth.UserRecord | undefined;
      try {
        fbUser = await deps.firebaseAdmin.getUserByEmail(userInfo.email);
      } catch (err: unknown) {
        const adminErr = err as { code?: string };
        if (adminErr.code === 'auth/user-not-found') {
          fbUser = await deps.firebaseAdmin.createUser({
            email: userInfo.email,
            displayName: userInfo.name,
            photoURL: userInfo.picture,
            emailVerified: userInfo.email_verified,
          });
        } else {
          throw err;
        }
      }

      if (fbUser?.disabled) {
        res.redirect('/?error=account_disabled');
        return;
      }

      const [user, userErr] = await getOrCreateUserFromVerifiedInfo(deps.users, {
        email: userInfo.email,
        displayName: userInfo.name,
        photoUrl: userInfo.picture,
        provider: 'google',
        providerUserId: userInfo.sub,
      });

      if (userErr) {
        logger.error('Error creating user from Google info:', userErr);
        res.redirect('/?error=user_creation_failed');
        return;
      }

      await loginUser(req, user);

      res.redirect(returnUrl);
    } catch (err) {
      logger.error('Error in Google OAuth callback:', err);
      await deps.stateStore.invalidate(state).catch(() => {});
      res.redirect('/?error=oauth_callback_failed');
    }
  };
}

export function createAppleOAuthInitiateHandler(deps: AppleOAuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    try {
      const returnUrl = typeof req.query.returnUrl === 'string' ? req.query.returnUrl : undefined;
      const state = await deps.stateStore.create(returnUrl);

      const redirectUri = `${deps.baseUrl}${deps.config.callbackPath}`;
      const params = new URLSearchParams({
        client_id: deps.config.clientId,
        redirect_uri: redirectUri,
        response_type: 'code',
        scope: deps.config.scopes.join(' '),
        state,
        response_mode: 'form_post',
      });

      res.redirect(`${deps.config.authorizationUrl}?${params.toString()}`);
    } catch (err) {
      logger.error('Error initiating Apple OAuth:', err);
      res.redirect('/?error=oauth_init_failed');
    }
  };
}

export function createAppleOAuthCallbackHandler(deps: AppleOAuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    const { code, state, error, id_token: bodyIdToken, user: appleUserJson } = req.body;

    if (error) {
      logger.error('Apple OAuth error:', error);
      res.redirect('/?error=oauth_denied');
      return;
    }

    if (typeof state !== 'string' || !state) {
      res.status(400).json({ error: 'Missing state parameter' });
      return;
    }

    if (typeof code !== 'string' || !code) {
      res.status(400).json({ error: 'Missing code parameter' });
      return;
    }

    try {
      const stateResult = await deps.stateStore.validate(state);
      if (!stateResult.valid) {
        res.status(400).json({ error: 'Invalid or expired state' });
        return;
      }

      const returnUrl = validateReturnUrl(stateResult.returnUrl, deps.baseUrl);

      await deps.stateStore.invalidate(state);

      const clientSecret = generateAppleClientSecret(
        deps.config.teamId,
        deps.config.clientId,
        deps.config.keyId,
        deps.config.privateKey,
      );

      const redirectUri = `${deps.baseUrl}${deps.config.callbackPath}`;
      const tokens = await exchangeAppleCode(deps.config.clientId, clientSecret, code, redirectUri);

      const idToken = tokens.id_token || bodyIdToken;
      if (!idToken) {
        throw new Error('No ID token received from Apple');
      }

      const claims = await verifyAppleIdToken(idToken, { clientId: deps.config.clientId });

      let appleUserName: string | undefined;
      if (typeof appleUserJson === 'string') {
        try {
          const appleUser = JSON.parse(appleUserJson);
          if (appleUser.name) {
            const { firstName, lastName } = appleUser.name;
            appleUserName = [firstName, lastName].filter(Boolean).join(' ');
          }
        } catch {
          // Ignore JSON parse errors
        }
      }

      const displayName = appleUserName || claims.name || claims.email || 'Apple User';
      const email = claims.email;

      if (!email) {
        // Apple omits email on subsequent logins. Look up user by providerUserId.
        // Include provider to prevent cross-provider collisions.
        let existingUser = await deps.users.findOneByScan({ providerUserId: claims.sub, provider: 'apple' });
        if (existingUser) {
          // Check if Firebase account is disabled before logging in
          let isDisabled = false;
          try {
            const fbUser = await deps.firebaseAdmin.getUserByProviderUid('apple.com', claims.sub);
            isDisabled = fbUser?.disabled ?? false;
          } catch {
            // If provider lookup fails, fallback to email lookup
            if (existingUser.getEmail()) {
              try {
                const fbUser = await deps.firebaseAdmin.getUserByEmail(existingUser.getEmail());
                isDisabled = fbUser?.disabled ?? false;
              } catch {
                // If neither lookup works, proceed with login
              }
            }
          }

          if (isDisabled) {
            res.redirect('/?error=account_disabled');
            return;
          }

          await loginUser(req, existingUser);
          res.redirect(returnUrl);
          return;
        }

        // Fallback for users created before providerUserId migration: try to find via Firebase
        // provider link. This handles users who signed in with Apple before we started storing
        // the Apple sub as providerUserId.
        try {
          const fbUser = await deps.firebaseAdmin.getUserByProviderUid('apple.com', claims.sub);
          if (fbUser && !fbUser.disabled && fbUser.email) {
            // Found Firebase user with this Apple ID - look up local user by email
            existingUser = await deps.users.findOneByScan({ email: fbUser.email });
            if (existingUser) {
              // Update providerUserId so future logins work directly
              existingUser.setProviderUserId(claims.sub);
              existingUser.setProvider('apple');
              await deps.users.update(existingUser.getId(), {}, existingUser);

              await loginUser(req, existingUser);
              res.redirect(returnUrl);
              return;
            }
          }
        } catch (err) {
          // getUserByProviderUid throws if user not found - that's expected
          logger.debug('No Firebase user found with Apple provider:', err);
        }

        // No email and no existing user - we can't create a new account
        logger.error('Apple user has no email and could not be found by providerUserId');
        res.redirect('/?error=apple_no_email');
        return;
      }

      let fbUser: admin.auth.UserRecord | undefined;
      try {
        fbUser = await deps.firebaseAdmin.getUserByEmail(email);
      } catch (err: unknown) {
        const adminErr = err as { code?: string };
        if (adminErr.code === 'auth/user-not-found') {
          fbUser = await deps.firebaseAdmin.createUser({
            email,
            displayName,
            emailVerified: claims.email_verified ?? false,
          });
        } else {
          throw err;
        }
      }

      if (fbUser?.disabled) {
        res.redirect('/?error=account_disabled');
        return;
      }

      const [user, userErr] = await getOrCreateUserFromVerifiedInfo(deps.users, {
        email,
        displayName,
        provider: 'apple',
        providerUserId: claims.sub,
      });

      if (userErr) {
        logger.error('Error creating user from Apple info:', userErr);
        res.redirect('/?error=user_creation_failed');
        return;
      }

      await loginUser(req, user);

      res.redirect(returnUrl);
    } catch (err) {
      logger.error('Error in Apple OAuth callback:', err);
      await deps.stateStore.invalidate(state).catch(() => {});
      res.redirect('/?error=oauth_callback_failed');
    }
  };
}
