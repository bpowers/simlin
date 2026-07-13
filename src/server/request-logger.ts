// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';
import * as logger from './logger';

import { interceptWriteHeaders } from './headers';
import { getSessionUserId } from './session-auth';

function now(): number {
  const time: [number, number] = process.hrtime();
  return time[0] + time[1] / 1.0e9;
}

function maybeGetUser(req: Request): string {
  // requestLogger sits before seshcookie in app.ts, so a response
  // written before the session middleware runs would log with no
  // session at all; never assume it exists or is well-formed here
  // (getSessionUserId tolerates both).
  const id = getSessionUserId(req);
  return id === undefined ? '' : ` user="${id}"`;
}

export function requestLogger(req: Request, res: Response, next: NextFunction): void {
  const start = now();
  let headersWritten = false;

  interceptWriteHeaders(res, (statusCode: number) => {
    const durationMs = ((now() - start) * 1000).toFixed(1);
    const log =
      `API-LINE status=${statusCode} method="${req.method}" path="${req.originalUrl}" duration_ms=${durationMs}` +
      maybeGetUser(req);
    logger.info(log);
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
