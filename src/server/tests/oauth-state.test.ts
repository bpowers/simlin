// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { createFirestoreStateStore } from '../auth/oauth-state';

function createMockFirestore() {
  const docs = new Map<string, { data: Record<string, unknown>; createTime: Date }>();

  const mockDoc = (id: string) => ({
    set: jest.fn(async (data: Record<string, unknown>) => {
      docs.set(id, { data, createTime: new Date() });
    }),
    get: jest.fn(async () => {
      const doc = docs.get(id);
      return {
        exists: doc !== undefined,
        data: () => doc?.data,
        createTime: doc?.createTime,
      };
    }),
    delete: jest.fn(async () => {
      docs.delete(id);
    }),
  });

  const collection = {
    doc: jest.fn((id: string) => mockDoc(id)),
  };

  return {
    collection: jest.fn(() => collection),
    _docs: docs,
    _mockDoc: mockDoc,
  };
}

describe('FirestoreOAuthStateStore', () => {
  it('should create unique state strings', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const state1 = await store.create();
    const state2 = await store.create();

    expect(state1).not.toBe(state2);
    expect(state1.length).toBeGreaterThanOrEqual(32);
    expect(state2.length).toBeGreaterThanOrEqual(32);
  });

  it('should store state document in Firestore', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const state = await store.create('/return-url');

    expect(firestore.collection).toHaveBeenCalledWith('oauth_state');
    expect(firestore._docs.has(state)).toBe(true);
  });

  it('should validate existing non-expired states', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const state = await store.create('/return-url');
    const result = await store.validate(state);

    expect(result.valid).toBe(true);
    expect(result.returnUrl).toBe('/return-url');
  });

  it('should reject unknown states', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const result = await store.validate('unknown-state-12345');

    expect(result.valid).toBe(false);
    expect(result.returnUrl).toBeUndefined();
  });

  it('should reject expired states', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(
      firestore as unknown as Parameters<typeof createFirestoreStateStore>[0],
      'oauth_state',
      1, // 1ms TTL
    );

    const state = await store.create('/return-url');

    await new Promise((resolve) => setTimeout(resolve, 10));

    const result = await store.validate(state);

    expect(result.valid).toBe(false);
  });

  it('should invalidate (delete) used states', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const state = await store.create('/return-url');
    expect(firestore._docs.has(state)).toBe(true);

    await store.invalidate(state);

    expect(firestore._docs.has(state)).toBe(false);
  });

  it('should store and retrieve returnUrl', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const state = await store.create('/projects/test/model');
    const result = await store.validate(state);

    expect(result.valid).toBe(true);
    expect(result.returnUrl).toBe('/projects/test/model');
  });

  it('should handle undefined returnUrl', async () => {
    const firestore = createMockFirestore();
    const store = createFirestoreStateStore(firestore as unknown as Parameters<typeof createFirestoreStateStore>[0]);

    const state = await store.create();
    const result = await store.validate(state);

    expect(result.valid).toBe(true);
    expect(result.returnUrl).toBeUndefined();
  });

  it('should use correct collection name', async () => {
    const firestore = createMockFirestore();
    const customCollection = 'custom_oauth_state';
    const store = createFirestoreStateStore(
      firestore as unknown as Parameters<typeof createFirestoreStateStore>[0],
      customCollection,
    );

    await store.create();

    expect(firestore.collection).toHaveBeenCalledWith(customCollection);
  });
});
