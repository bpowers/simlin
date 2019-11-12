// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Collection, Db } from 'mongodb';

import { Message } from 'google-protobuf';

import { defined } from '../engine/common';

interface SerializableClass {
  new (): Message;
  deserializeBinary(bytes: Uint8Array): Message;
}

export interface Table<T extends SerializableClass> {
  findOne(id: string): Promise<T>;
  find(idPrefix: string): Promise<T[]>;
  create(id: string, pb: T): Promise<void>;
  update(id: string, cond: any, pb: T): Promise<void>;
}

interface MongoTableOptions {
  readonly db?: Db;
  readonly name: string;
  readonly hoistColumns?: string[];
}

interface Schema<T> {
  _id: string;
  // additional stuff
  value: T;
}

export class MongoTable<T extends SerializableClass> implements Table<T> {
  readonly kind: T;
  readonly opts: MongoTableOptions;
  collection?: Collection<Schema<T>>;
  private collectionPromise?: Promise<Collection<Schema<T>>>;

  constructor(t: T, opts: MongoTableOptions) {
    this.kind = t;
    this.opts = opts;

    const { db } = opts;
    this.collectionPromise = defined(db).createCollection(this.opts.name);
  }

  async init(): Promise<void> {
    this.collection = await this.collectionPromise;
    this.collectionPromise = undefined;
  }

  async findOne(id: string): Promise<T> {
    const row = await defined(this.collection).findOne({ id });
    if (!row || !row.value) {
      throw new Error('not found');
    }
    return row.value;
  }

  async find(idPrefix: string): Promise<T[]> {
    const cursor = await defined(this.collection).find({ id: new RegExp(`^${idPrefix}`) });
    if (!cursor) {
      throw new Error('not found');
    }
    const rows = await cursor.toArray();
    return rows.map(r => r.value);
  }

  private doc(id: string, pb: T): Schema<T> {
    const doc: Schema<T> = {
      _id: id,
      value: pb,
    };
    // TODO: pull fields from PB out onto doc
    return doc;
  }

  async create(id: string, pb: T): Promise<void> {
    await defined(this.collection).insertOne(this.doc(id, pb));
    return;
  }

  async update(id: string, cond: any, pb: T): Promise<void> {
    await defined(this.collection).findOneAndUpdate(Object.assign({ _id: id }, cond), this.doc(id, pb));
    return;
  }
}
