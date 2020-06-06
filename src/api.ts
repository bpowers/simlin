// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Request, Response, Router } from 'express';
import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import * as logger from 'winston';

import { Application } from './application';
import { MongoDuplicateKeyCode } from './models/common';
import { Database } from './models/db-interfaces';
import { populateExamples } from './new-user';
import { createFile, createProject, emptyProject } from './project-creation';
import { renderToPNG } from './render';
import { Preview as PreviewPb } from './schemas/preview_pb';
import { Project as ProjectPb } from './schemas/project_pb';
import { User as UserPb } from './schemas/user_pb';
import { UsernameDenylist } from './usernames';

export async function updatePreview(db: Database, project: ProjectPb): Promise<PreviewPb> {
  const fileDoc = await db.file.findOne(project.getFileId());
  if (!fileDoc) {
    throw new Error(`no File document found for project ${project.getId()}`);
  }

  let png: Buffer;
  try {
    png = await renderToPNG(fileDoc);
  } catch (err) {
    throw new Error(`renderToPNG: ${err.message}`);
  }

  const created = new Timestamp();
  created.fromDate(new Date());

  const preview = new PreviewPb();
  preview.setId(project.getId());
  preview.setPng(png);
  preview.setCreated(created);

  await db.preview.create(preview.getId(), preview);

  return preview;
}

export const getUser = (req: Request, res: Response): UserPb => {
  const user = (req.user as unknown) as UserPb | undefined;
  if (!user) {
    logger.warn(`user not found, but passed authz?`);
    res.status(500).json({});
    throw new Error(`user not found, but passed authz?`);
  }
  return user;
};

