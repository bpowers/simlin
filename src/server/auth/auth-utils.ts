// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request } from 'express';
import { User } from '../schemas/user_pb';

// Promisify passport's callback-based req.login
export function loginUser(req: Request, user: User): Promise<void> {
  return new Promise((resolve, reject) => {
    req.login(user, (err) => {
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    });
  });
}
