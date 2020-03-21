// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { CollectionReference, Firestore } from '@google-cloud/firestore';
import { FieldPath } from '@google-cloud/firestore/build/src';
import { Message } from 'google-protobuf';

import { SerializableClass, Table } from './table';

interface FirestoreTableOptions {
  readonly db: Firestore;
  readonly name: string;
  readonly hoistColumns?: { [col: string]: number };
}

interface Schema<T> {
  // with Firestore, you specify the document name separately from the contents
  // _id: string;
  // additional stuff
  value: any;
}

export class FirestoreTable<T extends Message> implements Table<T> {
  readonly kind: SerializableClass<T>;
  readonly opts: FirestoreTableOptions;
  readonly collection: CollectionReference;
  private readonly db: Firestore;

  constructor(t: SerializableClass<T>, opts: FirestoreTableOptions) {
    this.kind = t;
    this.opts = opts;
    this.db = opts.db;
    this.collection = this.db.collection(opts.name);
  }

  async init(): Promise<void> {}

  private static filterId(id: string): string {
    return id.replace('/', '|');
  }

  private docRef(id: string) {
    return this.collection.doc(FirestoreTable.filterId(id));
  }

  private deserialize(value: Buffer): T {
    return this.kind.deserializeBinary(value);
  }

  async findOne(id: string): Promise<T | undefined> {
    const docSnapshot = await this.docRef(id).get();
    if (!docSnapshot || !docSnapshot.exists) {
      return undefined;
    }
    return this.deserialize(docSnapshot.get('value'));
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  async findOneByScan(query: any): Promise<T | undefined> {
    const keys = Object.keys(query);
    if (keys.length !== 1) {
      throw new Error('findOneByScan: expected single query key');
    }
    const key = keys[0];
    const querySnapshot = await this.collection.where(key, '==', query[key]).get();
    if (!querySnapshot || querySnapshot.empty) {
      return undefined;
    }
    if (querySnapshot.docs.length !== 1) {
      throw new Error(`findOneByScan: expected single result document, not ${querySnapshot.docs.length}`);
    }
    return this.deserialize(querySnapshot.docs[0].get('value'));
  }

  async find(idPrefix: string): Promise<T[]> {
    const querySnapshot = await this.collection
      .where(FieldPath.documentId(), '>=', FirestoreTable.filterId(idPrefix))
      .get();
    if (!querySnapshot || querySnapshot.empty) {
      throw new Error('not found');
    }

    return querySnapshot.docs.map(docRef => this.deserialize(docRef.get('value')));
  }

  private doc(_id: string, pb: T): Schema<T> {
    const serializedPb = pb.serializeBinary();
    const doc: Schema<T> = {
      value: Buffer.from(serializedPb),
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
    const docRef = this.docRef(id);
    await docRef.create(this.doc(id, pb));
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  async update(id: string, cond: any, pb: T): Promise<T | null> {
    try {
      await this.db.runTransaction(async tx => {
        const docRef = this.docRef(id);
        const doc = await tx.get(docRef);
        for (const [key, expected] of Object.entries(cond)) {
          const current = doc.get(key);
          if (current !== expected) {
            throw new Error(`precondition ${key} failed: ${expected} != ${current}`);
          }
        }
        tx.update(docRef, this.doc(id, pb));
      });
    } catch (err) {
      // our precondition failed
      return null;
    }

    return pb;
  }

  async deleteOne(id: string): Promise<void> {
    await this.docRef(id).delete();
  }
}
