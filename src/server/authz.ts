// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';

function jsonError(res: Response): void {
  res.status(401).json({ error: 'unauthorized' });
}

function redirectError(res: Response): void {
  res.redirect('/login');
}

const doAuthz = (req: Request, res: Response, next: NextFunction, onFail: (res: Response) => void): void => {
  // allow unauthorized access to projects for embedding in blogs
  const failEarly = !(req.method === 'GET' && req.path.startsWith('/projects/'));

  if (!req.session || !req.session.passport) {
    // clear session to unset cookie
    req.session = {};

    if (failEarly) {
      onFail(res);
      return;
    }
  } else if (!req.session.passport.user) {
    // clear session to unset cookie
    req.session = {};

    if (failEarly) {
      onFail(res);
      return;
    }
  }

  next();
};

export const authz = (req: Request, res: Response, next: NextFunction): void => {
  doAuthz(req, res, next, jsonError);
};

export const userAuthz = (req: Request, res: Response, next: NextFunction): void => {
  doAuthz(req, res, next, redirectError);
};
