// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';

// Express dispatches a 4-arg function as an error handler, not a
// request middleware. Keep this default export at exactly 3 declared
// parameters so `app.use('/api', authz, ...)` actually invokes it on
// every request. See tests/authz.test.ts.
export default (req: Request, res: Response, next: NextFunction): void => {
  // allow unauthorized access to projects for embedding in blogs
  const failEarly = !(req.method === 'GET' && req.path.startsWith('/projects/'));

  if (!req.session || !req.session.passport || !req.session.passport.user) {
    // clear session to unset cookie
    req.session = {};

    if (failEarly) {
      res.status(401).json({ error: 'unauthorized' });
      return;
    }
  }

  next();
};
