// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Application } from 'express';

import { URL } from 'url';

import * as mongoosedb from 'mongoose';
import * as logger from 'winston';

export const mongoose = (app: Application): void => {
  let url = app.get('mongodb');
  if (process.env.MODEL_MONGO_USERNAME && process.env.MODEL_MONGO_PASSWORD) {
    const exploded = new URL(url);
    exploded.username = process.env.MODEL_MONGO_USERNAME;
    exploded.password = process.env.MODEL_MONGO_PASSWORD;
    url = exploded.toString();
    logger.info(`mongo url: ${url}`);
  }

  (mongoosedb as any).Promise = global.Promise;

  mongoosedb.connect(url, { useNewUrlParser: true });

  app.set('mongooseClient', mongoosedb);
};
