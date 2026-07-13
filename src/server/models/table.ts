// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Message } from 'google-protobuf';

export type Query = Readonly<Record<string, unknown>>;

/**
 * Rejection contract for `Table.create` when a document with the given id
 * already exists: implementations map their backend's duplicate rejection
 * (Firestore surfaces gRPC ALREADY_EXISTS) onto this class, so callers can
 * branch on "already exists" without knowing backend error shapes.
 */
export class AlreadyExistsError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'AlreadyExistsError';
  }
}

/**
 * Sentinel thrown inside FirestoreTable.update's transaction callback when
 * a conditional precondition does not hold; update maps it to its `null`
 * return. This keeps `null` meaning exactly "precondition failed" -- the
 * optimistic-concurrency conflict signal -- while transport and other
 * backend failures propagate to the caller as rejections.
 *
 * Deliberately carries no `code` property: the Firestore SDK retries a
 * transaction callback whose error has a retryable numeric gRPC code, and
 * treats code-less errors as non-retryable, rejecting them unwrapped.
 */
export class PreconditionFailedError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PreconditionFailedError';
  }
}

export interface SerializableClass<T extends Message> {
  new (): T;
  deserializeBinary(bytes: Uint8Array): T;
}

export interface Table<T extends Message> {
  init(): Promise<void>;

  findOne(id: string): Promise<T | undefined>;
  findOneByScan(query: Query): Promise<T | undefined>;
  findByScan(query: Query): Promise<T[] | undefined>;
  find(idPrefix: string): Promise<T[]>;
  create(id: string, pb: T): Promise<void>;
  update(id: string, cond: Query, pb: T): Promise<T | null>;
  deleteOne(id: string): Promise<void>;
}
