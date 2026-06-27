// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { getApps, initializeApp } from 'firebase-admin/app';
import { Firestore, getFirestore } from 'firebase-admin/firestore';

import { File } from '../schemas/file_pb';
import { Preview } from '../schemas/preview_pb';
import { Project } from '../schemas/project_pb';
import { User } from '../schemas/user_pb';

import { Database, DatabaseOptions } from './db-interfaces';
import { FirestoreTable, firestoreDocumentId } from './table-firestore';
import { Table } from './table';

export class FirestoreDatabase implements Database {
  private readonly client: Firestore;
  readonly file: Table<File>;
  readonly project: Table<Project>;
  readonly preview: Table<Preview>;
  readonly user: Table<User>;

  constructor(client: Firestore) {
    this.client = client;
    const db = this.client;

    this.file = new FirestoreTable(File, { db, name: 'files' });
    this.project = new FirestoreTable(Project, { db, name: 'project', hoistColumns: { version: 7 } });
    this.preview = new FirestoreTable(Preview, { db, name: 'preview' });
    this.user = new FirestoreTable(User, { db, name: 'user', hoistColumns: { email: 2 } });
  }

  async init(): Promise<void> {
    await Promise.all([this.file.init(), this.project.init(), this.preview.init(), this.user.init()]);
  }

  async deleteProjectAndFiles(projectId: string, currentFileId: string | undefined): Promise<void> {
    const files = await this.client.collection('files').where('projectId', '==', projectId).get();
    const historicalFileRefs = files.docs.filter((doc) => doc.id !== currentFileId).map((doc) => doc.ref);

    for (let i = 0; i < historicalFileRefs.length; i += 500) {
      const batch = this.client.batch();
      for (const ref of historicalFileRefs.slice(i, i + 500)) {
        batch.delete(ref);
      }
      await batch.commit();
    }

    const finalBatch = this.client.batch();
    finalBatch.delete(this.client.collection('project').doc(firestoreDocumentId(projectId)));
    if (currentFileId) {
      finalBatch.delete(this.client.collection('files').doc(firestoreDocumentId(currentFileId)));
    }
    await finalBatch.commit();
  }
}

export async function createFirestoreDatabase(_opts: DatabaseOptions): Promise<Database> {
  // getFirestore() resolves against the default Firebase app and throws
  // `app/no-app` if none has been initialized. App bootstrap calls
  // admin.initializeApp() before building the DB, but standalone callers (e.g.
  // scripts/debug-import.mjs) do not -- so initialize on demand when no app
  // exists yet. The previous `new Firestore()` had no such prerequisite.
  if (getApps().length === 0) {
    initializeApp();
  }
  const client = getFirestore();
  const db = new FirestoreDatabase(client);
  await db.init();
  return db;
}
