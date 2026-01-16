// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { File } from '../schemas/file_pb.js';
import { Preview } from '../schemas/preview_pb.js';
import { Project } from '../schemas/project_pb.js';
import { User } from '../schemas/user_pb.js';
import { Table } from './table.js';

export type DatabaseBackend = 'firestore';

export interface DatabaseOptions {
  backend: DatabaseBackend;
}

export interface Database {
  readonly file: Table<File>;
  readonly project: Table<Project>;
  readonly preview: Table<Preview>;
  readonly user: Table<User>;
}
