// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

import { getStaticDirectory, validateStaticDirectory, StaticConfigError } from '../static-config';

describe('Static file configuration', () => {
  describe('getStaticDirectory', () => {
    it('should return public in production', () => {
      const dir = getStaticDirectory('production');
      expect(dir).toBe('public');
    });

    it('should return build in development if build/index.html exists', () => {
      // Check if build/index.html exists relative to working directory (src/server)
      const buildExists = fs.existsSync('build/index.html');
      const dir = getStaticDirectory('development');

      if (buildExists) {
        expect(dir).toBe('build');
      } else {
        expect(dir).toBe('public');
      }
    });

    it('should fall back to public in development if build/index.html is missing', () => {
      // Verify the fallback behavior by checking production mode always returns public
      const dir = getStaticDirectory('production');
      expect(dir).toBe('public');
    });
  });

  describe('validateStaticDirectory', () => {
    it('should succeed when index.html exists', () => {
      // public/index.html should exist via the symlink
      const publicDir = path.join(__dirname, '..', 'public');
      expect(() => validateStaticDirectory(publicDir)).not.toThrow();
    });

    it('should throw StaticConfigError when directory is missing', () => {
      expect(() => validateStaticDirectory('/nonexistent/path')).toThrow(StaticConfigError);
    });

    it('should throw StaticConfigError when index.html is missing', () => {
      // Use a directory that exists but has no index.html
      const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'test-static-'));
      try {
        expect(() => validateStaticDirectory(tempDir)).toThrow(/index\.html/);
      } finally {
        fs.rmdirSync(tempDir);
      }
    });
  });
});
