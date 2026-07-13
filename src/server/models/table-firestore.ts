// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { CollectionReference, FieldPath, Firestore } from 'firebase-admin/firestore';
import { Message } from 'google-protobuf';

import { AlreadyExistsError, PreconditionFailedError, Query, SerializableClass, Table } from './table';

// A duplicate-id create is rejected by the backend with gRPC status
// ALREADY_EXISTS, surfaced as a NUMERIC `code` of 6 in this dependency
// tree (google-gax build/src/status.js pins Status.ALREADY_EXISTS = 6;
// @google-cloud/firestore build/src/status-code.d.ts carries an internal
// copy of the same enum, and nothing in its write path remaps it to a
// string). The string form is accepted too as insurance against the SDK
// someday adopting firebase-admin-style string codes.
function isAlreadyExistsRejection(error: unknown): boolean {
  if (typeof error !== 'object' || error === null) {
    return false;
  }
  const code = (error as Record<string, unknown>).code;
  return code === 6 || code === 'already-exists';
}

interface FirestoreTableOptions {
  readonly db: Firestore;
  readonly name: string;
  readonly hoistColumns?: { [col: string]: number };
}

interface Schema {
  // with Firestore, you specify the document name separately from the contents
  // _id: string;
  // additional stuff
  [x: string]: unknown;
}

export function firestoreDocumentId(id: string): string {
  return id.replace('/', '|');
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

  private docRef(id: string) {
    return this.collection.doc(firestoreDocumentId(id));
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

  async findOneByScan(query: Query): Promise<T | undefined> {
    const docs = await this.findByScan(query);
    if (docs === undefined) {
      return undefined;
    }
    if (docs.length !== 1) {
      throw new Error(`findOneByScan: expected single result document, not ${docs.length}`);
    }
    return docs[0];
  }

  async findByScan(query: Query): Promise<T[] | undefined> {
    const keys = Object.keys(query);
    if (keys.length !== 1) {
      throw new Error('findByScan: expected single query key');
    }
    const key = keys[0];
    const querySnapshot = await this.collection.where(key, '==', query[key]).get();
    if (!querySnapshot || querySnapshot.empty) {
      return undefined;
    }
    return querySnapshot.docs.map((doc) => this.deserialize(doc.get('value')));
  }

  async find(idPrefix: string): Promise<T[]> {
    idPrefix = firestoreDocumentId(idPrefix);
    // https://stackoverflow.com/questions/46573804/firestore-query-documents-startswith-a-string
    const successor =
      idPrefix.substring(0, idPrefix.length - 1) + String.fromCharCode(idPrefix.charCodeAt(idPrefix.length - 1) + 1);
    const querySnapshot = await this.collection
      .where(FieldPath.documentId(), '>=', idPrefix)
      .where(FieldPath.documentId(), '<', successor)
      .get();
    if (!querySnapshot || querySnapshot.empty) {
      return [];
    }

    return querySnapshot.docs.map((docRef) => this.deserialize(docRef.get('value')));
  }

  private doc(_id: string, pb: T): Schema {
    const serializedPb = pb.serializeBinary();
    const doc = pb.toObject() as Record<string, unknown>;

    if (doc.hasOwnProperty('value')) {
      throw new Error('we expect document to not have "value" property');
    }

    // firestore doesn't like JS 'undefined'
    for (const [key, value] of Object.entries(doc)) {
      if (value === undefined) {
        doc[key] = null;
      }

      if (key === 'jsonContents') {
        const contents = value;
        // if the JSON is too big, don't expose it (as its only for debugging info anyway)
        if (typeof contents === 'string' && contents.length > 100 * 1024) {
          doc[key] = null;
        }
      }
    }

    doc['value'] = Buffer.from(serializedPb);

    // if (this.opts.hoistColumns) {
    //   const cols = this.opts.hoistColumns;
    //   for (const prop in cols) {
    //     if (!cols.hasOwnProperty(prop)) {
    //       continue;
    //     }
    //     doc[prop] = Message.getFieldWithDefault(pb, cols[prop], undefined);
    //   }
    // }
    return doc as Schema;
  }

  async create(id: string, pb: T): Promise<void> {
    const docRef = this.docRef(id);
    try {
      await docRef.create(this.doc(id, pb));
    } catch (err) {
      if (isAlreadyExistsRejection(err)) {
        throw new AlreadyExistsError(`${this.opts.name}/${firestoreDocumentId(id)} already exists`);
      }
      throw err;
    }
  }

  async update(id: string, cond: Query, pb: T): Promise<T | null> {
    try {
      await this.db.runTransaction(async (tx) => {
        const docRef = this.docRef(id);
        const doc = await tx.get(docRef);
        for (const [key, expected] of Object.entries(cond)) {
          const current = doc.get(key);
          if (current !== expected) {
            // A code-less error thrown here rejects runTransaction as-is,
            // unretried and unwrapped: the SDK's attempt loop only retries
            // errors carrying a retryable numeric gRPC code (verified
            // against @google-cloud/firestore 7.11.6 transaction.js). A
            // retryable COMMIT failure re-runs this callback, which
            // re-reads the doc and re-evaluates the condition -- still
            // correct.
            throw new PreconditionFailedError(`precondition ${key} failed: ${expected} != ${current}`);
          }
        }
        tx.update(docRef, this.doc(id, pb));
      });
    } catch (err) {
      if (err instanceof PreconditionFailedError) {
        return null;
      }
      // A transport or backend failure is not a failed precondition;
      // mapping it to null made outages read as version conflicts.
      throw err;
    }

    return pb;
  }

  async deleteOne(id: string): Promise<void> {
    await this.docRef(id).delete();
  }
}