export const apiRouter = (app: Application): Router => {
  const api = Router();

  api.get('/user', (req: Request, res: Response): void => {
    const user = getUser(req, res);
    res.status(200).json(user.toObject());
  });

  // create a new project
  api.post(
    '/projects',
    async (req: Request, res: Response): Promise<void> => {
      const user = getUser(req, res);

      if (!req.body.projectName) {
        res.status(400).json({ error: 'projectName is required' });
        return;
      }

      const projectName = (req.body.projectName as string) || '';
      const projectDescription = (req.body.description as string) || '';
      const isPublic = !!req.body.isPublic;

      try {
        const project = createProject(user, projectName, projectDescription, isPublic);
        const json = project.toObject();

        let sdJSON: string;
        if (req.body.projectJSON) {
          // TODO: ensure this is really a valid project...
          sdJSON = JSON.stringify(req.body.projectJSON);
        } else {
          sdJSON = JSON.stringify(emptyProject(projectName, user.getDisplayName()));
        }

        const filePb = createFile(project.getId(), user.getId(), undefined, sdJSON);
        await app.db.file.create(filePb.getId(), filePb);

        project.setFileId(filePb.getId());
        await app.db.project.create(project.getId(), project);

        res.status(200).json(json);
      } catch (err) {
        if (err.code === MongoDuplicateKeyCode) {
          res.status(400).json({ error: 'project name already taken' });
          return;
        }
        logger.error(':ohno:');
        logger.error(err);
        throw err;
      }
    },
  );

  api.get(
    '/projects',
    async (req: Request, res: Response): Promise<void> => {
      const user = getUser(req, res);
      const projectModels = await app.db.project.find(user.getId() + '/');
      const projects = await Promise.all(projectModels.map((project: ProjectPb) => project.toObject()));
      res.status(200).json(projects);
    },
  );

  api.get(
    '/projects/:username/:projectName',
    async (req: Request, res: Response): Promise<void> => {
      let authorUser: UserPb | undefined = getUser(req, res);
      if (authorUser.getId() !== req.params.username) {
        authorUser = await app.db.user.findOne(req.params.username);
      }
      if (!authorUser) {
        res.status(404).json({});
        return;
      }

      const projectSlug = `${req.params.username}/${req.params.projectName}`;
      const projectModel = await app.db.project.findOne(projectSlug);

      // the username check is skipped if the model exists and is public
      if (!projectModel?.getIsPublic()) {
        // TODO: implement collaborators
        if (
          !req.session ||
          !req.session.passport ||
          !req.session.passport.user ||
          authorUser.getId() !== req.session.passport.user.id
        ) {
          res.status(401).json({});
          return;
        }
      }

      if (!projectModel || !projectModel.getFileId()) {
        res.status(404).json({});
        return;
      }

      const file = await app.db.file.findOne(projectModel.getFileId());
      if (!file) {
        res.status(404).json({});
        return;
      }

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const project: any = projectModel.toObject();
      project.user = authorUser;
      project.file = file.getJsonContents();

      res.status(200).json(project);
    },
  );

  api.get(
    '/preview/:username/:projectName',
    async (req: Request, res: Response): Promise<void> => {
      let authorUser: UserPb | undefined = getUser(req, res);
      if (authorUser.getId() !== req.params.username) {
        authorUser = await app.db.user.findOne(req.params.username);
      }
      if (!authorUser) {
        res.status(404).json({});
        return;
      }

      const projectSlug = `${req.params.username}/${req.params.projectName}`;
      const projectModel = await app.db.project.findOne(projectSlug);

      // the username check is skipped if the model exists and is public
      if (!projectModel?.getIsPublic()) {
        // TODO: implement collaborators
        if (
          !req.session ||
          !req.session.passport ||
          !req.session.passport.user ||
          authorUser.getId() !== req.session.passport.user.id
        ) {
          res.status(401).json({});
          return;
        }
      }

      if (!projectModel || !projectModel.getFileId()) {
        res.status(404).json({});
        return;
      }

      let previewModel = await app.db.preview.findOne(projectSlug);
      if (!previewModel) {
        try {
          previewModel = await updatePreview(app.db, projectModel);
        } catch (err) {
          logger.error(`updatePreview: ${err}`);
          res.status(500).json({});
          return;
        }
      }

      const png = Buffer.from(previewModel.getPng() as Uint8Array);

      res.contentType('image/png');
      res.status(200).send(png);
    },
  );

  api.post(
    '/projects/:username/:projectName',
    async (req: Request, res: Response): Promise<void> => {
      const user = getUser(req, res);
      // TODO
      if (user.getId() !== req.params.username) {
        res.status(401).json({});
        return;
      }
      const projectSlug = `${req.params.username}/${req.params.projectName}`;
      const projectModel = await app.db.project.findOne(projectSlug);
      if (!projectModel || !projectModel.getFileId()) {
        res.status(404).json({});
        return;
      }

      if (!req.body || !req.body.currVersion) {
        res.status(400).json({ error: 'currVersion is required' });
        return;
      }

      if (!req.body.file) {
        res.status(400).json({ error: 'file is required' });
        return;
      }

      const projectVersion = req.body.currVersion as number;
      const newVersion = projectVersion + 1;
      const fileContents = req.body.file as string;

      const jsonContents = JSON.stringify(fileContents);

      const file = createFile(projectModel.getId(), user.getId(), undefined, jsonContents);
      await app.db.file.create(file.getId(), file);

      // only update if the version matches
      projectModel.setFileId(file.getId());
      projectModel.setVersion(newVersion);

      const result = await app.db.project.update(
        projectModel.getId(),
        {
          version: projectVersion,
        },
        projectModel,
      );

      // remove our preview
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(async () => {
        try {
          await app.db.preview.deleteOne(projectModel.getId());
        } catch (err) {
          logger.warn(`unable to delete preview for ${req.params.projectName}`);
        }
      });

      // if the result is null we weren't able to find a matching
      // version in the DB, probably due to concurrent modification in
      // a different browser tab
      if (result === null) {
        res.status(409).json({
          error: `error saving model: changes based on old version. refresh page to reload`,
        });
        return;
      }

      res.status(200).json({ version: newVersion });
    },
  );

  api.patch(
    '/user',
    async (req: Request, res: Response): Promise<void> => {
      const userModel = getUser(req, res);

      if (Object.keys(req.body).length !== 1 || !req.body.username) {
        res.status(400).json({ error: 'only username can be patched' });
        return;
      }

      const proposedUsername = req.body.username as string;

      if (UsernameDenylist.has(proposedUsername)) {
        res.status(400).json({ error: 'username already taken' });
        return;
      }

      if (!userModel.getId().startsWith(`temp-`)) {
        res.status(403).json({ error: 'username already set' });
        return;
      }

      const origUserId = userModel.getId();

      userModel.setId(proposedUsername);
      userModel.setCanCreateProjects(true);
      try {
        // updating the primary key of a user doesn't work in mongo
        await app.db.user.create(userModel.getId(), userModel);
        await app.db.user.deleteOne(origUserId);
      } catch (err) {
        if (err.code === MongoDuplicateKeyCode) {
          res.status(400).json({ error: 'username already taken' });
          return;
        }
        throw err;
      }

      req.session.passport.user.id = userModel.getId();

      const defaultProjectsDir = app.get('defaultProjectsDir') as string;
      // this error shouldn't ever happen, but also shouldn't be fatal
      if (defaultProjectsDir) {
        try {
          await populateExamples(app.db, userModel, defaultProjectsDir);
        } catch (err) {
          logger.error(`populateExamples(${userModel.getId()}, ${defaultProjectsDir}): ${err}`);
        }
      } else {
        logger.error('missing defaultProjectsDir in config');
      }

      res.status(200).json({});
    },
  );

  return api;
};
