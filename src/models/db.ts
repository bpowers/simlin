// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Database, DatabaseOptions } from './db-interfaces';

import { createMongoDatabase } from './db-mongo';

export async function createDatabase(opts: DatabaseOptions): Promise<Database> {
  switch (opts.backend) {
    case 'mongo':
      return createMongoDatabase(opts);
    default:
      throw new Error('not implemented yet');
  }
}
