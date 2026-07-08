// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect, beforeAll, afterEach, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

import * as path from 'path';
import * as os from 'os';

import { getStaticDirectory, validateStaticDirectory, StaticConfigError } from '../static-config';

// `{ spy: true }` wraps every export in a spy that still calls through, which is
// what the old `{ ...actual, existsSync: jest.fn(actual.existsSync) }` factory
// did by hand. A factory cannot be used: it is hoisted above the imports (so it
// cannot close over one), rstest rejects async factories, and the synchronous
// escape hatch -- `import ... with { rstest: 'importActual' }` -- needs an ES
// module target, while this package's program emits CommonJS and type-checks its
// own tests. Hoisted above the `fs` import below, exactly as jest.mock was.
rs.mock('fs', { spy: true });

import * as fs from 'fs';

// Tests that stub existsSync fall back on the real implementation, which must be
// the unmocked one or the stub would recurse into itself.
let actualFs: typeof import('fs');
beforeAll(async () => {
  actualFs = await rs.importActual<typeof import('fs')>('fs');
});

const existsSyncMock = fs.existsSync as unknown as Mock<typeof fs.existsSync>;

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
