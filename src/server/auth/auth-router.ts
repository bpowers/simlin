// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Router } from 'express';
import * as admin from 'firebase-admin';
import { Firestore } from '@google-cloud/firestore';

import { FirebaseRestClient } from './firebase-rest-client';
import {
  createLoginHandler,
  createSignupHandler,
  createProvidersHandler,
  createResetPasswordHandler,
  createLogoutHandler,
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

  const handlerDeps = {
    firebaseRestClient: deps.firebaseRestClient,
    firebaseAdmin: deps.firebaseAdmin,
    users: deps.users,
    baseUrl: deps.baseUrl,
  };

  router.post('/login', createLoginHandler(handlerDeps));
  router.post('/signup', createSignupHandler(handlerDeps));
  router.post('/providers', createProvidersHandler(handlerDeps));
  router.post('/reset-password', createResetPasswordHandler(handlerDeps));
  router.post('/logout', createLogoutHandler());

  if (deps.firestore && deps.googleConfig) {
    const stateStore = createFirestoreStateStore(deps.firestore);

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

  if (deps.firestore && deps.appleConfig) {
    const stateStore = createFirestoreStateStore(deps.firestore);

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

  return router;
}
