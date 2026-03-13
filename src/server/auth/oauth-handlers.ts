// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { CookieOptions, Request, Response, RequestHandler } from 'express';
import { randomBytes } from 'crypto';
import * as admin from 'firebase-admin';
import * as logger from 'winston';

import { loginUser } from './auth-utils';
import { DEFAULT_TTL_MS, OAuthStateStore } from './oauth-state';
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

const OAUTH_BINDING_COOKIE_OPTIONS: CookieOptions = {
  httpOnly: true,
  maxAge: DEFAULT_TTL_MS,
  path: '/auth',
  sameSite: 'none',
  secure: true,
};

function getOAuthBindingCookieName(state: string): string {
  return `oauth_state_${state}`;
}

function getOAuthBindingSecret(req: Request, state: string): string | undefined {
  const cookieName = getOAuthBindingCookieName(state);
  const bindingSecret = req.cookies?.[cookieName];
  return typeof bindingSecret === 'string' && bindingSecret !== '' ? bindingSecret : undefined;
}

function setOAuthBindingCookie(res: Response, state: string, bindingSecret: string): void {
  res.cookie(getOAuthBindingCookieName(state), bindingSecret, OAUTH_BINDING_COOKIE_OPTIONS);
}

function clearOAuthBindingCookie(res: Response, state: string): void {
  res.clearCookie(getOAuthBindingCookieName(state), OAUTH_BINDING_COOKIE_OPTIONS);
}

function isAppleProviderLinked(fbUser: admin.auth.UserRecord, appleSub: string): boolean {
  return fbUser.providerData?.some((provider) => provider.providerId === 'apple.com' && provider.uid === appleSub) ?? false;
}

async function ensureAppleProviderLinked(
  firebaseAdmin: admin.auth.Auth,
  fbUser: admin.auth.UserRecord,
  appleSub: string,
  email: string,
  displayName: string,
): Promise<void> {
  if (isAppleProviderLinked(fbUser, appleSub)) {
    return;
  }

  await firebaseAdmin.updateUser(fbUser.uid, {
    providerToLink: {
      providerId: 'apple.com',
      uid: appleSub,
      email,
      displayName,
    },
  });
}

export function createGoogleOAuthInitiateHandler(deps: GoogleOAuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    try {
      const returnUrl = typeof req.query.returnUrl === 'string' ? req.query.returnUrl : undefined;
      const bindingSecret = randomBytes(32).toString('hex');
      const state = await deps.stateStore.create({ returnUrl, bindingSecret });
      setOAuthBindingCookie(res, state, bindingSecret);

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
      if (typeof state === 'string' && state) {
        clearOAuthBindingCookie(res, state);
      }
      res.redirect('/?error=oauth_denied');
      return;
    }

    if (typeof state !== 'string' || !state) {
      res.status(400).json({ error: 'Missing state parameter' });
      return;
    }

    if (typeof code !== 'string' || !code) {
      clearOAuthBindingCookie(res, state);
      res.status(400).json({ error: 'Missing code parameter' });
      return;
    }

    try {
      const bindingSecret = getOAuthBindingSecret(req, state);
      const stateResult = await deps.stateStore.validate({ state, bindingSecret });
      if (!stateResult.valid) {
        clearOAuthBindingCookie(res, state);
        res.status(400).json({ error: 'Invalid or expired state' });
        return;
      }

      const returnUrl = validateReturnUrl(stateResult.returnUrl, deps.baseUrl);

      await deps.stateStore.invalidate(state);
      clearOAuthBindingCookie(res, state);

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
      clearOAuthBindingCookie(res, state);
      // Defense-in-depth: state was already invalidated above, so this is
      // a no-op in the normal case.  Catches the edge case where the error
      // occurred before the earlier invalidate call completed.
      await deps.stateStore.invalidate(state).catch(() => {});
      res.redirect('/?error=oauth_callback_failed');
    }
  };
}

type AppleNoEmailResult = { status: 'found'; user: User } | { status: 'disabled' } | { status: 'not_found' };

