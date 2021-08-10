// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';

const isProduction = process.env.NODE_ENV === 'production';

export function redirectToHttps(req: Request, res: Response, next: NextFunction): void {
  if (isProduction && req.get('X-Forwarded-Proto') === 'http') {
    const host = req.get('host') || 'app.simlin.com';
    const secureUrl = 'https://' + host + req.originalUrl;
    res.removeHeader('Strict-Transport-Security');
    res.setHeader('Location', secureUrl);
    res.status(301).send();
    return;
  }

  next();
}
