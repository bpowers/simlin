// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Pins the Table.create rejection contract: FirestoreTable maps the SDK's
// duplicate-document rejection (gRPC ALREADY_EXISTS, numeric code 6 -- see
// the comment in models/table-firestore.ts for the source citations) onto
// AlreadyExistsError so handlers can branch on "already exists" without
// duck-typing gRPC error shapes. Anything else must pass through untouched.

import { describe, it, expect } from '@rstest/core';
import type { Firestore } from 'firebase-admin/firestore';

import { AlreadyExistsError } from '../models/table';
import { FirestoreTable } from '../models/table-firestore';
import { File } from '../schemas/file_pb';

interface FakeTx {
  get: (ref: unknown) => Promise<{ get: (key: string) => unknown }>;
  update: (ref: unknown, doc: unknown) => void;
}

function tableWithCreate(create: () => Promise<unknown>): FirestoreTable<File> {
  const fake = {
    collection: () => ({
      doc: () => ({ create }),
    }),
  } as unknown as Firestore;
  return new FirestoreTable(File, { db: fake, name: 'files' });
}

function grpcAlreadyExists(): Error {
  // The shape @google-cloud/firestore rejects with when the doc exists.
  return Object.assign(new Error('6 ALREADY_EXISTS: Document already exists: projects/p/databases/(default)'), {
    code: 6,
  });
}

describe('FirestoreTable.create duplicate mapping', () => {
  it('maps the gRPC numeric ALREADY_EXISTS code onto AlreadyExistsError', async () => {
    const table = tableWithCreate(() => Promise.reject(grpcAlreadyExists()));
    await expect(table.create('file-1', new File())).rejects.toBeInstanceOf(AlreadyExistsError);
  });

  it('maps a string already-exists code (firebase-admin style) the same way', async () => {
    const err = Object.assign(new Error('already exists'), { code: 'already-exists' });
    const table = tableWithCreate(() => Promise.reject(err));
    await expect(table.create('file-1', new File())).rejects.toBeInstanceOf(AlreadyExistsError);
  });

  it('passes through unrelated rejections unchanged', async () => {
    const err = Object.assign(new Error('14 UNAVAILABLE: connection dropped'), { code: 14 });
    const table = tableWithCreate(() => Promise.reject(err));
    await expect(table.create('file-1', new File())).rejects.toBe(err);
  });

  it('resolves normally when the backend accepts the write', async () => {
    const table = tableWithCreate(() => Promise.resolve({}));
    await expect(table.create('file-1', new File())).resolves.toBeUndefined();
  });
});

// update's contract: `null` means exactly "a precondition did not hold" --
// the optimistic-concurrency signal the save handler turns into a 409 and
// the client turns into destructive conflict recovery. A transport failure
// disguised as null would drive users into that flow during an outage.
describe('FirestoreTable.update failure semantics', () => {
  function tableWithStoredDoc(stored: Record<string, unknown>): { table: FirestoreTable<File>; updates: unknown[] } {
    const updates: unknown[] = [];
    const fake = {
      collection: () => ({ doc: () => ({}) }),
      runTransaction: (fn: (tx: FakeTx) => Promise<void>) =>
        fn({
          get: () => Promise.resolve({ get: (key: string) => stored[key] }),
          update: (_ref: unknown, doc: unknown): void => {
            updates.push(doc);
          },
        }),
    } as unknown as Firestore;
    return { table: new FirestoreTable(File, { db: fake, name: 'files' }), updates };
  }

  it('returns null when a precondition does not hold, without writing', async () => {
    const { table, updates } = tableWithStoredDoc({ version: 2 });
    await expect(table.update('file-1', { version: 1 }, new File())).resolves.toBeNull();
    expect(updates).toHaveLength(0);
  });

  it('applies the write and returns the pb when preconditions hold', async () => {
    const { table, updates } = tableWithStoredDoc({ version: 1 });
    const pb = new File();
    await expect(table.update('file-1', { version: 1 }, pb)).resolves.toBe(pb);
    expect(updates).toHaveLength(1);
  });

  it('propagates transport failures instead of disguising them as precondition misses', async () => {
    const err = Object.assign(new Error('14 UNAVAILABLE: connection dropped'), { code: 14 });
    const fake = {
      collection: () => ({ doc: () => ({}) }),
      runTransaction: () => Promise.reject(err),
    } as unknown as Firestore;
    const table = new FirestoreTable(File, { db: fake, name: 'files' });
    await expect(table.update('file-1', { version: 1 }, new File())).rejects.toBe(err);
  });
});
