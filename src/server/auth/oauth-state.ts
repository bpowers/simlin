// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Firestore } from '@google-cloud/firestore';
import { createHash, randomBytes, timingSafeEqual } from 'crypto';

export interface OAuthState {
  state: string;
  returnUrl?: string;
  bindingHash: string;
  createdAt: Date;
  expiresAt: Date;
}

export interface CreateOAuthStateInput {
  returnUrl?: string;
  bindingSecret: string;
}

export interface ValidateOAuthStateInput {
  state: string;
  bindingSecret?: string;
}

export interface OAuthStateStore {
  create(input: CreateOAuthStateInput): Promise<string>;
  validate(input: ValidateOAuthStateInput): Promise<{ valid: boolean; returnUrl?: string }>;
  invalidate(state: string): Promise<void>;
}

export const DEFAULT_TTL_MS = 10 * 60 * 1000; // 10 minutes

function hashBindingSecret(bindingSecret: string): string {
  return createHash('sha256').update(bindingSecret, 'utf8').digest('hex');
}

function bindingSecretMatches(storedBindingHash: unknown, bindingSecret?: string): boolean {
  if (typeof storedBindingHash !== 'string' || !bindingSecret) {
    return false;
  }

  const actualHash = Buffer.from(storedBindingHash, 'utf8');
  const expectedHash = Buffer.from(hashBindingSecret(bindingSecret), 'utf8');

  if (actualHash.length !== expectedHash.length) {
    return false;
  }

  return timingSafeEqual(actualHash, expectedHash);
}

export function createFirestoreStateStore(
  firestore: Firestore,
  collectionName = 'oauth_state',
  ttlMs = DEFAULT_TTL_MS,
): OAuthStateStore {
  const collection = firestore.collection(collectionName);

  return {
    async create(input: CreateOAuthStateInput): Promise<string> {
      const state = randomBytes(32).toString('hex');
      const now = new Date();
      const expiresAt = new Date(now.getTime() + ttlMs);

      const data: Record<string, unknown> = {
        createdAt: now,
        expiresAt,
        bindingHash: hashBindingSecret(input.bindingSecret),
      };

      if (input.returnUrl !== undefined) {
        data.returnUrl = input.returnUrl;
      }

      await collection.doc(state).set(data);

      return state;
    },

    async validate(input: ValidateOAuthStateInput): Promise<{ valid: boolean; returnUrl?: string }> {
      const doc = await collection.doc(input.state).get();

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

      if (!bindingSecretMatches(data.bindingHash, input.bindingSecret)) {
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
