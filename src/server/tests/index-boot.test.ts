// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// index.ts is the server entrypoint: importing it schedules boot via
// setImmediate. Mocking createApp and the logger lets us exercise just
// the boot-failure contract -- log and exit non-zero -- without touching
// Firebase or the real app. Exit-on-failure matters because a zombie
// process that never binds the port hangs a GAE instance until the
// port-bind timeout instead of recycling immediately.
jest.mock('../app', () => ({
  createApp: jest.fn(() => Promise.reject(new Error('boot failed: engine WASM missing'))),
}));
jest.mock('../logger', () => ({
  error: jest.fn(),
  info: jest.fn(),
  warn: jest.fn(),
}));

import * as logger from '../logger';

describe('server boot failure', () => {
  it('logs the error and exits non-zero when createApp rejects', async () => {
    const exit = jest.spyOn(process, 'exit').mockImplementation((() => undefined) as never);
    try {
      await import('../index');

      // boot runs on a later tick (setImmediate); poll until the catch fires
      // rather than sleeping a fixed amount
      await new Promise<void>((resolve) => {
        const check = (): void => {
          if (exit.mock.calls.length > 0) {
            resolve();
          } else {
            setImmediate(check);
          }
        };
        setImmediate(check);
      });

      expect(exit).toHaveBeenCalledWith(1);
      expect(logger.error).toHaveBeenCalledWith(expect.stringContaining('server startup failed'));
      expect(logger.error).toHaveBeenCalledWith(expect.stringContaining('engine WASM missing'));
    } finally {
      exit.mockRestore();
    }
  });
});
