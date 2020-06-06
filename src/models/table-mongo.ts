// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Binary } from 'bson';
import { Message } from 'google-protobuf';
import { Collection, Db } from 'mongodb';

import { defined } from '../engine/common';
import { SerializableClass, Table } from './table';

interface MongoTableOptions {
  readonly db?: Db;
  readonly name: string;
  readonly hoistColumns?: { [col: string]: number };
}

interface Schema<T> {
  _id: string;
  // additional stuff
  value: Binary;
}

export class MongoTable<T extends Message> implements Table<T> {
  readonly kind: SerializableClass<T>;
  readonly opts: MongoTableOptions;
  collection?: Collection<Schema<T>>;
  private collectionPromise?: Promise<Collection<Schema<T>>>;

  constructor(t: SerializableClass<T>, opts: MongoTableOptions) {
    this.kind = t;
    this.opts = opts;

    const { db } = opts;
    this.collectionPromise = defined(db).createCollection(this.opts.name);
  }

  async init(): Promise<void> {
    this.collection = await this.collectionPromise;
    this.collectionPromise = undefined;
  }

  private deserialize(row: Schema<T>): T {
    const value = row.value;
    return this.kind.deserializeBinary(value.read(0, value.length()));
  }

  async findOne(id: string): Promise<T | undefined> {
    const row = await defined(this.collection).findOne({ _id: id });
    if (!row || !row.value) {
      return undefined;
    }
    return this.deserialize(row);
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any,@typescript-eslint/explicit-module-boundary-types
  async findOneByScan(query: any): Promise<T | undefined> {
    const row = await defined(this.collection).findOne(query);
    if (!row || !row.value) {
      return undefined;
    }
    return this.deserialize(row);
  }

  async find(idPrefix: string): Promise<T[]> {
    // eslint-disable-next-line @typescript-eslint/await-thenable
    const cursor = await defined(this.collection).find({ _id: new RegExp(`^${idPrefix}`) });
    if (!cursor) {
      throw new Error('not found');
    }
    const rows = await cursor.toArray();
    return rows.map((row) => this.deserialize(row));
  }

  private doc(id: string, pb: T): Schema<T> {
    const serializedPb = pb.serializeBinary();
    const doc: Schema<T> = {
      _id: id,
      value: new Binary(Buffer.from(serializedPb)),
    };
    if (this.opts.hoistColumns) {
      const cols = this.opts.hoistColumns;
      for (const prop in cols) {
        if (!cols.hasOwnProperty(prop)) {
          continue;
        }
        doc[prop] = Message.getFieldWithDefault(pb, cols[prop], undefined);
      }
    }
    return doc;
  }

  async create(id: string, pb: T): Promise<void> {
    await defined(this.collection).insertOne(this.doc(id, pb));
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any,@typescript-eslint/explicit-module-boundary-types
  async update(id: string, cond: any, pb: T): Promise<T | null> {
    const result = await defined(this.collection).findOneAndUpdate(Object.assign({ _id: id }, cond), {
      $set: this.doc(id, pb),
    });
    return result.value ? this.deserialize(result.value) : null;
  }

  async deleteOne(id: string): Promise<void> {
    await defined(this.collection).deleteOne({ _id: id });
  }
}