// Resolves an Apple user when the ID token omits the email (subsequent logins).
// Tries providerUserId lookup first, then falls back to Firebase provider link
// for users created before we started storing providerUserId.
async function resolveAppleUserWithoutEmail(
  users: Table<User>,
  firebaseAdmin: admin.auth.Auth,
  appleSub: string,
): Promise<AppleNoEmailResult> {
  // Primary lookup: find by providerUserId with provider scoping to prevent
  // cross-provider collisions
  const existingUser = await users.findOneByScan({ providerUserId: appleSub, provider: 'apple' });
  if (existingUser) {
    // Fail closed: if we can't verify the account isn't disabled (e.g.
    // Firebase is temporarily unreachable), reject the login rather than
    // risk letting a disabled account through.
    let statusVerified = false;
    let isDisabled = false;
    try {
      const fbUser = await firebaseAdmin.getUserByProviderUid('apple.com', appleSub);
      isDisabled = fbUser?.disabled ?? false;
      statusVerified = true;
    } catch {
      // If provider lookup fails, fallback to email lookup
      if (existingUser.getEmail()) {
        try {
          const fbUser = await firebaseAdmin.getUserByEmail(existingUser.getEmail());
          isDisabled = fbUser?.disabled ?? false;
          statusVerified = true;
        } catch {
          // Neither lookup succeeded
        }
      }
    }

    if (!statusVerified) {
      logger.error(`Cannot verify disabled status for Apple user ${appleSub}, rejecting login`);
      return { status: 'disabled' };
    }
    if (isDisabled) {
      return { status: 'disabled' };
    }
    return { status: 'found', user: existingUser };
  }

  // Fallback for users created before providerUserId migration: try to find via
  // Firebase provider link. This handles users who signed in with Apple before we
  // started storing the Apple sub as providerUserId.
  try {
    const fbUser = await firebaseAdmin.getUserByProviderUid('apple.com', appleSub);
    if (fbUser && !fbUser.disabled && fbUser.email) {
      const userByEmail = await users.findOneByScan({ email: fbUser.email });
      if (userByEmail) {
        // Only rewrite the local provider slot when it is empty or still
        // password-based. Existing OAuth slots stay intact and Firebase's
        // provider link becomes the source of truth for later Apple logins.
        if (!userByEmail.getProviderUserId() || userByEmail.getProvider() === 'password') {
          userByEmail.setProviderUserId(appleSub);
          userByEmail.setProvider('apple');
          await users.update(userByEmail.getId(), {}, userByEmail);
        }
        return { status: 'found', user: userByEmail };
      }
    }
  } catch (err) {
    // getUserByProviderUid throws if user not found - expected
    logger.debug('No Firebase user found with Apple provider:', err);
  }

  return { status: 'not_found' };
}

export function createAppleOAuthInitiateHandler(deps: AppleOAuthHandlerDeps): RequestHandler {
  return async (req: Request, res: Response): Promise<void> => {
    try {
      const returnUrl = typeof req.query.returnUrl === 'string' ? req.query.returnUrl : undefined;
      const bindingSecret = randomBytes(32).toString('hex');
      const state = await deps.stateStore.create({ returnUrl, bindingSecret });
      setOAuthBindingCookie(res, state, bindingSecret);

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
      if (typeof state === 'string' && state) {
        clearOAuthBindingCookie(res, state);
      }
      res.redirect('/?error=oauth_denied');
      return;
    }

    if (typeof state !== 'string' || !state) {
      res.status(400).json({ error: 'Missing state parameter' });
      return;
    }

    if (typeof code !== 'string' || !code) {
      clearOAuthBindingCookie(res, state);
      res.status(400).json({ error: 'Missing code parameter' });
      return;
    }

    try {
      const bindingSecret = getOAuthBindingSecret(req, state);
      const stateResult = await deps.stateStore.validate({ state, bindingSecret });
      if (!stateResult.valid) {
        clearOAuthBindingCookie(res, state);
        res.status(400).json({ error: 'Invalid or expired state' });
        return;
      }

      const returnUrl = validateReturnUrl(stateResult.returnUrl, deps.baseUrl);

      await deps.stateStore.invalidate(state);
      clearOAuthBindingCookie(res, state);

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
        // Apple omits email on subsequent logins; resolve user by providerUserId
        // or Firebase provider link fallback
        const result = await resolveAppleUserWithoutEmail(deps.users, deps.firebaseAdmin, claims.sub);
        if (result.status === 'disabled') {
          res.redirect('/?error=account_disabled');
          return;
        }
        if (result.status === 'found') {
          await loginUser(req, result.user);
          res.redirect(returnUrl);
          return;
        }
        // No email and no existing user - we can't create a new account
        logger.error('Apple user has no email and could not be found by providerUserId');
        res.redirect('/?error=apple_no_email');
        return;
      }

      let fbUser: admin.auth.UserRecord | undefined;
      let firebaseUserExisted = false;
      try {
        fbUser = await deps.firebaseAdmin.getUserByEmail(email);
        firebaseUserExisted = true;
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

      if (fbUser && firebaseUserExisted) {
        await ensureAppleProviderLinked(deps.firebaseAdmin, fbUser, claims.sub, email, displayName);
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
      clearOAuthBindingCookie(res, state);
      // Defense-in-depth: see comment in Google callback above
      await deps.stateStore.invalidate(state).catch(() => {});
      res.redirect('/?error=oauth_callback_failed');
    }
  };
}
