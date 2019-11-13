// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { MongoClient } from 'mongodb';

import { File } from '../schemas/file_pb';
import { Preview } from '../schemas/preview_pb';
import { Project } from '../schemas/project_pb';
import { User } from '../schemas/user_pb';
import { MongoTable, Table } from './table';

export type DatabaseBackend = 'mongo' | 'bigtable';

export interface DatabaseOptions {
  url: string; // includes DB name
  backend: DatabaseBackend;
}

export interface Database {
  readonly file: Table<File>;
  readonly project: Table<Project>;
  readonly preview: Table<Preview>;
  readonly user: Table<User>;
}

export async function createDatabase(opts: DatabaseOptions): Promise<Database> {
  if (opts.backend !== 'mongo') {
    throw new Error('not implemented yet');
  }

  const client = new MongoClient(opts.url, {
    useUnifiedTopology: true,
  });
  await client.connect();

  const db = new MongoDatabase(client);

  await db.init();

  return db;
}

export class MongoDatabase {
  private readonly client: MongoClient;
  readonly file: Table<File>;
  readonly project: Table<Project>;
  readonly preview: Table<Preview>;
  readonly user: Table<User>;

  constructor(client: MongoClient) {
    this.client = client;
    const db = client.db();

    this.file = new MongoTable(File, { db, name: 'files2' });
    this.project = new MongoTable(Project, { db, name: 'project2', hoistColumns: { version: 7 } });
    this.preview = new MongoTable(Preview, { db, name: 'preview2' });
    this.user = new MongoTable(User, { db, name: 'user2', hoistColumns: { email: 2 } });
  }

  async init(): Promise<void> {
    await Promise.all([this.file.init(), this.project.init(), this.preview.init(), this.user.init()]);
  }
}
