// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// getFirestore() resolves against the default Firebase app, so
// createFirestoreDatabase must ensure one exists -- standalone callers (e.g.
// scripts/debug-import.mjs) reach createDatabase without App bootstrap having
// run admin.initializeApp(). These tests pin that on-demand init contract.

import { getApps, initializeApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { createFirestoreDatabase } from '../models/db-firestore';

jest.mock('firebase-admin/app', () => ({
  getApps: jest.fn(),
  initializeApp: jest.fn(),
}));

jest.mock('firebase-admin/firestore', () => ({
  // FirestoreTable's constructor only needs `db.collection(name)`; init() is a
  // no-op, so a minimal stub client is enough to drive createFirestoreDatabase.
  getFirestore: jest.fn(() => ({ collection: () => ({}) })),
}));

describe('createFirestoreDatabase', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  test('initializes a default app on demand when none exists', async () => {
    (getApps as jest.Mock).mockReturnValue([]);
    await createFirestoreDatabase({ backend: 'firestore' });
    expect(initializeApp).toHaveBeenCalledTimes(1);
    expect(getFirestore).toHaveBeenCalledTimes(1);
  });

  test('reuses an already-initialized app (no double init)', async () => {
    (getApps as jest.Mock).mockReturnValue([{}]);
    await createFirestoreDatabase({ backend: 'firestore' });
    expect(initializeApp).not.toHaveBeenCalled();
    expect(getFirestore).toHaveBeenCalledTimes(1);
  });
});
