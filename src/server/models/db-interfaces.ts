// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { File } from '../schemas/file_pb';
import { Preview } from '../schemas/preview_pb';
import { Project } from '../schemas/project_pb';
import { User } from '../schemas/user_pb';
import { Table } from './table';

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
