// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Router } from 'express';
import * as admin from 'firebase-admin';
import { Firestore } from '@google-cloud/firestore';

import { FirebaseRestClient } from './firebase-rest-client';
import {
  createLoginHandler,
  createSignupHandler,
  createOAuthProvidersHandler,
  createProvidersHandler,
  createResetPasswordHandler,
  createLogoutHandler,
  OAuthProviderId,
} from './auth-handlers';
import {
  createGoogleOAuthInitiateHandler,
  createGoogleOAuthCallbackHandler,
  createAppleOAuthInitiateHandler,
  createAppleOAuthCallbackHandler,
  OAuthConfig,
  AppleOAuthConfig,
} from './oauth-handlers';
import { createFirestoreStateStore } from './oauth-state';
import { Table } from '../models/table';
import { User } from '../schemas/user_pb';

export interface AuthRouterDeps {
  firebaseRestClient: FirebaseRestClient;
  firebaseAdmin: admin.auth.Auth;
  users: Table<User>;
  baseUrl: string;
  firestore?: Firestore;
  googleConfig?: OAuthConfig;
  appleConfig?: AppleOAuthConfig;
}

export function createAuthRouter(deps: AuthRouterDeps): Router {
  const router = Router();
  const enabledOAuthProviders: OAuthProviderId[] = [];
  if (deps.googleConfig) {
    enabledOAuthProviders.push('google.com');
  }
  if (deps.appleConfig) {
    enabledOAuthProviders.push('apple.com');
  }

  const handlerDeps = {
    firebaseRestClient: deps.firebaseRestClient,
    firebaseAdmin: deps.firebaseAdmin,
    users: deps.users,
    baseUrl: deps.baseUrl,
    enabledOAuthProviders,
  };

  router.post('/login', createLoginHandler(handlerDeps));
  router.post('/signup', createSignupHandler(handlerDeps));
  router.get('/providers', createOAuthProvidersHandler(handlerDeps));
  router.post('/providers', createProvidersHandler(handlerDeps));
  router.post('/reset-password', createResetPasswordHandler(handlerDeps));
  router.post('/logout', createLogoutHandler());

  if (deps.firestore && (deps.googleConfig || deps.appleConfig)) {
    // Single shared state store for all OAuth providers -- they all use the
    // same Firestore collection and state tokens are provider-agnostic.
    const stateStore = createFirestoreStateStore(deps.firestore);

    if (deps.googleConfig) {
      const googleDeps = {
        config: deps.googleConfig,
        stateStore,
        firebaseAdmin: deps.firebaseAdmin,
        users: deps.users,
        baseUrl: deps.baseUrl,
      };

      router.get('/google', createGoogleOAuthInitiateHandler(googleDeps));
      router.get('/google/callback', createGoogleOAuthCallbackHandler(googleDeps));
    }

    if (deps.appleConfig) {
      const appleDeps = {
        config: deps.appleConfig,
        stateStore,
        firebaseAdmin: deps.firebaseAdmin,
        users: deps.users,
        baseUrl: deps.baseUrl,
      };

      router.get('/apple', createAppleOAuthInitiateHandler(appleDeps));
      router.post('/apple/callback', createAppleOAuthCallbackHandler(appleDeps));
    }
  }

  return router;
}
