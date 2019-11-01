// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Application, Request, Response, Router } from 'express';
import { List } from 'immutable';
import * as logger from 'winston';

import {
  File as XmileFile,
  Header as XmileHeader,
  Model as XmileModel,
  SimSpec as XmileSimSpec,
  View as XmileView,
  ViewDefaults,
} from './engine/xmile';

import { MongoDuplicateKeyCode } from './models/common';
import { File } from './models/file';
import { Preview, updatePreview } from './models/preview';
import { newProject, Project, ProjectDocument } from './models/project';
import { User, UserDocument } from './models/user';
import { populateExamples } from './new-user';
import { UsernameDenylist } from './usernames';

function emptyProject(name: string, userName: string): XmileFile {
  return new XmileFile({
    header: new XmileHeader({
      vendor: 'systemdynamics.net',
      product: 'Model v1.0',
      name,
      author: userName,
    } as any),
    simSpec: new XmileSimSpec({
      start: 0,
      stop: 100,
    } as any),
    models: List([
      new XmileModel({
        views: List([new XmileView(ViewDefaults)]),
      } as any),
    ]),
  } as any);
}

export const getUser = (req: Request, res: Response): UserDocument => {
  const user: UserDocument | undefined = req.user as any;
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
    res.status(200).json(user.toJSON());
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

      const projectName = req.body.projectName;
      const projectDescription = req.body.description || '';

      try {
        const project = await newProject(user, projectName, projectDescription);
        const json = await project.toJSON();

        if (req.body.isPublic) {
          project.isPublic = true;
        }

        let sdJSON: string;
        if (req.body.projectJSON) {
          // TODO: ensure this is really a valid project...
          sdJSON = JSON.stringify(req.body.projectJSON);
        } else {
          sdJSON = JSON.stringify(emptyProject(projectName, user.displayName));
        }

        const file = await File.create({
          project: project._id,
          user: user._id,
          created: new Date(Date.now()),
          contents: sdJSON,
        });

        project.fileId = file._id;
        await project.save();

        res.status(200).json(json);
      } catch (err) {
        if (err.code === MongoDuplicateKeyCode) {
          res.status(400).json({ error: 'project name already taken' });
          return;
        }
        console.log(':ohno:');
        console.log(err);
        throw err;
      }
    },
  );

  api.get(
    '/projects',
    async (req: Request, res: Response): Promise<void> => {
      const user = getUser(req, res);
      const projectModels = await Project.find({ owner: user._id }).exec();
      const projects = await Promise.all(projectModels.map(async (project: ProjectDocument) => project.toJSON()));
      res.status(200).json(projects);
    },
  );

  api.get(
    '/projects/:username/:projectName',
    async (req: Request, res: Response): Promise<void> => {
      const authorUser = await User.findOne({ username: req.params.username }).exec();
      if (!authorUser) {
        res.status(404).json({});
        return;
      }

      const projectName: string = req.params.projectName;
      const projectModel = await Project.findOne({ owner: authorUser._id, name: projectName }).exec();

      // the username check is skipped if the model exists and is public
      if (!(projectModel && projectModel.isPublic)) {
        // TODO: implement collaborators
        if (
          !req.session ||
          !req.session.passport ||
          !req.session.passport.user ||
          authorUser.email !== req.session.passport.user.email
        ) {
          res.status(401).json({});
          return;
        }
      }

      if (!projectModel || !projectModel.fileId) {
        res.status(404).json({});
        return;
      }

      const file = await File.findById(projectModel.fileId).exec();
      if (!file) {
        res.status(404).json({});
        return;
      }

      const project = await projectModel.toJSON();
      project.file = file.contents;

      res.status(200).json(project);
    },
  );

  api.get(
    '/preview/:username/:projectName',
    async (req: Request, res: Response): Promise<void> => {
      const authorUser = await User.findOne({ username: req.params.username }).exec();
      if (!authorUser) {
        res.status(404).json({});
        return;
      }

      const projectName: string = req.params.projectName;
      const projectModel = await Project.findOne({ owner: authorUser._id, name: projectName }).exec();

      // the username check is skipped if the model exists and is public
      if (!(projectModel && projectModel.isPublic)) {
        // TODO: implement collaborators
        if (
          !req.session ||
          !req.session.passport ||
          !req.session.passport.user ||
          authorUser.email !== req.session.passport.user.email
        ) {
          res.status(401).json({});
          return;
        }
      }

      if (!projectModel || !projectModel.fileId) {
        res.status(404).json({});
        return;
      }

      let previewModel = await Preview.findOne({ project: projectModel.id });
      if (!previewModel) {
        try {
          previewModel = await updatePreview(projectModel);
        } catch (err) {
          logger.error(`updatePreview: ${err}`);
          res.status(500).json({});
          return;
        }
      }

      res.contentType('image/png');
      res.status(200).send(previewModel.png);
    },
  );

  api.post(
    '/projects/:username/:projectName',
    async (req: Request, res: Response): Promise<void> => {
      const user = getUser(req, res);
      // TODO
      if (user.username !== req.params.username) {
        res.status(401).json({});
        return;
      }
      const projectName: string = req.params.projectName;
      const projectModel = await Project.findOne({ owner: user._id, name: projectName }).exec();
      if (!projectModel || !projectModel.fileId) {
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

      const projectVersion: number = req.body.currVersion;
      const newVersion = projectVersion + 1;
      const fileContents: string = req.body.file;

      const file = await File.create({
        project: projectModel._id,
        user: user._id,
        created: new Date(Date.now()),
        contents: JSON.stringify(fileContents),
      });

      // only update if the version matches
      const result = await Project.findOneAndUpdate(
        {
          owner: user._id,
          name: projectName,
          version: projectVersion,
        },
        {
          version: newVersion,
          fileId: file._id,
        },
        {
          new: true,
        },
      ).exec();

      // remove our preview
      setTimeout(async () => {
        try {
          await Preview.deleteOne({ project: projectModel.id });
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

      const proposedUsername = req.body.username;

      if (UsernameDenylist.has(proposedUsername)) {
        res.status(400).json({ error: 'username already taken' });
        return;
      }

      if (userModel.username) {
        res.status(403).json({ error: 'username already set' });
        return;
      }

      userModel.username = proposedUsername;
      try {
        await userModel.save();
      } catch (err) {
        if (err.code === MongoDuplicateKeyCode) {
          res.status(400).json({ error: 'username already taken' });
          return;
        }
        throw err;
      }

      const defaultProjectsDir = app.get('defaultProjectsDir');
      // this error shouldn't ever happen, but also shouldn't be fatal
      if (defaultProjectsDir) {
        try {
          await populateExamples(userModel, defaultProjectsDir);
        } catch (err) {
          logger.error(`populateExamples(${userModel.username}, ${defaultProjectsDir}): ${err}`);
        }
      } else {
        logger.error('missing defaultProjectsDir in config');
      }

      res.status(200).json({});
    },
  );

  return api;
};
