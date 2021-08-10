// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Database, DatabaseOptions } from './db-interfaces';

import { createFirestoreDatabase } from './db-firestore';

export async function createDatabase(opts: DatabaseOptions): Promise<Database> {
  switch (opts.backend) {
    case 'firestore':
      return createFirestoreDatabase(opts);
    default:
      throw new Error('not implemented yet');
  }
}
