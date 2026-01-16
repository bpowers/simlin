// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Application as ExpressApplication } from 'express';

import { Database } from './models/db-interfaces.js';

export interface Application extends ExpressApplication {
  db: Database;
}
