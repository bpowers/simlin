// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Firestore } from '@google-cloud/firestore';
import { randomBytes } from 'crypto';

export interface OAuthState {
  state: string;
  returnUrl?: string;
  createdAt: Date;
  expiresAt: Date;
}

export interface OAuthStateStore {
  create(returnUrl?: string): Promise<string>;
  validate(state: string): Promise<{ valid: boolean; returnUrl?: string }>;
  invalidate(state: string): Promise<void>;
}

const DEFAULT_TTL_MS = 10 * 60 * 1000; // 10 minutes

export function createFirestoreStateStore(
  firestore: Firestore,
  collectionName = 'oauth_state',
  ttlMs = DEFAULT_TTL_MS,
): OAuthStateStore {
  const collection = firestore.collection(collectionName);

  return {
    async create(returnUrl?: string): Promise<string> {
      const state = randomBytes(32).toString('hex');
      const now = new Date();
      const expiresAt = new Date(now.getTime() + ttlMs);

      const data: Record<string, unknown> = {
        createdAt: now,
        expiresAt,
      };

      if (returnUrl !== undefined) {
        data.returnUrl = returnUrl;
      }

      await collection.doc(state).set(data);

      return state;
    },

    async validate(state: string): Promise<{ valid: boolean; returnUrl?: string }> {
      const doc = await collection.doc(state).get();

      if (!doc.exists) {
        return { valid: false };
      }

      const data = doc.data();
      if (!data) {
        return { valid: false };
      }

      const expiresAt = data.expiresAt;
      let expiresAtDate: Date;

      if (expiresAt && typeof expiresAt.toDate === 'function') {
        expiresAtDate = expiresAt.toDate();
      } else if (expiresAt instanceof Date) {
        expiresAtDate = expiresAt;
      } else {
        return { valid: false };
      }

      if (expiresAtDate < new Date()) {
        return { valid: false };
      }

      return {
        valid: true,
        returnUrl: data.returnUrl as string | undefined,
      };
    },

    async invalidate(state: string): Promise<void> {
      await collection.doc(state).delete();
    },
  };
}
