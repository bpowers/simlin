// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Firestore } from '@google-cloud/firestore';

import { File } from '../schemas/file_pb';
import { Preview } from '../schemas/preview_pb';
import { Project } from '../schemas/project_pb';
import { User } from '../schemas/user_pb';

import { Database, DatabaseOptions } from './db-interfaces';
import { FirestoreTable } from './table-firestore';
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
}

export async function createFirestoreDatabase(_opts: DatabaseOptions): Promise<Database> {
  const client = new Firestore();
  const db = new FirestoreDatabase(client);
  await db.init();
  return db;
}
