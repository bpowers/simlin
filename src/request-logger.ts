// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';
import * as logger from 'winston';

import { interceptWriteHeaders } from './headers';

function now(): number {
  const time: [number, number] = process.hrtime();
  return time[0] + time[1] / 1.0e9;
}

export function requestLogger(req: Request, res: Response, next: NextFunction): void {
  const start = now();
  let headersWritten = false;

  interceptWriteHeaders(res, (statusCode: number) => {
    const durationS = now() - start;
    logger.log({
      level: 'info',
      message: `API-LINE status=${statusCode} method="${req.method}" path="${req.originalUrl}" duration_s=${durationS}`,
    });
    headersWritten = true;
  });

  try {
    next();
  } catch (err) {
    if (!headersWritten) {
      res.writeHead(500);
      headersWritten = true;
    }
    throw err;
  }
}
