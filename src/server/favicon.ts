// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { readFileSync } from 'fs';

import { NextFunction, Request, RequestHandler, Response } from 'express';

/**
 * Serve a single favicon from memory.
 *
 * Replaces the serve-favicon package for the one behavior we used: read
 * the icon once at startup, answer GET/HEAD /favicon.ico with long-lived
 * caching, reject other methods with 405 + Allow, and defer every other
 * path. Reading synchronously at setup is deliberate -- a missing icon
 * should fail server startup loudly, not 500 at request time.
 */
export function favicon(iconPath: string): RequestHandler {
  const icon = readFileSync(iconPath);
  const oneYearInSeconds = 365 * 24 * 60 * 60;

  return (req: Request, res: Response, next: NextFunction): void => {
    if (req.path !== '/favicon.ico') {
      next();
      return;
    }
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.status(405).setHeader('Allow', 'GET, HEAD');
      res.end();
      return;
    }
    res.setHeader('Content-Type', 'image/x-icon');
    res.setHeader('Cache-Control', `public, max-age=${oneYearInSeconds}`);
    res.send(icon);
  };
}
