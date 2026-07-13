// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';

import { isAuthenticated } from './session-auth';

// The one /api surface anonymous requests may reach: the public
// project-detail route, GET /projects/<username>/<projectName>. The
// pattern mirrors Express's dispatch semantics -- case-insensitive,
// non-strict (one optional trailing slash), exactly two non-empty
// segments -- so the carve-out and the router can't disagree about a
// path: a bare startsWith('/projects/') also admitted /projects/, the
// trailing-slash alias of the authenticated LIST route, which then
// 500'd in getUser instead of answering 401.
const publicProjectDetail = /^\/projects\/[^/]+\/[^/]+\/?$/i;

// Express dispatches a 4-arg function as an error handler, not a
// request middleware. Keep this default export at exactly 3 declared
// parameters so `app.use('/api', authz, ...)` actually invokes it on
// every request. See tests/authz.test.ts.
export default (req: Request, res: Response, next: NextFunction): void => {
  // allow unauthorized access to individual projects for embedding in blogs
  const failEarly = !(req.method === 'GET' && publicProjectDetail.test(req.path));

  // Authorization requires the deserialized req.user (set by sessionAuth
  // from a live DB record), not merely an authenticated-looking session:
  // a stale cookie naming a deleted user has valid shape but must be
  // treated as unauthenticated (issue #930).
  if (!isAuthenticated(req)) {
    // clear session to unset cookie; sessionAuth already emptied stale
    // sessions, this additionally covers shapes it doesn't recognize
    req.session = {};

    if (failEarly) {
      res.status(401).json({ error: 'unauthorized' });
      return;
    }
  }

  next();
};
