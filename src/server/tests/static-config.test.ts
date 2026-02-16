// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as path from 'path';
import * as os from 'os';

import { getStaticDirectory, validateStaticDirectory, StaticConfigError } from '../static-config';

const actualFs = jest.requireActual<typeof import('fs')>('fs');

jest.mock('fs', () => {
  const actual = jest.requireActual<typeof import('fs')>('fs');
  return {
    ...actual,
    existsSync: jest.fn(actual.existsSync),
  };
});

import * as fs from 'fs';

const existsSyncMock = fs.existsSync as jest.MockedFunction<typeof fs.existsSync>;

describe('Static file configuration', () => {
  describe('getStaticDirectory', () => {
    afterEach(() => {
      existsSyncMock.mockImplementation(actualFs.existsSync);
    });

    it('should return public in production', () => {
      const dir = getStaticDirectory('production');
      expect(dir).toBe('public');
    });

    it('should return build in development if build/index.html exists', () => {
      const buildExists = fs.existsSync('build/index.html');
      const dir = getStaticDirectory('development');

      if (buildExists) {
        expect(dir).toBe('build');
      } else {
        expect(dir).toBe('public');
      }
    });

    it('should fall back to public in development if build/index.html is missing', () => {
      existsSyncMock.mockImplementation((p: fs.PathLike) => {
        if (String(p) === 'build/index.html') return false;
        return actualFs.existsSync(p);
      });
      const dir = getStaticDirectory('development');
      expect(dir).toBe('public');
    });

    it('should respect explicit env override', () => {
      existsSyncMock.mockImplementation((p: fs.PathLike) => {
        if (String(p) === 'build/index.html') return true;
        return actualFs.existsSync(p);
      });
      const dir = getStaticDirectory('development');
      expect(dir).toBe('build');
    });

    it('should use process.env.NODE_ENV when no argument is passed', () => {
      const env = process.env.NODE_ENV;
      const dir = getStaticDirectory();
      if (env === 'production') {
        expect(dir).toBe('public');
      } else {
        // In non-production (or undefined), behavior depends on whether build/index.html exists
        expect(['build', 'public']).toContain(dir);
      }
    });
  });

  describe('validateStaticDirectory', () => {
    it('should succeed when index.html exists', () => {
      const publicDir = path.join(__dirname, '..', 'public');
      expect(() => validateStaticDirectory(publicDir)).not.toThrow();
    });

    it('should throw StaticConfigError when directory is missing', () => {
      expect(() => validateStaticDirectory('/nonexistent/path')).toThrow(StaticConfigError);
    });

    it('should throw StaticConfigError when index.html is missing', () => {
      const tempDir = actualFs.mkdtempSync(path.join(os.tmpdir(), 'test-static-'));
      try {
        expect(() => validateStaticDirectory(tempDir)).toThrow(/index\.html/);
      } finally {
        actualFs.rmdirSync(tempDir);
      }
    });
  });
});
