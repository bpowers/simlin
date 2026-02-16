// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { NextFunction, Request, Response } from 'express';
import * as logger from 'winston';

import { getAuthenticatedUser, isResourceOwner } from './auth-helpers';

/**
 * Interface for the project database operations needed by the route handler.
 */
export interface ProjectDb {
  findOne(id: string): Promise<ProjectRecord | undefined>;
}

/**
 * Interface representing a project record from the database.
 */
export interface ProjectRecord {
  getId(): string;
  getOwnerId(): string;
  getIsPublic(): boolean;
  getFileId(): string | undefined;
}

export interface ProjectRouteHandlerDeps {
  db: { project: ProjectDb };
}

/**
 * Create the route handler for /:username/:projectName
 *
 * This handler:
 * 1. Looks up the project by username/projectName
 * 2. Returns 404 if project not found
 * 3. Redirects to /?project=... for public projects
 * 4. For private projects:
 *    - Redirects unauthenticated users to /
 *    - Redirects non-owners to /
 *    - Serves index.html for authenticated owners
 */
export function createProjectRouteHandler(deps: ProjectRouteHandlerDeps) {
  return async (req: Request, res: Response, next: NextFunction): Promise<void> => {
    const username = req.params.username as string;
    const projectName = req.params.projectName as string;
    const projectId = `${username}/${projectName}`;
    const project = await deps.db.project.findOne(projectId);

    if (!project) {
      res.status(404).json({});
      return;
    }

    if (project.getIsPublic()) {
      res.redirect(encodeURI(`/?project=${project.getId()}`));
      return;
    }

    // Private project - check authentication BEFORE accessing session data
    const authUser = getAuthenticatedUser(req);

    if (!authUser) {
      logger.debug(`Unauthenticated access to private project ${projectId}, redirecting`);
      res.redirect('/');
      return;
    }

    // Check if authenticated user owns this project
    if (!isResourceOwner(authUser, project.getOwnerId())) {
      // User doesn't own this private project - redirect to home
      res.redirect('/');
      return;
    }

    // Validate path matches expected format
    const expectedPath = `/${username}/${projectName}`;
    if (req.path !== expectedPath && req.path !== `${expectedPath}/`) {
      res.status(404).json({});
      return;
    }

    // Verify project has a file (shouldn't happen but defensive check)
    if (!project.getFileId()) {
      logger.error(`Project ${projectId} exists but has no file`);
      res.status(404).json({});
      return;
    }

    // Serve the app for the authenticated owner
    req.url = '/index.html';
    res.set('Cache-Control', 'no-store');
    res.set('Max-Age', '0');
    next();
  };
}
