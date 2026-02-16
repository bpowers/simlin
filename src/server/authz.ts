// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';

function jsonError(res: Response): void {
  res.status(401).json({ error: 'unauthorized' });
}

export default (req: Request, res: Response, next: NextFunction, onFail?: (res: Response) => void): void => {
  // allow unauthorized access to projects for embedding in blogs
  const failEarly = !(req.method === 'GET' && req.path.startsWith('/projects/'));

  if (!req.session || !req.session.passport) {
    // clear session to unset cookie
    req.session = {};

    if (failEarly) {
      (onFail ?? jsonError)(res);
      return;
    }
  } else if (!req.session.passport.user) {
    // clear session to unset cookie
    req.session = {};

    if (failEarly) {
      (onFail ?? jsonError)(res);
      return;
    }
  }

  next();
};
