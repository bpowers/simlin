// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Message } from 'google-protobuf';

export interface SerializableClass<T extends Message> {
  new (): T;
  deserializeBinary(bytes: Uint8Array): T;
}

export interface Table<T extends Message> {
  init(): Promise<void>;

  findOne(id: string): Promise<T | undefined>;
  findOneByScan(query: any): Promise<T | undefined>;
  findByScan(query: any): Promise<T[] | undefined>;
  find(idPrefix: string): Promise<T[]>;
  create(id: string, pb: T): Promise<void>;
  update(id: string, cond: any, pb: T): Promise<T | null>;
  deleteOne(id: string): Promise<void>;
}
