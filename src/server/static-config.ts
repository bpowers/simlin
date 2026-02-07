// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

/**
 * Error thrown when static file configuration is invalid.
 */
export class StaticConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'StaticConfigError';
  }
}

/**
 * Get the static file directory based on environment.
 *
 * - Production: always 'public' (matches app.yaml and deploy script)
 * - Development: 'build' if build/index.html exists, else 'public'
 *
 * In development, the 'build' directory is typically a symlink to ../app/build,
 * so developers can run the frontend dev server separately and have the backend
 * serve those files.
 *
 * @param env - Optional environment override (defaults to process.env.NODE_ENV)
 */
export function getStaticDirectory(env?: string): string {
  const effectiveEnv = env ?? process.env.NODE_ENV;
  const isProduction = effectiveEnv === 'production';

  if (isProduction) {
    return 'public';
  }

  // Development: prefer build/ (symlinked to ../app/build) if frontend is built.
  // Use relative path since express.static also uses relative paths.
  if (fs.existsSync('build/index.html')) {
    return 'build';
  }

  return 'public';
}

/**
 * Validate that the static directory exists and contains index.html.
 *
 * @throws {StaticConfigError} If the directory or index.html is missing
 */
export function validateStaticDirectory(dir: string): void {
  if (!fs.existsSync(dir)) {
    throw new StaticConfigError(
      `Static directory not found: ${dir}. ` + `Ensure the frontend is built (pnpm --filter @simlin/app build) or deployed.`,
    );
  }

  const indexPath = path.join(dir, 'index.html');
  if (!fs.existsSync(indexPath)) {
    throw new StaticConfigError(
      `Required file not found: ${indexPath}. ` + `Ensure the frontend is built or deployed.`,
    );
  }
}
